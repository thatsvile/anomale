use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Label, ListBox, ListBoxRow, Entry, ScrolledWindow, Orientation, SelectionMode, Image};
use gtk4::gio;
use gtk4::glib;
use crate::config::AppConfig;
use crate::popup_window::{
    prepare_popup_window, present_popup, resize_popup_to_content_at, PopupOptions,
};
use std::rc::Rc;
use std::cell::RefCell;
use std::time::Duration;

const FILTER_DEBOUNCE_MS: u64 = 50;
/// Visible height for the app results list when searching.
const APPS_LIST_HEIGHT: i32 = 320;
/// Match niri `default-floating-position` for the apps launcher popup.
const APPS_TOP_Y: i32 = 200;

pub struct AppLauncher {
    pub window: ApplicationWindow,
    pub search_entry: Entry,
    pub list_box: ListBox,
    pub scrolled_window: ScrolledWindow,
    pub apps: RefCell<Vec<gio::AppInfo>>,
    pub current_matches: RefCell<Vec<gio::AppInfo>>,
    css_provider: gtk4::CssProvider,
    filter_timeout: RefCell<Option<glib::SourceId>>,
}

impl AppLauncher {
    pub fn new(app: &Application, css_provider_ref: &gtk4::CssProvider) -> Rc<RefCell<Self>> {
        let config = AppConfig::load().unwrap_or_else(|e| {
            eprintln!("Failed to load apps config: {}. Using defaults.", e);
            AppConfig::default()
        });

        let window = ApplicationWindow::builder()
            .application(app)
            .title("Anomale Launcher")
            .decorated(false)
            .visible(false) // Initially hidden
            .build();

        prepare_popup_window(
            &window,
            PopupOptions::from_search_width(config.search_width),
        );

        // Apply CSS (Initial load)
        let css = config.generate_css(None);
        css_provider_ref.load_from_data(&css);
        let css_provider = css_provider_ref.clone();

        window.add_css_class("apps-window");

        let popup_width = config.search_width + 20;
        let launcher_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .hexpand(true)
            .halign(Align::Fill)
            .build();
        launcher_box.set_width_request(popup_width);
        launcher_box.add_css_class("launcher-box");

        // Results List
        let list_box = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .hexpand(true)
            .halign(Align::Fill)
            .build();
        list_box.add_css_class("app-list");

        // Search Entry
        let search_entry = Entry::builder()
            .placeholder_text("Search apps...")
            .hexpand(true)
            .halign(Align::Fill)
            .build();
        search_entry.add_css_class("search-entry");

        launcher_box.append(&search_entry);
        
        // List Box Key Controller (for Up navigation back to entry)
        let list_controller = gtk4::EventControllerKey::new();
        let search_entry_clone_list = search_entry.clone();
        list_controller.connect_key_pressed(move |controller, key, _, _| {
             if key == gtk4::gdk::Key::Up {
                 if let Some(widget) = controller.widget() {
                    if let Some(list_box_widget) = widget.downcast_ref::<ListBox>() {
                        if let Some(row) = list_box_widget.selected_row() {
                            if row.index() == 0 {
                                    search_entry_clone_list.grab_focus();
                                    return gtk4::glib::Propagation::Stop;
                            }
                        }
                    }
                 }
             }
             gtk4::glib::Propagation::Proceed
        });
        list_box.add_controller(list_controller);



        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&list_box)
            .hexpand(true)
            .halign(Align::Fill)
            .build();

        // Auto-scroll to selected row
        let scrolled_window_clone_nav = scrolled_window.clone();
        list_box.connect_row_selected(move |_, row| {
             if let Some(row) = row {
                 let adj = scrolled_window_clone_nav.vadjustment();
                 let allocation = row.allocation();
                 let y = allocation.y() as f64;
                 let height = allocation.height() as f64;
                 let val = adj.value();
                 let page = adj.page_size();
                 
                 if y < val {
                     adj.set_value(y);
                 } else if y + height > val + page {
                     adj.set_value(y + height - page);
                 }
             }
        });

        scrolled_window.set_visible(false);

        launcher_box.append(&scrolled_window);
        window.set_child(Some(&launcher_box));

        // Load Apps
        let apps = gio::AppInfo::all();

        let launcher = Rc::new(RefCell::new(Self {
            window,
            search_entry,
            list_box,
            scrolled_window,
            apps: RefCell::new(apps),
            current_matches: RefCell::new(Vec::new()),
            css_provider,
            filter_timeout: RefCell::new(None),
        }));

        // Search Entry Key Controller
        let entry_controller = gtk4::EventControllerKey::new();
        let list_box_clone_entry = launcher.borrow().list_box.clone();
        let launcher_clone_entry_key = launcher.clone();
        entry_controller.connect_key_pressed(move |controller, key, _, _| {
            if key == gtk4::gdk::Key::Down {
                if let Some(entry) = controller.widget().and_downcast::<Entry>() {
                    let text = entry.text();
                    launcher_clone_entry_key.borrow().flush_filter(&text);
                }
                if let Some(first) = list_box_clone_entry.row_at_index(0) {
                    list_box_clone_entry.select_row(Some(&first));
                    first.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        launcher.borrow().search_entry.add_controller(entry_controller);

        // Signals
        let launcher_clone = launcher.clone();
        launcher.borrow().search_entry.connect_changed(move |entry| {
            let text = entry.text();
            Self::schedule_filter(&launcher_clone, &text);
        });
        
        // Enter key in Entry launches selected app
        let launcher_clone_enter = launcher.clone();
        launcher.borrow().search_entry.connect_activate(move |entry| {
            let text = entry.text();
            let launcher_ref = launcher_clone_enter.borrow();
            launcher_ref.flush_filter(&text);

            // If a row is selected in the listbox, launch it
            let app_to_launch = if let Some(row) = launcher_ref.list_box.selected_row() {
                 let idx = row.index();
                 if idx >= 0 {
                     let app = launcher_ref.current_matches.borrow().get(idx as usize).cloned();
                     app
                 } else {
                     None
                 }
            } else {
                 // If no row is selected, but results exist, launch the first one (index 0)
                  let app = launcher_ref.current_matches.borrow().first().cloned();
                  app
            };
            drop(launcher_ref);

            if let Some(app) = app_to_launch {
                let ctx = gtk4::gdk::Display::default()
                      .map(|d| d.app_launch_context());
                let launch_ctx: Option<&gio::AppLaunchContext> = ctx.as_ref().map(|c| c.upcast_ref());
                if let Err(e) = app.launch(&[], launch_ctx) {
                      eprintln!("Failed to launch app: {}", e);
                } else {
                      launcher_clone_enter.borrow().window.set_visible(false);
                }
            }
        });
        
        let launcher_clone_activate = launcher.clone();
        launcher.borrow().list_box.connect_row_activated(move |_, row| {
             let idx = row.index();
             if idx >= 0 {
                 let apps_ref = launcher_clone_activate.borrow();
                 let app_to_launch = apps_ref.current_matches.borrow().get(idx as usize).cloned();
                 // Drop the borrow of apps_ref before launching (though less critical here as we aren't using it in the if body fundamentally, but good practice)
                 drop(apps_ref);

                 if let Some(app) = app_to_launch {
                      let ctx = gtk4::gdk::Display::default()
                          .map(|d| d.app_launch_context());
                      let launch_ctx: Option<&gio::AppLaunchContext> = ctx.as_ref().map(|c| c.upcast_ref());
                      if let Err(e) = app.launch(&[], launch_ctx) {
                          eprintln!("Failed to launch app: {}", e);
                      } else {
                          launcher_clone_activate.borrow().window.set_visible(false);
                      }
                 }
             }
        });

        // Handle Escape to close
        let key_controller = gtk4::EventControllerKey::new();
        let launcher_clone_key = launcher.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                launcher_clone_key.borrow().window.set_visible(false);
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        launcher.borrow().window.add_controller(key_controller);

        let launcher_for_hide = launcher.clone();
        launcher.borrow().window.connect_visible_notify(move |window| {
            if !window.is_visible() {
                launcher_for_hide.borrow().reset_to_initial_state();
            }
        });

        launcher
    }

    fn reset_to_initial_state(&self) {
        self.cancel_pending_filter();
        self.search_entry.set_text("");

        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }
        self.current_matches.borrow_mut().clear();

        self.scrolled_window.set_visible(false);
        self.scrolled_window.set_size_request(-1, 0);
        self.apply_compact_window_size();
    }

    fn apply_compact_window_size(&self) {
        if let Some(child) = self.window.child() {
            let (min_w, nat_w, _, _) = child.measure(Orientation::Horizontal, -1);
            let (min_h, nat_h, _, _) = child.measure(Orientation::Vertical, -1);
            let width = nat_w.max(min_w);
            let height = nat_h.max(min_h);
            if width > 0 && height > 0 {
                self.window.set_default_size(width, height);
                self.window.set_size_request(width, height);
            }
        }
    }

    pub fn toggle(&self) {
        self.cancel_pending_filter();

        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            // Refresh CSS from config (picks up pywal changes)
            let config = AppConfig::load().unwrap_or_default();
            self.css_provider.load_from_data(&config.generate_css(None));

            // Refresh apps list
            *self.apps.borrow_mut() = gio::AppInfo::all();

            // Reset before showing so we never flash the previous search/results size.
            self.reset_to_initial_state();
            present_popup(&self.window);
            self.search_entry.grab_focus();
            resize_popup_to_content_at(&self.window, Some(APPS_TOP_Y));
        }
    }

    fn cancel_pending_filter(&self) {
        if let Some(id) = self.filter_timeout.borrow_mut().take() {
            id.remove();
        }
    }

    fn flush_filter(&self, query: &str) {
        self.cancel_pending_filter();
        self.filter_apps(query);
    }

    fn schedule_filter(launcher: &Rc<RefCell<Self>>, query: &str) {
        launcher.borrow().cancel_pending_filter();

        if query.trim().is_empty() {
            launcher.borrow().filter_apps(query);
            return;
        }

        let query = query.to_string();
        let launcher_for_timeout = launcher.clone();
        let id = glib::timeout_add_local(Duration::from_millis(FILTER_DEBOUNCE_MS), move || {
            launcher_for_timeout.borrow().filter_apps(&query);
            *launcher_for_timeout.borrow().filter_timeout.borrow_mut() = None;
            glib::ControlFlow::Break
        });
        *launcher.borrow().filter_timeout.borrow_mut() = Some(id);
    }

    fn filter_apps(&self, query: &str) {
        // Clear existing items
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        // Clear matches
        self.current_matches.borrow_mut().clear();

        if query.trim().is_empty() {
            self.scrolled_window.set_visible(false);
            self.scrolled_window.set_size_request(-1, 0);
            resize_popup_to_content_at(&self.window, Some(APPS_TOP_Y));
            return;
        }

        self.scrolled_window.set_visible(true);
        self.scrolled_window.set_size_request(-1, APPS_LIST_HEIGHT);

        let query_lower = query.to_lowercase();
        
        // Scoring logic
        let apps = self.apps.borrow();
        let mut matches: Vec<(i32, gio::AppInfo)> = apps.iter().filter_map(|app| {
            let name = app.name().to_string();
            let name_lower = name.to_lowercase();
            let display_name = app.display_name().to_string();
            let display_name_lower = display_name.to_lowercase();
            let mut score = 0;

            if name_lower == query_lower || display_name_lower == query_lower {
                score = 100;
            } else if name_lower.starts_with(&query_lower) || display_name_lower.starts_with(&query_lower) {
                score = 80;
            } else if name_lower.contains(&query_lower) || display_name_lower.contains(&query_lower) {
                score = 60;
            } else {
                // Check executable (commandline) without panicking
                if let Some(cmd) = app.commandline() {
                    let cmd_lower = cmd.to_string_lossy().to_lowercase();
                    if !cmd_lower.is_empty() && cmd_lower.contains(&query_lower) {
                         score = 40;
                    }
                }
            }

            if score > 0 {
                Some((score, app.clone()))
            } else {
                None
            }
        }).collect();

        // Sort by score desc, then name asc
        matches.sort_by(|a, b| {
            b.0.cmp(&a.0).then_with(|| a.1.name().cmp(&b.1.name()))
        });

        // Deduplicate by display name (keep highest-scored entry)
        let mut seen_names = std::collections::HashSet::new();
        matches.retain(|(_, app)| seen_names.insert(app.name().to_string()));
        
        if matches.is_empty() {
             self.scrolled_window.set_visible(false);
             self.scrolled_window.set_size_request(-1, 0);
             resize_popup_to_content_at(&self.window, Some(APPS_TOP_Y));
             return;
        }

        // Take top 50, store them in matches, and create rows
        let mut row_matches = Vec::new();
        for (_, app) in matches.into_iter().take(50) {
            row_matches.push(app.clone());

            let row = ListBoxRow::new();
            
            // Horizontal box for icon + label
            let row_box = gtk4::Box::builder()
                .orientation(Orientation::Horizontal)
                .spacing(10)
                .build();

            // App icon
            let icon_widget = if let Some(icon) = app.icon() {
                let img = Image::from_gicon(&icon);
                img.set_pixel_size(24);
                img
            } else {
                let img = Image::from_icon_name("application-x-executable");
                img.set_pixel_size(24);
                img
            };
            icon_widget.add_css_class("app-icon");
            row_box.append(&icon_widget);

            // App name
            let label = Label::new(Some(&app.name()));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_hexpand(true);
            label.set_width_chars(1); 
            label.set_xalign(0.0);
            row_box.append(&label);
            
            row.set_child(Some(&row_box));
            

            self.list_box.append(&row);
        }

        *self.current_matches.borrow_mut() = row_matches;

        self.scrolled_window.set_size_request(-1, APPS_LIST_HEIGHT);
        resize_popup_to_content_at(&self.window, Some(APPS_TOP_Y));

        if let Some(row) = self.list_box.first_child() {
             if let Some(row_widget) = row.downcast_ref::<ListBoxRow>() {
                  self.list_box.select_row(Some(row_widget));
             }
        }
    }
}

