use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Label, ListBox, ListBoxRow, Orientation, SelectionMode};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;

use crate::config::AppConfig;

pub struct ActionMenu {
    pub window: ApplicationWindow,
    pub list_box: ListBox,
    css_provider: gtk4::CssProvider,
}

impl ActionMenu {
    pub fn new(app: &Application, css_provider_ref: &gtk4::CssProvider, menu_id: &str) -> Rc<RefCell<Self>> {
        let config = AppConfig::load().unwrap_or_else(|e| {
            eprintln!("Failed to load menus config: {}. Using defaults.", e);
            AppConfig::default()
        });

        let window = ApplicationWindow::builder()
            .application(app)
            .title(format!("Anomale {} Menu", capitalize_menu_id(menu_id)))
            .decorated(false)
            .visible(false)
            .build();

        window.init_layer_shell();
        window.set_namespace(&menu_namespace(menu_id));
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_exclusive_zone(-1);

        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);

        let css = config.generate_css(None);
        css_provider_ref.load_from_data(&css);
        let css_provider = css_provider_ref.clone();

        let overlay_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .halign(Align::Center)
            .valign(Align::Center)
            .build();

        if menu_id == "power" {
            window.add_css_class("power-window");
        } else {
            window.add_css_class("action-menu-window");
        }

        let launcher_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        launcher_box.set_width_request(config.search_width + 10);
        launcher_box.add_css_class("launcher-box");

        let list_box = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        list_box.add_css_class("app-list");

        launcher_box.append(&list_box);
        overlay_box.append(&launcher_box);
        window.set_child(Some(&overlay_box));

        let menu = Rc::new(RefCell::new(Self {
            window,
            list_box,
            css_provider,
        }));

        let menu_clone_activate = menu.clone();
        menu.borrow().list_box.connect_row_activated(move |_, row| {
            unsafe {
                if let Some(cmd) = row.data::<String>("command") {
                    let cmd_str = cmd.as_ref();
                    let _ = Command::new("sh").arg("-c").arg(cmd_str).spawn();
                    menu_clone_activate.borrow().window.set_visible(false);
                }
            }
        });

        let list_box_clone_nav = menu.borrow().list_box.clone();
        let list_nav_controller = gtk4::EventControllerKey::new();
        list_nav_controller.connect_key_pressed(move |_, key, _, _| {
            if list_box_clone_nav.selected_row().is_some() {
                return gtk4::glib::Propagation::Proceed;
            }

            let row_count = {
                let mut count = 0;
                while list_box_clone_nav.row_at_index(count).is_some() {
                    count += 1;
                }
                count
            };
            if row_count == 0 {
                return gtk4::glib::Propagation::Proceed;
            }

            let target_index = match key {
                k if k == gtk4::gdk::Key::Down => Some(0),
                k if k == gtk4::gdk::Key::Up => Some(row_count - 1),
                _ => None,
            };

            if let Some(index) = target_index {
                if let Some(row) = list_box_clone_nav.row_at_index(index) {
                    list_box_clone_nav.select_row(Some(&row));
                    row.grab_focus();
                    return gtk4::glib::Propagation::Stop;
                }
            }

            gtk4::glib::Propagation::Proceed
        });
        menu.borrow().list_box.add_controller(list_nav_controller);

        let key_controller = gtk4::EventControllerKey::new();
        let menu_clone_key = menu.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                menu_clone_key.borrow().window.set_visible(false);
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        menu.borrow().window.add_controller(key_controller);

        let click_controller = gtk4::GestureClick::new();
        let menu_clone_click = menu.clone();
        click_controller.connect_released(move |_, _, x, y| {
            let menu = menu_clone_click.borrow();
            if let Some(child) = menu.window.child() {
                if let Some(overlay) = child.first_child() {
                    let alloc = overlay.allocation();
                    let bx = alloc.x() as f64;
                    let by = alloc.y() as f64;
                    let bw = alloc.width() as f64;
                    let bh = alloc.height() as f64;
                    if x < bx || x > bx + bw || y < by || y > by + bh {
                        menu.window.set_visible(false);
                    }
                }
            }
        });
        menu.borrow().window.add_controller(click_controller);

        menu
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn reload(&self, config: &AppConfig, actions: &[(String, String)]) {
        self.css_provider.load_from_data(&config.generate_css(None));

        while let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.remove(&row);
        }

        for (label_text, cmd) in actions {
            let row = ListBoxRow::new();

            let row_box = gtk4::Box::builder()
                .orientation(Orientation::Horizontal)
                .spacing(10)
                .build();

            let label = Label::new(Some(label_text));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_hexpand(true);
            label.set_width_chars(1);
            label.set_halign(Align::Center);
            label.set_xalign(0.5);
            row_box.append(&label);

            row.set_child(Some(&row_box));

            unsafe {
                row.set_data("command", cmd.clone());
            }

            self.list_box.append(&row);
        }
    }

    pub fn show(&self) {
        self.window.set_visible(true);
        self.list_box.unselect_all();
        self.list_box.grab_focus();
    }

    pub fn toggle_open(&self, config: &AppConfig, actions: &[(String, String)]) {
        self.reload(config, actions);
        self.show();
    }
}

pub struct ActionMenuRegistry {
    menus: RefCell<HashMap<String, Rc<RefCell<ActionMenu>>>>,
}

impl ActionMenuRegistry {
    pub fn new() -> Self {
        Self {
            menus: RefCell::new(HashMap::new()),
        }
    }

    pub fn toggle(
        &self,
        app: &Application,
        css_provider: &gtk4::CssProvider,
        menu_id: &str,
    ) -> bool {
        let config = AppConfig::load().unwrap_or_default();
        if !config.action_menu_exists(menu_id) {
            eprintln!(
                "Unknown or empty action menu '{}'. Define entries in menus.conf as menu_{}_1=Label:command",
                menu_id, menu_id
            );
            return false;
        }
        let actions = config.action_menu_actions(menu_id).unwrap().clone();

        for (id, menu) in self.menus.borrow().iter() {
            if id != menu_id && menu.borrow().is_visible() {
                menu.borrow().hide();
            }
        }

        let menu = self.get_or_create(app, css_provider, menu_id);
        let menu_ref = menu.borrow();
        if menu_ref.is_visible() {
            menu_ref.hide();
            return true;
        }
        drop(menu_ref);

        menu.borrow().toggle_open(&config, &actions);
        true
    }

    fn get_or_create(
        &self,
        app: &Application,
        css_provider: &gtk4::CssProvider,
        menu_id: &str,
    ) -> Rc<RefCell<ActionMenu>> {
        let mut menus = self.menus.borrow_mut();
        if let Some(menu) = menus.get(menu_id) {
            return menu.clone();
        }

        let menu = ActionMenu::new(app, css_provider, menu_id);
        menus.insert(menu_id.to_string(), menu.clone());
        menu
    }
}

fn menu_namespace(menu_id: &str) -> String {
    if menu_id == "power" {
        "anomale-powermenu".to_string()
    } else {
        format!("anomale-menu-{}", menu_id)
    }
}

fn capitalize_menu_id(menu_id: &str) -> String {
    let mut chars = menu_id.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
