use gtk4::prelude::*;
use gtk4::{Application, gio};
use clap::Parser;

mod bar;
mod config;
mod layout;
mod modules;
mod watcher;
mod apps;
mod action_menu;
mod wallpapers;
mod notify;
mod notify_server;
mod notification_window;
mod tray;

use config::Config;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashSet;
use gtk4::gdk::Monitor;
use gtk4::ApplicationWindow;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Full reload - restarts the application
    #[arg(long)]
    reload: bool,
    
    /// Quick refresh - CSS-only update without restart
    #[arg(long)]
    refresh: bool,
    
    /// Refresh CSS and run exec commands from active configs
    #[arg(long)]
    refreshexec: bool,

    /// Toggle App Launcher
    #[arg(long)]
    apps: bool,

    /// Toggle Power Menu
    #[arg(long)]
    power: bool,

    /// Toggle System Tray
    #[arg(long)]
    tray: bool,

    /// Toggle a named action menu defined in menus.conf
    #[arg(long, value_name = "NAME")]
    menu: Option<String>,

    /// Toggle Wallpaper Selector
    #[arg(long)]
    wallpapers: bool,

    /// Enable, disable, or toggle notification popups (on|off|toggle)
    #[arg(long, value_name = "STATE")]
    notif: Option<String>,
}

fn run_command(cmd: &str) {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .spawn()
        .ok();
}

fn spawn_restart_process() {
    // Get the path to current executable
    if let Ok(exe_path) = std::env::current_exe() {
        // Spawn new instance with delay via shell to avoid DBus race
        let exe_str = exe_path.to_string_lossy().to_string();
        println!("Spawning new instance: {}", exe_str);
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sleep 0.5 && {}", exe_str))
            .spawn()
            .ok();
    }
}

fn restart_app(app: &Application) {
    println!("Restarting...");
    spawn_restart_process();
    // Quit current instance
    app.quit();
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location();
        let file = location.map(|l| l.file()).unwrap_or("<unknown>");
        let line = location.map(|l| l.line()).unwrap_or(0);

        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => *s,
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => &s[..],
                None => "Box<Any>",
            },
        };

        eprintln!(
            "CRASH DETECTED at {}:{}: {}",
            file,
            line,
            msg
        );
        
        // Spawn new process
        spawn_restart_process();
    }));
}

fn refresh_menu_css(provider: &gtk4::CssProvider) {
    let menus_config = config::AppConfig::load().unwrap_or_default();
    provider.load_from_data(&menus_config.generate_css(None));
}

fn refresh_css(
    bars: &Rc<RefCell<Vec<(Option<String>, ApplicationWindow, gtk4::CssProvider)>>>,
    menu_provider: &Option<gtk4::CssProvider>,
) {
    println!("Refreshing CSS...");
    let bars = bars.borrow();
    
    for (monitor_name, _window, provider) in bars.iter() {
        let config = Config::load(monitor_name.as_deref()).unwrap_or_else(|_| {
            Config::default()
        });
        let css = bar::generate_css(&config, monitor_name.as_deref());
        provider.load_from_data(&css);
    }

    if let Some(mp) = menu_provider {
        refresh_menu_css(mp);
    }
}

fn refresh_with_exec(
    bars: &Rc<RefCell<Vec<(Option<String>, ApplicationWindow, gtk4::CssProvider)>>>,
    menu_provider: &Option<gtk4::CssProvider>,
) {
    println!("Refreshing CSS and executing commands...");
    let bars = bars.borrow();
    let mut exec_commands = HashSet::new();
    
    for (monitor_name, _window, provider) in bars.iter() {
        let config = Config::load(monitor_name.as_deref()).unwrap_or_else(|_| {
            Config::default()
        });
        let css = bar::generate_css(&config, monitor_name.as_deref());
        provider.load_from_data(&css);
        
        for cmd in &config.exec {
            exec_commands.insert(cmd.clone());
        }
    }

    if let Some(mp) = menu_provider {
        refresh_menu_css(mp);
    }
    
    for cmd in exec_commands {
        run_command(&cmd);
    }
}

fn toggle_action_menu(
    app: &Application,
    registry: &Rc<RefCell<action_menu::ActionMenuRegistry>>,
    pending: &Rc<RefCell<Option<String>>>,
    menu_provider: &Rc<RefCell<Option<gtk4::CssProvider>>>,
    menu_id: &str,
) {
    if let Some(provider) = menu_provider.borrow().as_ref() {
        registry.borrow().toggle(app, provider, menu_id);
    } else {
        *pending.borrow_mut() = Some(menu_id.to_string());
    }
}

fn apply_notif_state(manager: &Rc<notify::NotifyManager>, state: &str) {
    match state.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "enable" | "enabled" => manager.set_enabled(true),
        "off" | "false" | "0" | "disable" | "disabled" => manager.set_enabled(false),
        "toggle" => manager.toggle_enabled(),
        other => eprintln!(
            "Unknown --notif state '{}'. Use on, off, or toggle.",
            other
        ),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_panic_hook();

    let app = Application::builder()
        .application_id("com.jor.anomale")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let bars: Rc<RefCell<Vec<(Option<String>, ApplicationWindow, gtk4::CssProvider)>>> = Rc::new(RefCell::new(Vec::new()));
    let menu_css_provider_store: Rc<RefCell<Option<gtk4::CssProvider>>> = Rc::new(RefCell::new(None));
    
    // We need to keep track if we have initialized the bars/watcher to avoid double init
    // because activate might be called after command-line
    let is_initialized = Rc::new(RefCell::new(false));

    // Launcher state
    let app_launcher_store: Rc<RefCell<Option<Rc<RefCell<apps::AppLauncher>>>>> = Rc::new(RefCell::new(None));
    let should_launch_apps_store = Rc::new(RefCell::new(false));

    let action_menu_registry: Rc<RefCell<action_menu::ActionMenuRegistry>> =
        Rc::new(RefCell::new(action_menu::ActionMenuRegistry::new()));
    let pending_action_menu_store: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let wallpaper_menu_store: Rc<RefCell<Option<Rc<RefCell<wallpapers::WallpaperMenu>>>>> = Rc::new(RefCell::new(None));
    let should_show_wallpapers_store = Rc::new(RefCell::new(false));

    let tray_menu_store: Rc<RefCell<Option<Rc<RefCell<tray::TrayMenu>>>>> =
        Rc::new(RefCell::new(None));
    let should_show_tray_store = Rc::new(RefCell::new(false));

    let notify_manager_store: Rc<RefCell<Option<Rc<notify::NotifyManager>>>> = Rc::new(RefCell::new(None));
    let pending_notif_store: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let bars_clone_activate = bars.clone();
    let is_initialized_clone_activate = is_initialized.clone();
    let app_launcher_store_activate = app_launcher_store.clone();
    let should_launch_apps_store_activate = should_launch_apps_store.clone();
    let action_menu_registry_activate = action_menu_registry.clone();
    let pending_action_menu_store_activate = pending_action_menu_store.clone();
    let wallpaper_menu_store_activate = wallpaper_menu_store.clone();
    let should_show_wallpapers_store_activate = should_show_wallpapers_store.clone();
    let tray_menu_store_activate = tray_menu_store.clone();
    let should_show_tray_store_activate = should_show_tray_store.clone();
    let menu_css_provider_store_activate = menu_css_provider_store.clone();
    let notify_manager_store_activate = notify_manager_store.clone();
    let pending_notif_store_activate = pending_notif_store.clone();

    app.connect_activate(move |app| {
        if *is_initialized_clone_activate.borrow() {
            return;
        }
        *is_initialized_clone_activate.borrow_mut() = true;

        // Apply last wallpaper FIRST (synchronous) so pywal colors are ready before UI loads
        {
            let menus_config = config::AppConfig::load().unwrap_or_default();
            wallpapers::apply_last_wallpaper(&menus_config);
        }

        let display = gtk4::gdk::Display::default().expect("Could not get default display");
        let monitors = display.monitors();

        let mut exec_commands = HashSet::new();
        let mut exec_once_commands = HashSet::new();

        // Debug: List config directory
        if let Ok(config_path) = Config::get_config_path(None) {
            if let Some(parent) = config_path.parent() {
                println!("DEBUG: Listing config directory: {:?}", parent);
                if let Ok(entries) = std::fs::read_dir(parent) {
                    for entry in entries {
                        if let Ok(entry) = entry {
                            println!("DEBUG: Found file: {:?}", entry.path());
                        }
                    }
                }
            }
        }

        // Iterate over monitors and create a bar for each
        for i in 0..monitors.n_items() {
            if let Some(monitor) = monitors.item(i).and_downcast::<Monitor>() {
                if let Some(monitor_name) = monitor.connector() {
                    println!("DEBUG: Processing monitor: {}", monitor_name);
                    let config = Config::load(Some(monitor_name.as_str())).unwrap_or_else(|e| {
                        eprintln!("Warning: Failed to load config for monitor {:?}: {}. Using defaults.", monitor_name, e);
                        Config::default()
                    });
                    println!("DEBUG: Loaded config for {} with bar_height: {}", monitor_name, config.bar_height);

                    // Collect commands
                    for cmd in &config.exec {
                        exec_commands.insert(cmd.clone());
                    }
                    for cmd in &config.exec_once {
                        exec_once_commands.insert(cmd.clone());
                    }

                     let (window, provider) = bar::create_bar(app, &monitor, &config);
                    
                    bars_clone_activate.borrow_mut().push((Some(monitor_name.into()), window, provider));
                }
            }
        }
        
        // Run startup commands
        // Deduplicated by HashSet
        for cmd in exec_once_commands {
            run_command(&cmd);
        }
        for cmd in exec_commands {
            run_command(&cmd);
        }
        
        // Setup watcher
        let (sender, receiver) = async_channel::unbounded();
        
        let mut watch_paths = Vec::new();
        // Watch config directory
        if let Ok(config_path) = Config::get_config_path(None) {
            if let Some(parent) = config_path.parent() {
                watch_paths.push(parent.to_path_buf());
            }
        }
        
        // Watch pywal colors
        if let Ok(home) = std::env::var("HOME") {
            let pywal_path = std::path::PathBuf::from(home).join(".cache/wal/colors.json");
             if let Some(parent) = pywal_path.parent() {
                 watch_paths.push(parent.to_path_buf());
             }
        }
        
        watcher::spawn_watcher(watch_paths, sender);
        
        let bars_clone = bars_clone_activate.clone();
        let menu_provider_clone = menu_css_provider_store_activate.clone();
        gtk4::glib::MainContext::default().spawn_local(async move {
            while let Ok(_) = receiver.recv().await {
                refresh_css(&bars_clone, &menu_provider_clone.borrow());
            }
        });

        // Create shared CSS provider for menus
        let menu_css_provider = gtk4::CssProvider::new();
        // Load initial CSS
        let menus_config = config::AppConfig::load().unwrap_or_default();
        menu_css_provider.load_from_data(&menus_config.generate_css(None));
        
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("Could not get default display"),
            &menu_css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );

        // Store for later refresh
        *menu_css_provider_store_activate.borrow_mut() = Some(menu_css_provider.clone());

        // Initialize Launcher
        if app_launcher_store_activate.borrow().is_none() {
             let launcher = apps::AppLauncher::new(app, &menu_css_provider);
             *app_launcher_store_activate.borrow_mut() = Some(launcher);
        }

        // Check if we need to auto-launch from command line args (first run)
        if *should_launch_apps_store_activate.borrow() {
             if let Some(launcher) = app_launcher_store_activate.borrow().as_ref() {
                 launcher.borrow().toggle();
             }
             *should_launch_apps_store_activate.borrow_mut() = false;
        }

        // Toggle action menu requested before GTK init finished
        if let Some(menu_id) = pending_action_menu_store_activate.borrow_mut().take() {
            action_menu_registry_activate
                .borrow()
                .toggle(app, &menu_css_provider, &menu_id);
        }

        // Initialize Wallpaper Menu
        if wallpaper_menu_store_activate.borrow().is_none() {
             let menu = wallpapers::WallpaperMenu::new(app, &menu_css_provider);
             *wallpaper_menu_store_activate.borrow_mut() = Some(menu);
        }

        if *should_show_wallpapers_store_activate.borrow() {
             if let Some(menu) = wallpaper_menu_store_activate.borrow().as_ref() {
                         wallpapers::WallpaperMenu::toggle(menu);
             }
             *should_show_wallpapers_store_activate.borrow_mut() = false;
        }

        // Initialize the StatusNotifier watcher/host and tray menu.
        if tray_menu_store_activate.borrow().is_none() {
            let (tray_updates_tx, tray_updates_rx) = async_channel::unbounded();
            let (tray_commands_tx, tray_commands_rx) = async_channel::unbounded();
            let tray_menu = tray::TrayMenu::new(app, &menu_css_provider, tray_commands_tx);
            *tray_menu_store_activate.borrow_mut() = Some(tray_menu.clone());

            tokio::spawn(async move {
                if let Err(error) = tray::run(tray_updates_tx, tray_commands_rx).await {
                    eprintln!("System tray service stopped: {}", error);
                }
            });

            gtk4::glib::MainContext::default().spawn_local(async move {
                while let Ok(items) = tray_updates_rx.recv().await {
                    tray_menu.borrow().update(items);
                }
            });
        }

        if *should_show_tray_store_activate.borrow() {
            if let Some(menu) = tray_menu_store_activate.borrow().as_ref() {
                menu.borrow().toggle();
            }
            *should_show_tray_store_activate.borrow_mut() = false;
        }

        // Initialize Notification System
        let (dbus_to_gtk_tx, dbus_to_gtk_rx) = async_channel::unbounded();
        let (gtk_to_dbus_tx, gtk_to_dbus_rx) = async_channel::unbounded();
        
        let notify_manager = notify::NotifyManager::new(app, gtk_to_dbus_tx);
        *notify_manager_store_activate.borrow_mut() = Some(notify_manager.clone());

        if let Some(state) = pending_notif_store_activate.borrow_mut().take() {
            apply_notif_state(&notify_manager, &state);
        }

        let server = notify_server::NotificationServer {
            events_tx: dbus_to_gtk_tx,
        };

        // Spawn DBus server in tokio task
        tokio::spawn(async move {
            let conn_res = zbus::connection::Builder::session()
                .unwrap_or_else(|e| {
                    eprintln!("Failed to connect to session bus: {}", e);
                    panic!("DBus fail");
                })
                .name("org.freedesktop.Notifications")
                .expect("Failed to set DBus name (is another daemon running?)")
                .serve_at("/org/freedesktop/Notifications", server)
                .expect("Failed to serve notification object")
                .build()
                .await;

            if let Ok(conn) = conn_res {
                println!("Notification service registered");
                
                // Handle signals from GTK thread
                while let Ok(event) = gtk_to_dbus_rx.recv().await {
                    match event {
                        notify_server::NotifyEvent::ActionInvoked(id, key) => {
                            let _ = conn.emit_signal(
                                None::<&str>,
                                "/org/freedesktop/Notifications",
                                "org.freedesktop.Notifications",
                                "ActionInvoked",
                                &(id, key),
                            ).await;
                        }
                        notify_server::NotifyEvent::NotificationClosed(id, reason) => {
                            let _ = conn.emit_signal(
                                None::<&str>,
                                "/org/freedesktop/Notifications",
                                "org.freedesktop.Notifications",
                                "NotificationClosed",
                                &(id, reason),
                            ).await;
                        }
                        _ => {}
                    }
                }
            } else if let Err(e) = conn_res {
                eprintln!("Notification service error: {}", e);
            }
        });

        // Bridge DBus events to GTK thread
        let notify_manager_clone = notify_manager.clone();
        gtk4::glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = dbus_to_gtk_rx.recv().await {
                notify_manager_clone.handle_event(event);
            }
        });

    });

    let bars_clone_cmd = bars.clone();
    let app_launcher_store_cmd = app_launcher_store.clone();
    let should_launch_apps_store_cmd = should_launch_apps_store.clone();
    let action_menu_registry_cmd = action_menu_registry.clone();
    let pending_action_menu_store_cmd = pending_action_menu_store.clone();
    let wallpaper_menu_store_cmd = wallpaper_menu_store.clone();
    let should_show_wallpapers_store_cmd = should_show_wallpapers_store.clone();
    let tray_menu_store_cmd = tray_menu_store.clone();
    let should_show_tray_store_cmd = should_show_tray_store.clone();
    let menu_css_provider_store_cmd = menu_css_provider_store.clone();
    let notify_manager_store_cmd = notify_manager_store.clone();
    let pending_notif_store_cmd = pending_notif_store.clone();
    app.connect_command_line(move |app, cmdline| {
        let args = cmdline.arguments();
        // Parse arguments directly from OsString
        println!("DEBUG: Received command line args: {:?}", args);
        match Args::try_parse_from(&args) {
            Ok(parsed) => {
                println!("DEBUG: Parsed args: {:?}", parsed);
                if parsed.reload {
                     println!("DEBUG: Reload requested");
                     // Full restart
                     restart_app(app);
                } else if parsed.refreshexec {
                     println!("DEBUG: Refresh exec requested");
                     // Refresh CSS and run exec commands
                     refresh_with_exec(&bars_clone_cmd, &menu_css_provider_store_cmd.borrow());
                } else if parsed.refresh {
                     println!("DEBUG: Refresh requested");
                     // CSS-only refresh
                     refresh_css(&bars_clone_cmd, &menu_css_provider_store_cmd.borrow());
                } else if parsed.apps {
                     println!("DEBUG: Apps toggle requested");
                     if let Some(launcher) = app_launcher_store_cmd.borrow().as_ref() {
                         launcher.borrow().toggle();
                     } else {
                          println!("DEBUG: Apps launcher not initialized yet, marking for launch");
                          *should_launch_apps_store_cmd.borrow_mut() = true;
                      }
                } else if let Some(menu_name) = parsed.menu {
                     println!("DEBUG: Action menu toggle requested: {}", menu_name);
                     toggle_action_menu(
                         app,
                         &action_menu_registry_cmd,
                         &pending_action_menu_store_cmd,
                         &menu_css_provider_store_cmd,
                         &menu_name,
                     );
                } else if parsed.power {
                     println!("DEBUG: Power toggle requested");
                     toggle_action_menu(
                         app,
                         &action_menu_registry_cmd,
                         &pending_action_menu_store_cmd,
                         &menu_css_provider_store_cmd,
                         "power",
                     );
                } else if parsed.tray {
                     println!("DEBUG: System tray toggle requested");
                     if let Some(menu) = tray_menu_store_cmd.borrow().as_ref() {
                         menu.borrow().toggle();
                     } else {
                         println!("DEBUG: System tray not initialized yet, marking for launch");
                         *should_show_tray_store_cmd.borrow_mut() = true;
                     }
                } else if parsed.wallpapers {
                     println!("DEBUG: Wallpapers toggle requested");
                     if let Some(menu) = wallpaper_menu_store_cmd.borrow().as_ref() {
                                 wallpapers::WallpaperMenu::toggle(menu);
                     } else {
                         println!("DEBUG: Wallpaper menu not initialized yet, marking for launch");
                         *should_show_wallpapers_store_cmd.borrow_mut() = true;
                     }
                } else if let Some(state) = parsed.notif {
                     println!("DEBUG: Notification state requested: {}", state);
                     if let Some(manager) = notify_manager_store_cmd.borrow().as_ref() {
                         apply_notif_state(manager, &state);
                     } else {
                         println!("DEBUG: Notify manager not initialized yet, deferring");
                         *pending_notif_store_cmd.borrow_mut() = Some(state);
                     }
                }
            }
            Err(e) => {
                eprintln!("Error parsing args: {}", e);
            }
        }


        app.activate();
        
        0 // Return status code 0
    });

    app.run();

    Ok(())
}
