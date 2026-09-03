use gtk4::prelude::*;
use gtk4::Application;
use crate::config::NotifyConfig;
use crate::notification_window::NotificationWindow;
use crate::notify_server::NotifyEvent;
use std::rc::Rc;
use std::cell::RefCell;
use async_channel::Sender;

pub struct NotifyManager {
    app: Application,
    active_notifications: RefCell<Vec<Rc<NotificationWindow>>>,
    config: RefCell<NotifyConfig>,
    id_counter: std::sync::atomic::AtomicU32,
    events_tx: Sender<NotifyEvent>,
    enabled: RefCell<bool>,
    css_provider: RefCell<Option<gtk4::CssProvider>>,
    last_notification_css: RefCell<String>,
}

impl NotifyManager {
    pub fn new(app: &Application, events_tx: Sender<NotifyEvent>) -> Rc<Self> {
        let config = NotifyConfig::load().unwrap_or_default();
        
        Rc::new(Self {
            app: app.clone(),
            active_notifications: RefCell::new(Vec::new()),
            config: RefCell::new(config),
            id_counter: std::sync::atomic::AtomicU32::new(1),
            events_tx,
            enabled: RefCell::new(true),
            css_provider: RefCell::new(None),
            last_notification_css: RefCell::new(String::new()),
        })
    }

    /// Enable or disable popup display without restarting.
    pub fn set_enabled(self: &Rc<Self>, enabled: bool) {
        let was_enabled = *self.enabled.borrow();
        *self.enabled.borrow_mut() = enabled;
        if was_enabled && !enabled {
            self.dismiss_all();
        }
        println!(
            "Notifications {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    pub fn toggle_enabled(self: &Rc<Self>) {
        let next = !*self.enabled.borrow();
        self.set_enabled(next);
    }

    fn dismiss_all(self: &Rc<Self>) {
        let ids: Vec<u32> = self
            .active_notifications
            .borrow()
            .iter()
            .map(|n| n.id)
            .collect();
        for id in ids {
            self.dismiss_notification(id);
        }
    }

    fn apply_notification_css(self: &Rc<Self>, config: &NotifyConfig) {
        let css = config.generate_css();
        if *self.last_notification_css.borrow() == css {
            return;
        }
        *self.last_notification_css.borrow_mut() = css.clone();

        let mut store = self.css_provider.borrow_mut();
        if store.is_none() {
            let provider = gtk4::CssProvider::new();
            if let Some(display) = gtk4::gdk::Display::default() {
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_USER,
                );
            }
            *store = Some(provider);
        }
        if let Some(provider) = store.as_ref() {
            provider.load_from_data(&css);
        }
    }
    fn resolve_monitor(&self, config: &NotifyConfig) -> Option<gtk4::gdk::Monitor> {
        let display = gtk4::gdk::Display::default()?;
        let monitors = display.monitors();

        if let Some(name) = config.monitor.as_ref() {
            for i in 0..monitors.n_items() {
                if let Some(monitor) = monitors.item(i).and_downcast::<gtk4::gdk::Monitor>() {
                    if monitor.connector().as_deref() == Some(name.as_str()) {
                        return Some(monitor);
                    }
                }
            }
            eprintln!(
                "Warning: notification monitor '{}' not found; falling back to primary",
                name
            );
        }

        if monitors.n_items() > 0 {
            monitors.item(0).and_downcast::<gtk4::gdk::Monitor>()
        } else {
            None
        }
    }

    pub fn handle_event(self: &Rc<Self>, event: NotifyEvent) {
        match event {
            NotifyEvent::Notify {
                app_name,
                replaces_id,
                app_icon,
                summary,
                body,
                hints,
                expire_timeout,
                id_sender,
            } => {
                let id = if replaces_id > 0 {
                    replaces_id
                } else {
                    self.id_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                };

                if !*self.enabled.borrow() {
                    let _ = id_sender.send(id);
                    return;
                }

                self.remove_notification(id);

                let mut config = self.config.borrow().clone();

                if let Some(urgency) = hints.get("urgency").and_then(|v| v.downcast_ref::<u8>().ok()) {
                    if urgency == 3 {
                        config.border_color = "#ff0000".to_string();
                    }
                }

                let monitor = self.resolve_monitor(&config);

                self.apply_notification_css(&config);

                let notify_win = NotificationWindow::new(
                    &self.app,
                    id,
                    &app_name,
                    &summary,
                    &body,
                    &app_icon,
                    config,
                    monitor.as_ref(),
                );

                self.active_notifications.borrow_mut().insert(0, notify_win.clone());

                let gesture = gtk4::GestureClick::new();
                let id_clone = id;
                let tx_clone = self.events_tx.clone();
                gesture.connect_pressed(move |_, _, _, _| {
                    let _ = tx_clone.send_blocking(NotifyEvent::ActionInvoked(id_clone, "default".to_string()));
                });
                notify_win.window.add_controller(gesture);

                let manager_clone = self.clone();
                let timeout_ms = if expire_timeout > 0 {
                    expire_timeout as u32
                } else {
                    (self.config.borrow().timeout * 1000) as u32
                };

                gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(timeout_ms as u64), move || {
                    manager_clone.dismiss_notification(id);
                });

                self.update_positions();
                notify_win.show();

                let _ = id_sender.send(id);
            }
            NotifyEvent::Close(id) => {
                self.dismiss_notification(id);
            }
            _ => {}
        }
    }

    fn remove_notification(&self, id: u32) {
        let mut active = self.active_notifications.borrow_mut();
        if let Some(pos) = active.iter().position(|n| n.id == id) {
            let n = active.remove(pos);
            n.window.close();
        }
    }

    fn dismiss_notification(self: &Rc<Self>, id: u32) {
        let n: Option<Rc<NotificationWindow>> = {
            let mut active = self.active_notifications.borrow_mut();
            if let Some(pos) = active.iter().position(|n| n.id == id) {
                Some(active.remove(pos))
            } else {
                None
            }
        };

        if let Some(n) = n {
            self.update_positions();

            let tx_clone = self.events_tx.clone();
            n.hide(move || {
                let _ = tx_clone.send_blocking(NotifyEvent::NotificationClosed(id, 2));
            });
        }
    }

    fn update_positions(&self) {
        let active = self.active_notifications.borrow();
        let config = self.config.borrow();
        let mut current_offset = 0;

        for n in active.iter() {
            n.set_y_offset(current_offset);
            current_offset += config.height + config.spacing;
        }
    }
}
