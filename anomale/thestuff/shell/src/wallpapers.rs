use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Button, Label, Orientation};
use gtk4::gdk_pixbuf::Pixbuf;
use crate::config::AppConfig;
use crate::popup_window::{prepare_popup_window, present_popup, PopupOptions};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

fn anomale_cache_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".cache/anomale"))
}

fn resolve_wallpaper_command(wall_path_str: &str, config: &AppConfig, command_name: Option<&str>) -> String {
    let path = PathBuf::from(wall_path_str);
    if let Some(stem) = path.file_stem() {
        if let Some(parent) = path.parent() {
            let txt_path = parent.join(stem).with_extension("txt");
            if txt_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&txt_path) {
                    let cmd_template = content.trim();
                    if !cmd_template.is_empty() {
                        return cmd_template.replace("[[w]]", wall_path_str);
                    }
                }
            }
        }
    }
    config
        .wallpaper_command_template(command_name)
        .replace("[[w]]", wall_path_str)
}

fn run_wallpaper_commands(wall_path: &str, config: &AppConfig, command_name: Option<&str>) {
    let main_cmd = resolve_wallpaper_command(wall_path, config, command_name);
    println!("Executing: {}", main_cmd);
    let _ = Command::new("sh").arg("-c").arg(&main_cmd).status();

    if let Some(ref post) = config.wallpaper_command_post {
        let post_cmd = post.replace("[[w]]", wall_path);
        println!("Executing post command: {}", post_cmd);
        let _ = Command::new("sh").arg("-c").arg(&post_cmd).spawn();
    }
}

fn save_last_wallpaper(path: &str, command_name: Option<&str>) {
    let Some(cache_dir) = anomale_cache_dir() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        eprintln!("Failed to create cache dir {:?}: {}", cache_dir, e);
        return;
    }
    let last_file = cache_dir.join("last.txt");
    if let Err(e) = std::fs::write(&last_file, path) {
        eprintln!("Failed to write last wallpaper to {:?}: {}", last_file, e);
    }
    let last_cmd_file = cache_dir.join("last_command.txt");
    let cmd_name = command_name.unwrap_or("");
    if let Err(e) = std::fs::write(&last_cmd_file, cmd_name) {
        eprintln!(
            "Failed to write last wallpaper command to {:?}: {}",
            last_cmd_file, e
        );
    }
}

fn read_last_command_name() -> Option<String> {
    let cache_dir = anomale_cache_dir()?;
    let content = std::fs::read_to_string(cache_dir.join("last_command.txt")).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn effective_command_name(config: &AppConfig, command_name: Option<&str>) -> Option<String> {
    if config.has_named_wallpaper_commands() {
        Some(
            command_name
                .map(|s| s.to_string())
                .or_else(|| config.default_wallpaper_command_name())
                .unwrap_or_default(),
        )
    } else {
        None
    }
}

fn apply_wallpaper_selection(
    wall_path: &str,
    config: &AppConfig,
    command_name: Option<&str>,
    window: Option<&ApplicationWindow>,
) {
    let cmd_name = effective_command_name(config, command_name);
    save_last_wallpaper(wall_path, cmd_name.as_deref());

    let path = wall_path.to_string();
    let config = config.clone();
    let resolved_name = cmd_name;

    std::thread::spawn(move || {
        run_wallpaper_commands(&path, &config, resolved_name.as_deref());
    });

    if let Some(win) = window {
        win.set_visible(false);
    }
}

/// If use_last_wall is enabled, read last.txt and run the wallpaper command.
pub fn apply_last_wallpaper(config: &AppConfig) {
    if !config.use_last_wall {
        return;
    }
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let last_file = PathBuf::from(&home).join(".cache/anomale/last.txt");
    let wall_path = match std::fs::read_to_string(&last_file) {
        Ok(p) => p.trim().to_string(),
        Err(_) => return,
    };
    if wall_path.is_empty() {
        return;
    }

    let command_name = read_last_command_name();
    run_wallpaper_commands(&wall_path, config, command_name.as_deref());
}

fn wallpaper_display_name(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandViewEntry {
    Shift,
    RightClick,
}

pub struct WallpaperMenu {
    pub window: ApplicationWindow,
    stack: gtk4::Stack,
    title_label: Label,
    back_button: Button,
    command_list: gtk4::Box,
    command_scrolled: gtk4::ScrolledWindow,
    content_box: gtk4::Box,
    css_provider: gtk4::CssProvider,
    config: AppConfig,
    frames: RefCell<Vec<gtk4::Box>>,
    paths: RefCell<Vec<String>>,
    command_buttons: RefCell<Vec<Button>>,
    cmd_menu_open: RefCell<Option<usize>>,
    cmd_view_entry: RefCell<Option<CommandViewEntry>>,
    cmd_menu_option: RefCell<i32>,
    selected: RefCell<i32>,
    cols: RefCell<i32>,
    load_version: Arc<AtomicU64>,
}

enum LoadMessage {
    Image {
        path: PathBuf,
        bytes: gtk4::glib::Bytes,
        colorspace: gtk4::gdk_pixbuf::Colorspace,
        has_alpha: bool,
        bits_per_sample: i32,
        width: i32,
        height: i32,
        rowstride: i32,
    },
    Done,
}

impl WallpaperMenu {
    pub fn new(app: &Application, css_provider_ref: &gtk4::CssProvider) -> Rc<RefCell<Self>> {
        let config = AppConfig::load().unwrap_or_else(|e| {
            eprintln!("Failed to load menus config: {}. Using defaults.", e);
            AppConfig::default()
        });

        let window = ApplicationWindow::builder()
            .application(app)
            .title("Anomale Wallpaper Selector")
            .decorated(false)
            .visible(false)
            .build();

        prepare_popup_window(
            &window,
            PopupOptions::sized(config.wallpapers_width, config.wallpapers_height),
        );
        window.add_css_class("wallpaper-popup");

        let css = config.generate_css();
        css_provider_ref.load_from_data(&css);
        let css_provider = css_provider_ref.clone();

        let outer_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .vexpand(true)
            .hexpand(true)
            .halign(Align::Fill)
            .valign(Align::Fill)
            .build();
        outer_box.add_css_class("wallpaper-window");

        let header = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(0)
            .margin_bottom(10)
            .build();

        let back_button = Button::builder()
            .label("←")
            .has_frame(false)
            .halign(Align::Start)
            .valign(Align::Center)
            .visible(false)
            .build();
        back_button.add_css_class("wallpaper-cmd-back");

        let title_label = Label::new(Some("Choose Your Wallpaper"));
        title_label.add_css_class("launcher-box");
        title_label.set_hexpand(true);
        title_label.set_halign(Align::Center);

        header.append(&back_button);
        header.append(&title_label);
        outer_box.append(&header);

        let stack = gtk4::Stack::builder()
            .vexpand(true)
            .hexpand(true)
            .build();

        let grid_page = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .vexpand(true)
            .hexpand(true)
            .build();

        let scrolled_window = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .build();

        let content_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .halign(Align::Fill)
            .hexpand(true)
            .valign(Align::Start)
            .spacing(20)
            .build();

        scrolled_window.set_child(Some(&content_box));
        grid_page.append(&scrolled_window);
        stack.add_named(&grid_page, Some("grid"));

        let command_page = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .vexpand(true)
            .hexpand(true)
            .build();

        let command_scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .build();

        let command_list = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .halign(Align::Fill)
            .hexpand(true)
            .valign(Align::Start)
            .build();
        command_list.add_css_class("wallpaper-cmd-view");

        command_scrolled.set_child(Some(&command_list));
        command_page.append(&command_scrolled);
        stack.add_named(&command_page, Some("command"));

        stack.set_visible_child_name("grid");
        outer_box.append(&stack);
        window.set_child(Some(&outer_box));

        let menu = Rc::new(RefCell::new(Self {
            window: window.clone(),
            stack,
            title_label: title_label.clone(),
            back_button: back_button.clone(),
            command_list,
            command_scrolled,
            content_box,
            css_provider,
            config,
            frames: RefCell::new(Vec::new()),
            paths: RefCell::new(Vec::new()),
            command_buttons: RefCell::new(Vec::new()),
            cmd_menu_open: RefCell::new(None),
            cmd_view_entry: RefCell::new(None),
            cmd_menu_option: RefCell::new(-1),
            selected: RefCell::new(-1),
            cols: RefCell::new(1),
            load_version: Arc::new(AtomicU64::new(0)),
        }));

        let menu_clone_back = menu.clone();
        back_button.connect_clicked(move |_| {
            menu_clone_back.borrow().close_command_view();
        });

        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let menu_clone_key = menu.clone();

        key_controller.connect_key_pressed(move |_, key, _, state| {
            let m = menu_clone_key.borrow();
            let total = m.frames.borrow().len() as i32;

            if total == 0 {
                if key == gtk4::gdk::Key::Escape {
                    m.window.set_visible(false);
                    return gtk4::glib::Propagation::Stop;
                }
                return gtk4::glib::Propagation::Proceed;
            }

            let cols = *m.cols.borrow();
            let mut sel = *m.selected.borrow();
            let shift = state.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
            let open_panel = *m.cmd_menu_open.borrow();

            match key {
                k if k == gtk4::gdk::Key::Escape => {
                    if open_panel.is_some() {
                        m.close_command_view();
                        return gtk4::glib::Propagation::Stop;
                    }
                    m.window.set_visible(false);
                    return gtk4::glib::Propagation::Stop;
                }
                k if k == gtk4::gdk::Key::Return || k == gtk4::gdk::Key::KP_Enter => {
                    if let Some(_panel_idx) = open_panel {
                        let opt = *m.cmd_menu_option.borrow();
                        if opt >= 0 {
                            m.activate_command_option(opt as usize);
                        }
                    } else if sel >= 0 && sel < total {
                        if shift && m.config.has_named_wallpaper_commands() {
                            m.open_command_view(sel as usize, CommandViewEntry::Shift);
                        } else {
                            let path = m.paths.borrow()[sel as usize].clone();
                            let cfg = m.config.clone();
                            let win = m.window.clone();
                            apply_wallpaper_selection(&path, &cfg, None, Some(&win));
                        }
                    }
                    return gtk4::glib::Propagation::Stop;
                }
                _ => {}
            }

            if open_panel.is_some() {
                match key {
                    k if k == gtk4::gdk::Key::Down => {
                        m.navigate_command_option(1);
                        return gtk4::glib::Propagation::Stop;
                    }
                    k if k == gtk4::gdk::Key::Up => {
                        m.navigate_command_option(-1);
                        return gtk4::glib::Propagation::Stop;
                    }
                    _ => return gtk4::glib::Propagation::Proceed,
                }
            }

            match key {
                k if k == gtk4::gdk::Key::Right => {
                    sel = if sel < 0 { 0 } else { (sel + 1).min(total - 1) };
                }
                k if k == gtk4::gdk::Key::Left => {
                    sel = if sel < 0 { 0 } else { (sel - 1).max(0) };
                }
                k if k == gtk4::gdk::Key::Down => {
                    sel = if sel < 0 { 0 } else { (sel + cols).min(total - 1) };
                }
                k if k == gtk4::gdk::Key::Up => {
                    sel = if sel < 0 { 0 } else { (sel - cols).max(0) };
                }
                _ => return gtk4::glib::Propagation::Proceed,
            }

            m.update_selection(sel);
            m.scroll_to_selected(sel);

            gtk4::glib::Propagation::Stop
        });

        let menu_clone_key_release = menu.clone();
        key_controller.connect_key_released(move |_, key, _, _| {
            let m = menu_clone_key_release.borrow();
            if m.cmd_menu_open.borrow().is_none() {
                return;
            }
            if *m.cmd_view_entry.borrow() != Some(CommandViewEntry::Shift) {
                return;
            }

            match key {
                k if k == gtk4::gdk::Key::Shift_L || k == gtk4::gdk::Key::Shift_R => {
                    m.close_command_view();
                }
                _ => {}
            }
        });

        menu.borrow().window.add_controller(key_controller);

        menu
    }

    fn update_selection(&self, new_sel: i32) {
        let frames = self.frames.borrow();
        let old_sel = *self.selected.borrow();

        if old_sel >= 0 && (old_sel as usize) < frames.len() {
            frames[old_sel as usize].remove_css_class("selected");
        }

        if new_sel >= 0 && (new_sel as usize) < frames.len() {
            let frame = &frames[new_sel as usize];
            frame.add_css_class("selected");
            frame.grab_focus();
        }

        *self.selected.borrow_mut() = new_sel;
    }

    fn scroll_to_selected(&self, sel: i32) {
        let frames = self.frames.borrow();
        if sel < 0 || (sel as usize) >= frames.len() {
            return;
        }
        let frame = &frames[sel as usize];
        if let Some(parent) = self.content_box.parent() {
            if let Some(scrolled) = parent.downcast_ref::<gtk4::ScrolledWindow>() {
                let adj = scrolled.vadjustment();
                let alloc = frame.allocation();
                let content_alloc = self.content_box.allocation();

                let frame_y = alloc.y() as f64 - content_alloc.y() as f64;
                let frame_h = alloc.height() as f64;

                let view_y = adj.value();
                let page_size = adj.page_size();

                if frame_y < view_y {
                    adj.set_value(frame_y);
                } else if frame_y + frame_h > view_y + page_size {
                    adj.set_value(frame_y + frame_h - page_size);
                }
            }
        }
    }

    fn close_command_view(&self) {
        self.title_label.set_text("Choose Your Wallpaper");
        self.back_button.set_visible(false);
        self.stack.set_visible_child_name("grid");

        for btn in self.command_buttons.borrow().iter() {
            btn.remove_css_class("selected");
        }
        *self.cmd_menu_open.borrow_mut() = None;
        *self.cmd_view_entry.borrow_mut() = None;
        *self.cmd_menu_option.borrow_mut() = -1;

        let sel = *self.selected.borrow();
        let frames = self.frames.borrow();
        if sel >= 0 && (sel as usize) < frames.len() {
            frames[sel as usize].grab_focus();
        } else {
            self.window.grab_focus();
        }
    }

    fn open_command_view(&self, index: usize, entry: CommandViewEntry) {
        if !self.config.has_named_wallpaper_commands() {
            return;
        }
        let path = match self.paths.borrow().get(index) {
            Some(path) => path.clone(),
            None => return,
        };

        while let Some(child) = self.command_list.first_child() {
            self.command_list.remove(&child);
        }
        self.command_buttons.borrow_mut().clear();

        for (name, _) in &self.config.wallpaper_commands {
            let btn = Button::builder()
                .label(name)
                .has_frame(false)
                .halign(Align::Fill)
                .hexpand(true)
                .build();
            btn.add_css_class("wallpaper-cmd-option");

            let wall_path = path.clone();
            let cmd_name = name.clone();
            let cfg = self.config.clone();
            let win = self.window.clone();

            btn.connect_clicked(move |_| {
                apply_wallpaper_selection(&wall_path, &cfg, Some(&cmd_name), Some(&win));
            });
            self.command_list.append(&btn);
            self.command_buttons.borrow_mut().push(btn);
        }

        let display_name = wallpaper_display_name(&path);
        self.title_label
            .set_text(&format!("Choose command — {display_name}"));
        self.back_button
            .set_visible(entry == CommandViewEntry::RightClick);
        self.command_scrolled.vadjustment().set_value(0.0);
        self.stack.set_visible_child_name("command");
        *self.cmd_menu_open.borrow_mut() = Some(index);
        *self.cmd_view_entry.borrow_mut() = Some(entry);
        self.update_command_option_highlight(0);
        self.window.grab_focus();
    }

    fn scroll_command_option_into_view(&self, option_index: i32) {
        if option_index < 0 {
            return;
        }
        let buttons = self.command_buttons.borrow();
        let Some(btn) = buttons.get(option_index as usize) else {
            return;
        };

        let Some((_, btn_y)) = btn.translate_coordinates(&self.command_list, 0.0, 0.0) else {
            return;
        };
        let btn_h = btn.height() as f64;
        let adj = self.command_scrolled.vadjustment();
        let page = adj.page_size();
        let value = adj.value();

        if btn_y < value {
            adj.set_value(btn_y);
        } else if btn_y + btn_h > value + page {
            adj.set_value(btn_y + btn_h - page);
        }
    }

    fn update_command_option_highlight(&self, option_index: i32) {
        for (i, btn) in self.command_buttons.borrow().iter().enumerate() {
            if i as i32 == option_index {
                btn.add_css_class("selected");
            } else {
                btn.remove_css_class("selected");
            }
        }
        *self.cmd_menu_option.borrow_mut() = option_index;
        self.scroll_command_option_into_view(option_index);
    }

    fn navigate_command_option(&self, delta: i32) {
        let count = self.command_buttons.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = *self.cmd_menu_option.borrow();
        let next = if current < 0 {
            0
        } else {
            (current + delta).clamp(0, count - 1)
        };
        self.update_command_option_highlight(next);
    }

    fn activate_command_option(&self, option_index: usize) {
        if let Some(btn) = self.command_buttons.borrow().get(option_index) {
            btn.activate();
        }
    }

    pub fn toggle(self_rc: &Rc<RefCell<Self>>) {
        let mut m = self_rc.borrow_mut();
        if m.window.is_visible() {
            m.close_command_view();
            m.window.set_visible(false);
        } else {
            let config = AppConfig::load().unwrap_or_default();
            m.config = config.clone();

            m.css_provider
                .load_from_data(&config.generate_css());

            present_popup(&m.window);
            m.populate_wallpapers(self_rc);
        }
    }

    fn populate_wallpapers(&mut self, self_rc: &Rc<RefCell<Self>>) {
        let cur_version = self.load_version.fetch_add(1, Ordering::SeqCst) + 1;

        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }
        self.frames.borrow_mut().clear();
        self.paths.borrow_mut().clear();
        while let Some(child) = self.command_list.first_child() {
            self.command_list.remove(&child);
        }
        self.command_buttons.borrow_mut().clear();
        self.close_command_view();
        *self.selected.borrow_mut() = -1;

        let scan_label = Label::new(Some("Scanning directory..."));
        scan_label.set_halign(Align::Center);
        scan_label.set_margin_top(40);
        self.content_box.append(&scan_label);

        let path_str = if self.config.wallpapers_path.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                self.config.wallpapers_path.replacen("~", &home, 1)
            } else {
                self.config.wallpapers_path.clone()
            }
        } else {
            self.config.wallpapers_path.clone()
        };

        let wall_dir = PathBuf::from(&path_str);
        let thumb_size = self.config.wallpapers_thumb_size;

        if !wall_dir.exists() || !wall_dir.is_dir() {
            scan_label.set_text("Wallpapers directory does not exist or is not a directory.");
            return;
        }

        let (tx, rx) = async_channel::unbounded();

        let version_check = self.load_version.clone();
        std::thread::spawn(move || {
            let mut entries: Vec<PathBuf> = Vec::new();
            if let Ok(dir) = std::fs::read_dir(&wall_dir) {
                for entry in dir.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension() {
                            let ext_lower = ext.to_string_lossy().to_lowercase();
                            if [
                                "jpg", "jpeg", "png", "webp", "bmp", "gif", "tiff", "tif",
                            ]
                            .contains(&ext_lower.as_str())
                            {
                                entries.push(path);
                            }
                        }
                    }
                }
            }
            entries.sort();

            for wall_path in entries {
                if version_check.load(Ordering::SeqCst) != cur_version {
                    return;
                }

                if let Ok(pixbuf) =
                    Pixbuf::from_file_at_scale(&wall_path, thumb_size, thumb_size, true)
                {
                    let _ = tx.send_blocking(LoadMessage::Image {
                        path: wall_path,
                        bytes: pixbuf.read_pixel_bytes(),
                        colorspace: pixbuf.colorspace(),
                        has_alpha: pixbuf.has_alpha(),
                        bits_per_sample: pixbuf.bits_per_sample(),
                        width: pixbuf.width(),
                        height: pixbuf.height(),
                        rowstride: pixbuf.rowstride(),
                    });
                }
            }

            if version_check.load(Ordering::SeqCst) == cur_version {
                let _ = tx.send_blocking(LoadMessage::Done);
            }
        });

        let flow_box = gtk4::FlowBox::builder()
            .homogeneous(true)
            .row_spacing(10)
            .column_spacing(10)
            .halign(Align::Center)
            .hexpand(false)
            .valign(Align::Start)
            .selection_mode(gtk4::SelectionMode::None)
            .max_children_per_line(50)
            .min_children_per_line(1)
            .build();

        let menu_clone = Rc::downgrade(self_rc);
        let thumb_size_fixed = thumb_size;
        let version_check_rx = self.load_version.clone();

        gtk4::glib::idle_add_local({
            let version_check_rx_idle = version_check_rx.clone();
            let rx_idle = rx.clone();
            let menu_clone_idle = menu_clone.clone();
            let scan_label_idle = scan_label.clone();
            let flow_box_idle = flow_box.clone();

            let mut first = true;
            let mut count = 0;

            move || {
                if version_check_rx_idle.load(Ordering::SeqCst) != cur_version {
                    return gtk4::glib::ControlFlow::Break;
                }

                let m_rc = match menu_clone_idle.upgrade() {
                    Some(rc) => rc,
                    None => return gtk4::glib::ControlFlow::Break,
                };

                for _ in 0..3 {
                    match rx_idle.try_recv() {
                        Ok(LoadMessage::Image {
                            path,
                            bytes,
                            colorspace,
                            has_alpha,
                            bits_per_sample,
                            width,
                            height,
                            rowstride,
                        }) => {
                            let m = m_rc.borrow();
                            let scaled = gtk4::gdk_pixbuf::Pixbuf::from_bytes(
                                &bytes,
                                colorspace,
                                has_alpha,
                                bits_per_sample,
                                width,
                                height,
                                rowstride,
                            );

                            if first {
                                m.content_box.remove(&scan_label_idle);
                                m.content_box.append(&flow_box_idle);
                                first = false;
                            }

                            let texture = gtk4::gdk::Texture::for_pixbuf(&scaled);
                            let picture = gtk4::Picture::for_paintable(&texture);
                            picture.set_can_shrink(true);
                            picture.set_size_request(thumb_size_fixed, thumb_size_fixed);

                            let path_str = path.to_string_lossy().to_string();
                            let cfg = m.config.clone();
                            let win = m.window.clone();
                            let thumb_index = count;

                            let frame = gtk4::Box::builder()
                                .orientation(Orientation::Vertical)
                                .spacing(0)
                                .halign(Align::Center)
                                .valign(Align::Center)
                                .focusable(true)
                                .build();
                            frame.add_css_class("wallpaper-thumb");
                            frame.append(&picture);

                            if cfg.has_named_wallpaper_commands() {
                                let menu_weak = Rc::downgrade(&m_rc);
                                let idx = thumb_index;
                                let right_click = gtk4::GestureClick::new();
                                right_click.set_button(3);
                                right_click.connect_pressed(move |gesture, _, _, _| {
                                    if gesture.current_button()
                                        == gtk4::gdk::BUTTON_SECONDARY
                                    {
                                        if let Some(rc) = menu_weak.upgrade() {
                                            let m = rc.borrow();
                                            m.update_selection(idx as i32);
                                            m.open_command_view(idx, CommandViewEntry::RightClick);
                                        }
                                    }
                                });
                                picture.add_controller(right_click);
                            }

                            let path_for_click = path_str.clone();
                            let cfg_for_click = cfg.clone();
                            let win_for_click = win.clone();
                            let click = gtk4::GestureClick::new();
                            click.connect_released(move |gesture, _, _, _| {
                                if gesture.current_button() == gtk4::gdk::BUTTON_PRIMARY {
                                    apply_wallpaper_selection(
                                        &path_for_click,
                                        &cfg_for_click,
                                        None,
                                        Some(&win_for_click),
                                    );
                                }
                            });
                            picture.add_controller(click);

                            flow_box_idle.insert(&frame, -1);
                            m.frames.borrow_mut().push(frame);
                            m.paths.borrow_mut().push(path_str);

                            count += 1;

                            let content_width = m.content_box.width();
                            if content_width > 0 {
                                let cols = ((content_width as f64)
                                    / (thumb_size_fixed as f64 + 10.0))
                                    .floor() as i32;
                                *m.cols.borrow_mut() = cols.max(1);
                            }
                        }
                        Ok(LoadMessage::Done) => {
                            if first && count == 0 {
                                scan_label_idle.set_text("No wallpapers found.");
                            }
                            return gtk4::glib::ControlFlow::Break;
                        }
                        Err(async_channel::TryRecvError::Empty) => break,
                        Err(async_channel::TryRecvError::Closed) => {
                            return gtk4::glib::ControlFlow::Break;
                        }
                    }
                }

                gtk4::glib::ControlFlow::Continue
            }
        });
    }
}
