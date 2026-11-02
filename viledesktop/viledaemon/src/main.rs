mod anim;
mod apps;
mod layout;
mod menu;
mod niri;
mod notify;
mod sys;
mod tray;

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use calloop::channel::{self, Event as ChannelEvent};
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use log::{debug, info, warn};
use niri_ipc::{Window, Workspace};
use vile_protocol::{
    ensure_config_file, ensure_runtime_dir, load_config, load_wallpaper_state,
    sanitize_recipe_name, save_wallpaper_state, socket_path, write_json_line, BarToDaemon,
    CellStyle, Config, DaemonToBar, MeterStyle, NotificationSide, OverlayKind, OverlayMode,
    PointerEvent, WallpaperMode, WallpaperState,
};

use crate::anim::{FrozenLine, Phase, Region, SlideDir, Transition};
use crate::apps::{command_remainder, launch_app, spawn_terminal, AppsPrompt};
use crate::layout::{
    compose_line, is_full_overlay, should_anim_mode, span_at, workspace_id_at, ComposedLine,
    ModeKind, ModuleSpan,
};
use crate::menu::{spawn_menu_command, usable_items, MenuCenter, MenuSession};
use crate::niri::{apply_event, occupied_workspace_ids, should_yield_grab, workspaces_for, Niri};
use crate::notify::{
    match_window, spawn_server, InboxState, NotifyCenter, NotifyEvent, NotifyItem, ToastState,
};
use crate::sys::{set_volume_delta, SysSampler, SysSnapshot};
use crate::tray::{
    spawn_watcher, terminate_pid, wrap_index, TrayCenter, TrayEvent, TrayHandle, TrayItem,
    TraySession, TrayView,
};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config_path = ensure_config_file()?;
    let config = load_config()?;
    info!("config {}", config_path.display());

    let runtime_dir = ensure_runtime_dir()?;
    let sock = socket_path();
    bind_socket(&sock)?;

    let mut event_loop = EventLoop::<Daemon>::try_new().context("creating event loop")?;
    let handle = event_loop.handle();

    let listener =
        UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    listener.set_nonblocking(true)?;
    info!("listening on {}", sock.display());

    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, daemon| {
                daemon.accept_clients(listener);
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("inserting socket listener: {err}"))?;

    handle
        .insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            |_, _, daemon| {
                daemon.tick();
                TimeoutAction::ToDuration(Duration::from_secs(1))
            },
        )
        .map_err(|err| anyhow::anyhow!("inserting clock timer: {err}"))?;

    handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(200)),
            |_, _, daemon| {
                daemon.charge_tick();
                TimeoutAction::ToDuration(Duration::from_millis(200))
            },
        )
        .map_err(|err| anyhow::anyhow!("inserting charge timer: {err}"))?;

    let (colors_tx, colors_rx) = channel::channel();
    handle
        .insert_source(colors_rx, |event, _, daemon| {
            if let ChannelEvent::Msg(result) = event {
                daemon.on_colors_done(result);
            }
        })
        .map_err(|err| anyhow::anyhow!("inserting colors channel: {err}"))?;

    let (notify_tx, notify_rx) = channel::channel();
    handle
        .insert_source(notify_rx, |event, _, daemon| {
            if let ChannelEvent::Msg(event) = event {
                daemon.on_notify_event(event);
            }
        })
        .map_err(|err| anyhow::anyhow!("inserting notify channel: {err}"))?;
    if config.notifications.enabled {
        spawn_server(notify_tx);
    }

    let (tray_tx, tray_rx) = channel::channel();
    handle
        .insert_source(tray_rx, |event, _, daemon| {
            if let ChannelEvent::Msg(event) = event {
                daemon.on_tray_event(event);
            }
        })
        .map_err(|err| anyhow::anyhow!("inserting tray channel: {err}"))?;
    let tray_cmd = if config.tray.enabled {
        Some(spawn_watcher(tray_tx))
    } else {
        None
    };

    let mut daemon = Daemon {
        handle: handle.clone(),
        config,
        clients: HashMap::new(),
        next_id: 1,
        sampler: SysSampler::default(),
        sys: SysSnapshot::default(),
        niri: None,
        niri_token: None,
        workspaces: Vec::new(),
        windows: HashMap::new(),
        colors_tx,
        colors_busy: false,
        wallpaper: load_wallpaper_state(),
        wall_spawned_at: None,
        apps: None,
        notify: NotifyCenter::default(),
        tray: TrayCenter::default(),
        tray_cmd,
        menu: MenuCenter::default(),
        toast_timer: None,
        grab_window: None,
        grab_lost_window_focus: false,
        started: Instant::now(),
    };
    daemon.sys = daemon.sampler.sample();
    daemon.connect_niri();

    info!("viledaemon ready");
    event_loop
        .run(None, &mut daemon, |_| {})
        .context("event loop")?;
    let _ = runtime_dir;
    Ok(())
}

fn bind_socket(path: &Path) -> Result<()> {
    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                anyhow::bail!(
                    "another viledaemon is already running at {}",
                    path.display()
                );
            }
            Err(_) => {
                std::fs::remove_file(path)
                    .with_context(|| format!("removing stale socket {}", path.display()))?;
            }
        }
    }
    Ok(())
}

fn find_sibling(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(name);
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from(name)
}

fn find_vilecolors() -> PathBuf {
    find_sibling("vilecolors")
}

fn resolve_image_path(image: &str) -> Result<String, String> {
    let path = PathBuf::from(image);
    if !path.is_file() {
        return Err(format!("image not found: {image}"));
    }
    Ok(path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned())
}

fn run_vilecolors(bin: &Path, recipe: &str, image: &str) -> Result<(), String> {
    let output = Command::new(bin)
        .args(["--recipe", recipe, "--img", image])
        .output()
        .map_err(|err| format!("spawn {}: {err}", bin.display()))?;
    if output.status.success() {
        if !output.stderr.is_empty() {
            info!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!(
            "vilecolors exited {}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

fn format_clock(fmt: &str) -> String {
    let now = chrono::Local::now();
    std::panic::catch_unwind(|| now.format(fmt).to_string())
        .ok()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| now.format("%H:%M:%S").to_string())
}

struct Daemon {
    handle: LoopHandle<'static, Daemon>,
    config: Config,
    clients: HashMap<u64, Client>,
    next_id: u64,
    sampler: SysSampler,
    sys: SysSnapshot,
    niri: Option<Niri>,
    niri_token: Option<RegistrationToken>,
    workspaces: Vec<Workspace>,
    windows: HashMap<u64, Window>,
    colors_tx: channel::Sender<ColorsJobResult>,
    colors_busy: bool,
    wallpaper: Option<WallpaperState>,
    wall_spawned_at: Option<Instant>,
    apps: Option<AppsPrompt>,
    notify: NotifyCenter,
    tray: TrayCenter,
    tray_cmd: Option<TrayHandle>,
    menu: MenuCenter,
    toast_timer: Option<RegistrationToken>,
    grab_window: Option<u64>,
    grab_lost_window_focus: bool,
    started: Instant,
}

struct ColorsJobResult {
    client_id: u64,
    outcome: Result<(), String>,
}

struct Client {
    writer: UnixStream,
    read_buf: Vec<u8>,
    bar: Option<String>,
    output: Option<String>,
    cols: u32,
    hover_col: Option<u32>,
    mode: Option<ActiveMode>,
    token: Option<RegistrationToken>,
    last_text: String,
    last_cells: Vec<CellStyle>,
    spans: Vec<ModuleSpan>,
    is_wall: bool,
    transition: Option<Transition>,
    anim_timer: Option<RegistrationToken>,
}

struct ActiveMode {
    kind: ModeKind,
    until: Instant,
    timer: Option<RegistrationToken>,
}

struct ClientFd {
    id: u64,
    stream: UnixStream,
}

impl AsFd for ClientFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

struct NiriFd {
    stream: UnixStream,
}

impl AsFd for NiriFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl Daemon {
    fn accept_clients(&mut self, listener: &UnixListener) {
        loop {
            match listener.accept() {
                Ok((stream, _)) => self.add_client(stream),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    warn!("accept failed: {err}");
                    break;
                }
            }
        }
    }

    fn add_client(&mut self, stream: UnixStream) {
        if let Err(err) = stream.set_nonblocking(true) {
            warn!("set_nonblocking failed: {err}");
            return;
        }
        let writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(err) => {
                warn!("clone client socket failed: {err}");
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;

        let fd = ClientFd { id, stream };
        let token = match self.handle.insert_source(
            Generic::new(fd, Interest::READ, Mode::Level),
            |_, fd, daemon| daemon.on_client_readable(fd),
        ) {
            Ok(token) => token,
            Err(err) => {
                warn!("insert client source failed: {err}");
                return;
            }
        };

        self.clients.insert(
            id,
            Client {
                writer,
                read_buf: Vec::new(),
                bar: None,
                output: None,
                cols: 0,
                hover_col: None,
                mode: None,
                token: Some(token),
                last_text: String::new(),
                last_cells: Vec::new(),
                spans: Vec::new(),
                is_wall: false,
                transition: None,
                anim_timer: None,
            },
        );
        info!("bar client {id} connected");
    }

    fn on_client_readable(&mut self, fd: &impl AsRef<ClientFd>) -> std::io::Result<PostAction> {
        let fd = fd.as_ref();
        let id = fd.id;
        let mut stream = &fd.stream;
        let messages: Vec<BarToDaemon> =
            match vile_protocol::read_json_lines(&mut stream, self.read_buf(id)) {
                Ok(messages) => messages,
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.drop_client(id, false);
                    return Ok(PostAction::Remove);
                }
                Err(err) => {
                    warn!("client {id} read error: {err}");
                    self.drop_client(id, false);
                    return Ok(PostAction::Remove);
                }
            };

        for message in messages {
            if !self.handle_message(id, message) {
                self.drop_client(id, false);
                return Ok(PostAction::Remove);
            }
        }
        Ok(PostAction::Continue)
    }

    fn read_buf(&mut self, id: u64) -> &mut Vec<u8> {
        self.clients
            .get_mut(&id)
            .map(|client| &mut client.read_buf)
            .expect("client missing during read")
    }

    fn handle_message(&mut self, id: u64, message: BarToDaemon) -> bool {
        match message {
            BarToDaemon::Hello {
                bar, output, cols, ..
            } => {
                if let Some(client) = self.clients.get_mut(&id) {
                    info!("client {id} hello bar={bar} output={output} cols={cols}");
                    client.bar = Some(bar);
                    client.output = Some(output);
                    client.cols = cols;
                    client.last_text.clear();
                    client.last_cells.clear();
                }
                let ok = self.send_frame(id);
                if self.exclusive_prompt() && !self.send_keyboard(id, self.should_grab(id)) {
                    return false;
                }
                ok
            }
            BarToDaemon::Configure { cols, .. } => {
                if let Some(client) = self.clients.get_mut(&id) {
                    if client.cols != cols {
                        debug!("client {id} cols {cols}");
                        client.cols = cols;
                        client.last_text.clear();
                        client.last_cells.clear();
                    }
                }
                self.send_frame(id)
            }
            BarToDaemon::Pointer { col, event } => self.handle_pointer(id, col, event),
            BarToDaemon::Volume { delta } => {
                info!("volume delta {delta}");
                self.yield_toast();
                self.adjust_volume(delta);
                self.show_volume_overlay_from_buttons();
                let _ = self.ack(id);
                true
            }
            BarToDaemon::Mute => {
                info!("volume mute toggle");
                self.yield_toast();
                self.apply_mute_toggle();
                let _ = self.ack(id);
                true
            }
            BarToDaemon::Colors {
                recipe,
                image,
                apply_wall,
                wall_mode,
            } => {
                self.yield_toast();
                self.start_colors(id, recipe, image, apply_wall, wall_mode)
            }
            BarToDaemon::Wallpaper { image, mode } => {
                self.yield_toast();
                self.start_wallpaper(id, image, mode)
            }
            BarToDaemon::WallHello => self.on_wall_hello(id),
            BarToDaemon::AppsToggle => {
                self.yield_toast();
                self.toggle_apps();
                self.ack(id)
            }
            BarToDaemon::NotificationsToggle => {
                self.toggle_notifications();
                self.ack(id)
            }
            BarToDaemon::TrayToggle => {
                self.toggle_tray();
                self.ack(id)
            }
            BarToDaemon::MenuToggle => {
                self.toggle_menu();
                self.ack(id)
            }
            BarToDaemon::Overlay { kind } => {
                self.yield_toast();
                self.show_info_overlay(kind);
                self.ack(id)
            }
            BarToDaemon::Key { name, text } => {
                self.handle_key(name, text);
                true
            }
            BarToDaemon::KeyboardLost => {
                self.yield_grab_from_outside();
                true
            }
        }
    }

    fn on_wall_hello(&mut self, id: u64) -> bool {
        if let Some(client) = self.clients.get_mut(&id) {
            client.is_wall = true;
            info!("client {id} wall hello");
        }
        if let Some(state) = self.wallpaper.clone() {
            self.send_wallpaper_to(id, &state);
        }
        true
    }

    fn start_wallpaper(&mut self, id: u64, image: String, mode: Option<WallpaperMode>) -> bool {
        match self.apply_wallpaper(&image, mode) {
            Ok(()) => self.ack(id),
            Err(message) => self.reply_error(id, &message),
        }
    }

    fn apply_wallpaper(&mut self, image: &str, mode: Option<WallpaperMode>) -> Result<(), String> {
        let image = resolve_image_path(image)?;
        let mode = mode.unwrap_or(self.config.wallpaper.mode);
        let state = WallpaperState { image, mode };
        if let Err(err) = save_wallpaper_state(&state) {
            warn!("persist wallpaper state: {err}");
        }
        info!("wallpaper mode={} img={}", mode.as_str(), state.image);
        self.wallpaper = Some(state.clone());
        self.broadcast_wallpaper(&state);
        self.ensure_vilewall();
        Ok(())
    }

    fn broadcast_wallpaper(&mut self, state: &WallpaperState) {
        let ids: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, client)| client.is_wall)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if !self.send_wallpaper_to(id, state) {
                self.drop_client(id, true);
            }
        }
    }

    fn send_wallpaper_to(&mut self, id: u64, state: &WallpaperState) -> bool {
        let Some(client) = self.clients.get_mut(&id) else {
            return true;
        };
        match write_json_line(
            &mut client.writer,
            &DaemonToBar::SetWallpaper {
                image: state.image.clone(),
                mode: state.mode,
            },
        ) {
            Ok(()) => true,
            Err(err) => {
                warn!("client {id} wallpaper write failed: {err}");
                false
            }
        }
    }

    fn ensure_vilewall(&mut self) {
        if self.clients.values().any(|client| client.is_wall) {
            return;
        }
        if self
            .wall_spawned_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        let bin = find_sibling("vilewall");
        match Command::new(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                info!("spawned {}", bin.display());
                self.wall_spawned_at = Some(Instant::now());
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(err) => warn!("spawn {}: {err}", bin.display()),
        }
    }

    fn start_colors(
        &mut self,
        id: u64,
        recipe: String,
        image: String,
        apply_wall: bool,
        wall_mode: Option<WallpaperMode>,
    ) -> bool {
        if self.colors_busy {
            return self.reply_error(id, "colors job already running");
        }
        let recipe = sanitize_recipe_name(&recipe);
        if recipe.is_empty() {
            return self.reply_error(id, "invalid recipe name");
        }
        if image.is_empty() {
            return self.reply_error(id, "missing image path");
        }
        if apply_wall {
            if let Err(message) = self.apply_wallpaper(&image, wall_mode) {
                return self.reply_error(id, &message);
            }
        }
        let bin = find_vilecolors();
        info!(
            "colors recipe={recipe} img={image} helper={}",
            bin.display()
        );
        self.colors_busy = true;
        let tx = self.colors_tx.clone();
        thread::spawn(move || {
            let outcome = run_vilecolors(&bin, &recipe, &image);
            if let Err(err) = tx.send(ColorsJobResult {
                client_id: id,
                outcome,
            }) {
                warn!("colors result send failed: {err}");
            }
        });
        true
    }

    fn on_colors_done(&mut self, result: ColorsJobResult) {
        self.colors_busy = false;
        match result.outcome {
            Ok(()) => {
                info!("colors job ok for client {}", result.client_id);
                let _ = self.ack(result.client_id);
            }
            Err(message) => {
                warn!("colors job failed: {message}");
                let _ = self.reply_error(result.client_id, &message);
            }
        }
    }

    fn reply_error(&mut self, id: u64, message: &str) -> bool {
        let Some(client) = self.clients.get_mut(&id) else {
            return true;
        };
        match write_json_line(
            &mut client.writer,
            &DaemonToBar::Error {
                message: message.to_string(),
            },
        ) {
            Ok(()) => true,
            Err(err) => {
                warn!("client {id} error reply failed: {err}");
                false
            }
        }
    }

    fn ack(&mut self, id: u64) -> bool {
        let Some(client) = self.clients.get_mut(&id) else {
            return true;
        };
        match write_json_line(&mut client.writer, &DaemonToBar::Ok) {
            Ok(()) => true,
            Err(err) => {
                warn!("client {id} ack failed: {err}");
                false
            }
        }
    }

    fn toggle_apps(&mut self) {
        if self.apps.is_some() {
            self.close_apps();
        } else {
            self.open_apps();
        }
    }

    fn open_apps(&mut self) {
        info!("apps prompt open");
        let from = self.snapshot_all_lines();
        self.notify.inbox = None;
        self.notify.empty_until = None;
        self.tray.session = None;
        self.tray.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = None;
        self.note_grab_window();
        self.apps = Some(AppsPrompt::open());
        self.sync_apps_keyboard();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn close_apps(&mut self) {
        info!("apps prompt close");
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.sync_apps_keyboard();
        self.animate_all(from, SlideDir::Right, Region::Full);
    }

    fn focused_output(&self) -> Option<String> {
        self.workspaces
            .iter()
            .find(|ws| ws.is_focused)
            .and_then(|ws| ws.output.clone())
    }

    fn exclusive_prompt(&self) -> bool {
        self.apps.is_some()
            || self.notify.inbox.is_some()
            || self.tray.session.is_some()
            || self.menu.session.is_some()
    }

    fn note_grab_window(&mut self) {
        if self.grab_window.is_some() {
            return;
        }
        self.grab_window = self.focused_window_id();
        self.grab_lost_window_focus = false;
    }

    fn focused_window_id(&self) -> Option<u64> {
        self.windows
            .values()
            .find(|win| win.is_focused)
            .map(|win| win.id)
    }

    fn restore_grab_window(&mut self) {
        self.grab_lost_window_focus = false;
        let Some(id) = self.grab_window.take() else {
            return;
        };
        if self.windows.contains_key(&id) {
            if let Some(niri) = self.niri.as_mut() {
                niri.focus_window(id);
            }
        }
    }

    fn dismiss_exclusive_prompts(&mut self) {
        if self.apps.is_some() {
            self.close_apps();
        }
        if self.notify.inbox.is_some() {
            self.close_inbox(true);
        }
        if self.tray.session.is_some() {
            self.close_tray(true);
        }
        if self.menu.session.is_some() {
            self.close_menu(true);
        }
    }

    fn yield_grab_from_outside(&mut self) {
        if !self.exclusive_prompt() {
            return;
        }
        if let Some(id) = self.focused_window_id() {
            self.grab_window = Some(id);
        }
        info!("releasing exclusive grab (click outside)");
        self.dismiss_exclusive_prompts();
    }

    fn on_window_focus_while_grab(&mut self, id: Option<u64>) {
        if !self.exclusive_prompt() {
            return;
        }
        if id.is_none() {
            self.grab_lost_window_focus = true;
        }
        if should_yield_grab(self.grab_window, self.grab_lost_window_focus, id) {
            if let Some(id) = id {
                self.grab_window = Some(id);
            }
            info!("releasing exclusive grab (window focus)");
            self.dismiss_exclusive_prompts();
        }
    }

    fn should_grab(&self, id: u64) -> bool {
        if !self.exclusive_prompt() {
            return false;
        }
        let Some(focused) = self.focused_output() else {
            return true;
        };
        self.clients
            .get(&id)
            .and_then(|client| client.output.as_deref())
            == Some(focused.as_str())
    }

    fn sync_apps_keyboard(&mut self) {
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            let exclusive = self.should_grab(id);
            if !self.send_keyboard(id, exclusive) {
                self.drop_client(id, true);
            }
        }
        if !self.exclusive_prompt() {
            self.restore_grab_window();
        }
    }

    fn send_keyboard(&mut self, id: u64, exclusive: bool) -> bool {
        let Some(client) = self.clients.get_mut(&id) else {
            return true;
        };
        if client.is_wall || client.cols == 0 {
            return true;
        }
        match write_json_line(&mut client.writer, &DaemonToBar::Keyboard { exclusive }) {
            Ok(()) => true,
            Err(err) => {
                warn!("client {id} keyboard grab failed: {err}");
                false
            }
        }
    }

    fn handle_key(&mut self, name: Option<String>, text: Option<String>) {
        if self.menu.session.is_some() {
            self.handle_menu_key(name);
        } else if self.tray.session.is_some() {
            self.handle_tray_key(name);
        } else if self.apps.is_some() {
            self.handle_apps_key(name, text);
        } else if self.notify.inbox.is_some() {
            self.handle_inbox_key(name);
        }
    }

    fn handle_apps_key(&mut self, name: Option<String>, text: Option<String>) {
        if self.apps.is_none() {
            return;
        }
        match name.as_deref() {
            Some("escape") => {
                self.close_apps();
                return;
            }
            Some("return") => {
                self.launch_apps_selection();
                return;
            }
            Some("backspace") => {
                if let Some(apps) = self.apps.as_mut() {
                    apps.query.pop();
                    apps.clamp_selected();
                }
            }
            Some("tab") => {
                if let Some(name) = self
                    .apps
                    .as_ref()
                    .and_then(|apps| apps.selected_match())
                    .map(|app| app.name.clone())
                {
                    if let Some(apps) = self.apps.as_mut() {
                        apps.query = name;
                        apps.selected = 0;
                    }
                }
            }
            Some("up") => {
                if let Some(apps) = self.apps.as_mut() {
                    let n = apps.matches().len();
                    if n > 0 {
                        apps.selected = (apps.selected + n - 1) % n;
                    }
                }
            }
            Some("down") => {
                if let Some(apps) = self.apps.as_mut() {
                    let n = apps.matches().len();
                    if n > 0 {
                        apps.selected = (apps.selected + 1) % n;
                    }
                }
            }
            _ => {
                if let Some(text) = text {
                    if let Some(apps) = self.apps.as_mut() {
                        apps.query.push_str(&text);
                        apps.selected = 0;
                        apps.clamp_selected();
                    }
                }
            }
        }
        self.broadcast_frames();
    }

    fn launch_apps_selection(&mut self) {
        let terminal = self.config.terminal.clone();
        let Some(apps) = self.apps.as_ref() else {
            return;
        };
        if let Some(cmd) = command_remainder(&apps.query) {
            let command = if cmd.is_empty() {
                None
            } else {
                Some(cmd.to_string())
            };
            match spawn_terminal(&terminal, command.as_deref()) {
                Ok(()) => self.close_apps(),
                Err(err) => warn!("spawn terminal: {err}"),
            }
            return;
        }
        let Some(app) = apps.selected_match().cloned() else {
            return;
        };
        match launch_app(&app, &terminal) {
            Ok(()) => self.close_apps(),
            Err(err) => warn!("launch {}: {err}", app.name),
        }
    }

    fn overlay_mode(
        &self,
        id: u64,
        pick: fn(&vile_protocol::BarConfig) -> OverlayMode,
    ) -> OverlayMode {
        let Some(client) = self.clients.get(&id) else {
            return OverlayMode::default();
        };
        let name = client.bar.as_deref().unwrap_or("main");
        self.config
            .bar(name)
            .or_else(|| self.config.bars.first())
            .map(pick)
            .unwrap_or_default()
    }

    fn volume_overlay_mode(&self, id: u64) -> OverlayMode {
        self.overlay_mode(id, |bar| bar.volume_overlay)
    }

    fn battery_overlay_mode(&self, id: u64) -> OverlayMode {
        self.overlay_mode(id, |bar| bar.battery_overlay)
    }

    fn mem_overlay_mode(&self, id: u64) -> OverlayMode {
        self.overlay_mode(id, |bar| bar.mem_overlay)
    }

    fn cpu_overlay_mode(&self, id: u64) -> OverlayMode {
        self.overlay_mode(id, |bar| bar.cpu_overlay)
    }

    fn gpu_overlay_mode(&self, id: u64) -> OverlayMode {
        self.overlay_mode(id, |bar| bar.gpu_overlay)
    }

    fn client_overlay_kind(&self, id: u64) -> Option<ModeKind> {
        match &self.clients.get(&id)?.mode.as_ref()?.kind {
            ModeKind::VolumeOverlay => Some(ModeKind::VolumeOverlay),
            ModeKind::BatteryOverlay => Some(ModeKind::BatteryOverlay),
            ModeKind::MemOverlay => Some(ModeKind::MemOverlay),
            ModeKind::CpuOverlay => Some(ModeKind::CpuOverlay),
            ModeKind::GpuOverlay => Some(ModeKind::GpuOverlay),
            _ => None,
        }
    }

    fn show_volume_overlay_from_buttons(&mut self) {
        let timeout = Duration::from_millis(self.config.mode_timeout_ms);
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        let ids: Vec<u64> = ids
            .into_iter()
            .filter(|&id| {
                self.clients.get(&id).is_some_and(|client| client.cols > 0)
                    && self.volume_overlay_mode(id).from_buttons()
            })
            .collect();
        for id in ids {
            self.enter_mode(id, ModeKind::VolumeOverlay, timeout);
        }
    }

    fn show_info_overlay(&mut self, kind: OverlayKind) {
        let timeout = Duration::from_millis(self.config.mode_timeout_ms);
        let mode = match kind {
            OverlayKind::Battery => ModeKind::BatteryOverlay,
            OverlayKind::Mem => ModeKind::MemOverlay,
            OverlayKind::Cpu => ModeKind::CpuOverlay,
            OverlayKind::Gpu => ModeKind::GpuOverlay,
        };
        let ids: Vec<u64> = self
            .clients
            .keys()
            .copied()
            .filter(|&id| {
                self.clients
                    .get(&id)
                    .is_some_and(|client| !client.is_wall && client.cols > 0)
            })
            .collect();
        for id in ids {
            self.enter_mode(id, mode.clone(), timeout);
        }
    }

    fn handle_pointer(&mut self, id: u64, col: u32, event: PointerEvent) -> bool {
        let dismissed_toast = self.notify.toast.is_some();
        if dismissed_toast {
            self.yield_toast();
        }
        let timeout = Duration::from_millis(self.config.mode_timeout_ms);
        let overlay = self.client_overlay_kind(id);
        let span_name = self
            .clients
            .get(&id)
            .and_then(|client| span_at(&client.spans, col))
            .map(|span| (span.name.clone(), span.workspace_id));

        match event {
            PointerEvent::Leave => {
                if let Some(client) = self.clients.get_mut(&id) {
                    client.hover_col = None;
                }
                if let Some(kind) = overlay {
                    self.enter_mode(id, kind, timeout)
                } else {
                    self.send_frame(id)
                }
            }
            PointerEvent::Enter | PointerEvent::Motion => {
                let changed = self
                    .clients
                    .get_mut(&id)
                    .map(|client| {
                        let changed = client.hover_col != Some(col);
                        if changed {
                            client.hover_col = Some(col);
                        }
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.send_frame(id)
                } else {
                    true
                }
            }
            PointerEvent::Button { pressed, .. }
                if pressed && matches!(overlay, Some(ModeKind::VolumeOverlay)) =>
            {
                match span_name.as_ref().map(|(name, _)| name.as_str()) {
                    Some("volume-down") => {
                        self.adjust_volume(-5);
                        self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
                    }
                    Some("volume-up") => {
                        self.adjust_volume(5);
                        self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
                    }
                    Some("volume-mute") => {
                        self.apply_mute_toggle();
                        self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
                    }
                    Some("volume") => self.enter_mode(id, ModeKind::VolumeOverlay, timeout),
                    _ => true,
                }
            }
            PointerEvent::Button { pressed, .. }
                if pressed && matches!(overlay, Some(ModeKind::BatteryOverlay)) =>
            {
                self.enter_mode(id, ModeKind::BatteryOverlay, timeout)
            }
            PointerEvent::Button { pressed, .. }
                if pressed && matches!(overlay, Some(ModeKind::MemOverlay)) =>
            {
                self.enter_mode(id, ModeKind::MemOverlay, timeout)
            }
            PointerEvent::Button { pressed, .. }
                if pressed && matches!(overlay, Some(ModeKind::CpuOverlay)) =>
            {
                self.enter_mode(id, ModeKind::CpuOverlay, timeout)
            }
            PointerEvent::Button { pressed, .. }
                if pressed && matches!(overlay, Some(ModeKind::GpuOverlay)) =>
            {
                self.enter_mode(id, ModeKind::GpuOverlay, timeout)
            }
            PointerEvent::Button { code, pressed } if pressed => {
                if let Some(workspace_id) = self
                    .clients
                    .get(&id)
                    .and_then(|client| workspace_id_at(&client.spans, col))
                {
                    self.focus_workspace(id, workspace_id);
                    return true;
                }
                match span_name {
                    Some((name, _)) if name == "volume" => {
                        if self.volume_overlay_mode(id).from_buttons() {
                            self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
                        } else {
                            true
                        }
                    }
                    Some((name, _)) if name == "battery" => {
                        if self.battery_overlay_mode(id).from_buttons() {
                            self.enter_mode(id, ModeKind::BatteryOverlay, timeout)
                        } else {
                            true
                        }
                    }
                    Some((name, _)) if name == "mem" => {
                        if self.mem_overlay_mode(id).from_buttons() {
                            self.enter_mode(id, ModeKind::MemOverlay, timeout)
                        } else {
                            true
                        }
                    }
                    Some((name, _)) if name == "cpu" => {
                        if self.cpu_overlay_mode(id).from_buttons() {
                            self.enter_mode(id, ModeKind::CpuOverlay, timeout)
                        } else {
                            true
                        }
                    }
                    Some((name, _)) if name == "gpu" => {
                        if self.gpu_overlay_mode(id).from_buttons() {
                            self.enter_mode(id, ModeKind::GpuOverlay, timeout)
                        } else {
                            true
                        }
                    }
                    Some((name, _)) if name == "notifications" => {
                        if dismissed_toast {
                            true
                        } else {
                            self.toggle_notifications();
                            true
                        }
                    }
                    Some((name, _)) if name == "hover" || name == "hint" => {
                        self.enter_mode(id, ModeKind::InspectClick { col, button: code }, timeout)
                    }
                    _ => true,
                }
            }
            PointerEvent::Button { .. } => true,
            PointerEvent::Axis { steps }
                if steps != 0 && matches!(overlay, Some(ModeKind::VolumeOverlay)) =>
            {
                self.adjust_volume(-steps * 5);
                self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
            }
            PointerEvent::Axis { steps }
                if steps != 0 && matches!(overlay, Some(ModeKind::BatteryOverlay)) =>
            {
                self.enter_mode(id, ModeKind::BatteryOverlay, timeout)
            }
            PointerEvent::Axis { steps }
                if steps != 0 && matches!(overlay, Some(ModeKind::MemOverlay)) =>
            {
                self.enter_mode(id, ModeKind::MemOverlay, timeout)
            }
            PointerEvent::Axis { steps }
                if steps != 0 && matches!(overlay, Some(ModeKind::CpuOverlay)) =>
            {
                self.enter_mode(id, ModeKind::CpuOverlay, timeout)
            }
            PointerEvent::Axis { steps }
                if steps != 0 && matches!(overlay, Some(ModeKind::GpuOverlay)) =>
            {
                self.enter_mode(id, ModeKind::GpuOverlay, timeout)
            }
            PointerEvent::Axis { steps } if steps != 0 => match span_name {
                Some((name, _)) if name == "volume" => {
                    self.adjust_volume(-steps * 5);
                    if self.volume_overlay_mode(id).from_scroll() {
                        self.enter_mode(id, ModeKind::VolumeOverlay, timeout)
                    } else {
                        self.send_frame(id)
                    }
                }
                Some((name, _)) if name == "battery" => {
                    if self.battery_overlay_mode(id).from_scroll() {
                        self.enter_mode(id, ModeKind::BatteryOverlay, timeout)
                    } else {
                        true
                    }
                }
                Some((name, _)) if name == "mem" => {
                    if self.mem_overlay_mode(id).from_scroll() {
                        self.enter_mode(id, ModeKind::MemOverlay, timeout)
                    } else {
                        true
                    }
                }
                Some((name, _)) if name == "cpu" => {
                    if self.cpu_overlay_mode(id).from_scroll() {
                        self.enter_mode(id, ModeKind::CpuOverlay, timeout)
                    } else {
                        true
                    }
                }
                Some((name, _)) if name == "gpu" => {
                    if self.gpu_overlay_mode(id).from_scroll() {
                        self.enter_mode(id, ModeKind::GpuOverlay, timeout)
                    } else {
                        true
                    }
                }
                Some((name, _)) if name == "workspaces" => {
                    self.cycle_workspace(id, -steps);
                    self.send_frame(id)
                }
                Some((name, _)) if name == "hover" || name == "hint" => {
                    self.enter_mode(id, ModeKind::InspectScroll { col, steps }, timeout)
                }
                _ => true,
            },
            PointerEvent::Axis { .. } => true,
        }
    }

    fn adjust_volume(&mut self, delta: i32) {
        if delta != 0 {
            if let Some(volume) = self.sys.volume.as_mut() {
                if !volume.muted {
                    let next = (volume.percent as i32 + delta).clamp(0, 100);
                    volume.percent = next as u32;
                }
            }
            self.broadcast_frames();
        }
        if let Some(volume) = set_volume_delta(delta) {
            self.sampler.note_volume(volume.clone());
            self.sys.volume = Some(volume);
        }
        self.broadcast_frames();
    }

    fn apply_mute_toggle(&mut self) {
        if let Some(volume) = self.sys.volume.as_mut() {
            volume.muted = !volume.muted;
        }
        self.broadcast_frames();
        if let Some(volume) = crate::sys::toggle_mute() {
            self.sampler.note_volume(volume.clone());
            self.sys.volume = Some(volume);
        }
        self.broadcast_frames();
    }

    fn focus_workspace(&mut self, id: u64, workspace_id: u64) {
        let Some(output) = self
            .clients
            .get(&id)
            .and_then(|client| client.output.clone())
        else {
            return;
        };
        if let Some(niri) = self.niri.as_mut() {
            niri.focus_workspace(&output, workspace_id);
        } else {
            warn!("niri unavailable; cannot focus workspace {workspace_id}");
        }
    }

    fn cycle_workspace(&mut self, id: u64, steps: i32) {
        let Some(output) = self
            .clients
            .get(&id)
            .and_then(|client| client.output.clone())
        else {
            return;
        };
        let occupied = occupied_workspace_ids(&self.windows);
        let list = workspaces_for(&self.workspaces, &output, &occupied);
        if list.is_empty() {
            return;
        }
        let active = list.iter().position(|ws| ws.is_active).unwrap_or(0);
        let len = list.len() as i32;
        let next = (active as i32 + steps.signum()).rem_euclid(len) as usize;
        let workspace_id = list[next].id;
        if let Some(niri) = self.niri.as_mut() {
            niri.focus_workspace(&output, workspace_id);
        }
    }

    fn enter_mode(&mut self, id: u64, kind: ModeKind, timeout: Duration) -> bool {
        self.yield_toast();
        let prev = self
            .clients
            .get(&id)
            .and_then(|client| client.mode.as_ref().map(|mode| mode.kind.clone()));
        let animate = should_anim_mode(prev.as_ref(), Some(&kind));
        let from = if animate { self.compose_live(id) } else { None };
        self.clear_mode_timer(id);
        let handle = self.handle.clone();
        let timer = handle
            .insert_source(Timer::from_duration(timeout), move |_, _, daemon| {
                daemon.expire_mode(id);
                TimeoutAction::Drop
            })
            .ok();
        if let Some(client) = self.clients.get_mut(&id) {
            client.mode = Some(ActiveMode {
                kind,
                until: Instant::now() + timeout,
                timer,
            });
        }
        if let Some(from) = from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    FrozenLine::from_composed(&from),
                    FrozenLine::from_composed(&to),
                    SlideDir::Left,
                    Phase::Out,
                    Region::Full,
                );
                return true;
            }
        }
        self.send_frame(id)
    }

    fn expire_mode(&mut self, id: u64) {
        let timeout = Duration::from_millis(self.config.mode_timeout_ms);
        let hovering = self
            .clients
            .get(&id)
            .is_some_and(|client| client.hover_col.is_some());
        if hovering {
            if let Some(kind) = self.client_overlay_kind(id) {
                self.enter_mode(id, kind, timeout);
                return;
            }
        }
        let prev = self
            .clients
            .get(&id)
            .and_then(|client| client.mode.as_ref().map(|mode| mode.kind.clone()));
        if let Some(client) = self.clients.get(&id) {
            if let Some(mode) = &client.mode {
                if mode.until > Instant::now() {
                    return;
                }
            }
        }
        let animate = should_anim_mode(prev.as_ref(), None);
        let from = if animate { self.compose_live(id) } else { None };
        if let Some(client) = self.clients.get_mut(&id) {
            client.mode = None;
        }
        if let Some(from) = from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    FrozenLine::from_composed(&from),
                    FrozenLine::from_composed(&to),
                    SlideDir::Right,
                    Phase::Out,
                    Region::Full,
                );
                return;
            }
        }
        if !self.send_frame(id) {
            self.drop_client(id, true);
        }
    }

    fn clear_mode_timer(&mut self, id: u64) {
        let token = self
            .clients
            .get_mut(&id)
            .and_then(|client| client.mode.as_mut().and_then(|mode| mode.timer.take()));
        if let Some(token) = token {
            self.handle.remove(token);
        }
    }

    fn tick(&mut self) {
        self.sys = self.sampler.sample();
        if self.niri.is_none() {
            self.connect_niri();
        }
        self.broadcast_frames();
    }

    fn charge_tick(&mut self) {
        if self.needs_charge_anim() {
            self.broadcast_frames();
        }
    }

    fn needs_charge_anim(&self) -> bool {
        let Some(bat) = self.sys.battery.as_ref() else {
            return false;
        };
        if !bat.charging || bat.percent >= 100 || bat.status.eq_ignore_ascii_case("Full") {
            return false;
        }
        let overlay = self.clients.values().any(|client| {
            matches!(
                client.mode.as_ref().map(|m| &m.kind),
                Some(ModeKind::BatteryOverlay)
            )
        });
        overlay
            || self.config.bars.iter().any(|bar| {
                bar.battery_style == MeterStyle::Meter
                    && (bar
                        .modules
                        .left
                        .iter()
                        .chain(&bar.modules.center)
                        .chain(&bar.modules.right))
                    .any(|name| name == "battery")
            })
    }

    fn connect_niri(&mut self) {
        if self.niri.is_some() {
            return;
        }
        let Some((mut niri, stream)) = Niri::connect() else {
            return;
        };
        if let Some((workspaces, windows)) = niri.refresh() {
            self.workspaces = workspaces;
            self.windows = windows.into_iter().map(|win| (win.id, win)).collect();
        }
        let token = match self.handle.insert_source(
            Generic::new(NiriFd { stream }, Interest::READ, Mode::Level),
            |_, fd, daemon| daemon.on_niri_readable(fd),
        ) {
            Ok(token) => token,
            Err(err) => {
                warn!("insert niri source failed: {err}");
                return;
            }
        };
        self.niri = Some(niri);
        self.niri_token = Some(token);
        info!("connected to niri");
        self.broadcast_frames();
    }

    fn on_niri_readable(&mut self, fd: &impl AsRef<NiriFd>) -> std::io::Result<PostAction> {
        let stream = &fd.as_ref().stream;
        let Some(niri) = self.niri.as_mut() else {
            return Ok(PostAction::Remove);
        };
        let events = match niri.read_events(stream) {
            Ok(events) => events,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.drop_niri(false);
                return Ok(PostAction::Remove);
            }
            Err(err) => {
                warn!("niri read error: {err}");
                self.drop_niri(false);
                return Ok(PostAction::Remove);
            }
        };
        let mut changed = false;
        let had_prompt = self.exclusive_prompt();
        for event in events {
            if apply_event(&mut self.workspaces, &mut self.windows, &event) {
                changed = true;
            }
            if !had_prompt {
                continue;
            }
            match &event {
                niri_ipc::Event::WindowFocusChanged { id } => {
                    self.on_window_focus_while_grab(*id);
                }
                niri_ipc::Event::OverviewOpenedOrClosed { is_open: true } => {
                    info!("releasing exclusive grab (overview)");
                    self.dismiss_exclusive_prompts();
                }
                _ => {}
            }
        }
        if changed {
            self.broadcast_frames();
        }
        Ok(PostAction::Continue)
    }

    fn drop_niri(&mut self, remove_source: bool) {
        self.niri = None;
        let token = self.niri_token.take();
        if remove_source {
            if let Some(token) = token {
                self.handle.remove(token);
            }
        }
        warn!("niri socket closed");
    }

    fn broadcast_frames(&mut self) {
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            if !self.send_frame(id) {
                self.drop_client(id, true);
            }
        }
    }

    fn send_frame(&mut self, id: u64) -> bool {
        let Some(client) = self.clients.get(&id) else {
            return true;
        };
        if client.is_wall || client.cols == 0 {
            return true;
        }
        let bar_name = client.bar.clone().unwrap_or_else(|| "main".into());
        let last_text = client.last_text.clone();
        let last_cells = client.last_cells.clone();
        let anim_line = client.transition.as_ref().map(|t| t.current());
        let bar = self
            .config
            .bar(&bar_name)
            .cloned()
            .or_else(|| self.config.bars.first().cloned());
        let Some(bar) = bar else {
            return true;
        };
        let animating = anim_line.is_some();
        let (text, cells, spans) = if let Some(frozen) = anim_line {
            (frozen.text, frozen.cells, Vec::new())
        } else {
            let Some(composed) = self.compose_live(id) else {
                return true;
            };
            (composed.text, composed.cells, composed.spans)
        };
        if text == last_text && cells == last_cells {
            if let Some(client) = self.clients.get_mut(&id) {
                if !animating {
                    client.spans = spans;
                }
            }
            return true;
        }
        let frame = DaemonToBar::Frame {
            text: text.clone(),
            fg: bar.fg_argb(),
            bg: bar.bg_argb(),
            cells: cells.clone(),
        };
        let client = self.clients.get_mut(&id).unwrap();
        match write_json_line(&mut client.writer, &frame) {
            Ok(()) => {
                client.last_text = text;
                client.last_cells = cells;
                client.spans = spans;
                true
            }
            Err(err) => {
                warn!("client {id} write failed: {err}");
                false
            }
        }
    }

    fn inbox_text(&self) -> Option<String> {
        if let Some(line) = self.menu.line(&self.config.menu.items) {
            return Some(line);
        }
        if let Some(line) = self.tray.line() {
            return Some(line);
        }
        if let Some(until) = self.notify.empty_until {
            if Instant::now() < until {
                return Some("no notifications".into());
            }
        }
        let inbox = self.notify.inbox.as_ref()?;
        if inbox.confirm_clear {
            return Some("clear all?  enter / esc".into());
        }
        let total = self.notify.items.len();
        self.notify
            .current()
            .map(|item| item.inbox_text(inbox.index, total))
    }

    fn toast_view(&self, id: u64) -> Option<(NotificationSide, String)> {
        if self.apps.is_some()
            || self.notify.inbox.is_some()
            || self.tray.session.is_some()
            || self.menu.session.is_some()
        {
            return None;
        }
        if self
            .clients
            .get(&id)
            .and_then(|client| client.mode.as_ref())
            .is_some_and(|mode| is_full_overlay(&mode.kind))
        {
            return None;
        }
        let toast = self.notify.toast.as_ref()?;
        Some((self.config.notifications.side, toast.text.clone()))
    }

    fn compose_live(&self, id: u64) -> Option<ComposedLine> {
        let client = self.clients.get(&id)?;
        if client.is_wall || client.cols == 0 {
            return None;
        }
        let bar_name = client.bar.clone().unwrap_or_else(|| "main".into());
        let bar = self
            .config
            .bar(&bar_name)
            .cloned()
            .or_else(|| self.config.bars.first().cloned())?;
        let clock = format_clock(&bar.clock_format);
        let inbox = self.inbox_text();
        let toast = self.toast_view(id);
        let toast_ref = toast.as_ref().map(|(side, text)| (*side, text.as_str()));
        let notify_count = if self.config.notifications.enabled {
            self.notify.items.len()
        } else {
            0
        };
        Some(compose_line(
            &bar,
            client.cols as usize,
            &clock,
            client.hover_col,
            client.mode.as_ref().map(|mode| &mode.kind),
            self.apps.as_ref(),
            &self.sys,
            &self.workspaces,
            &occupied_workspace_ids(&self.windows),
            client.output.as_deref(),
            self.started.elapsed(),
            inbox.as_deref(),
            toast_ref,
            notify_count,
        ))
    }

    fn snapshot_all_lines(&self) -> Vec<(u64, FrozenLine)> {
        self.clients
            .keys()
            .copied()
            .filter_map(|id| {
                self.compose_live(id)
                    .map(|line| (id, FrozenLine::from_composed(&line)))
            })
            .collect()
    }

    fn animate_all(&mut self, from: Vec<(u64, FrozenLine)>, dir: SlideDir, region: Region) {
        if from.is_empty() {
            self.broadcast_frames();
            return;
        }
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Out,
                    region,
                );
            }
        }
    }

    fn begin_transition(
        &mut self,
        id: u64,
        from: FrozenLine,
        to: FrozenLine,
        dir: SlideDir,
        phase: Phase,
        region: Region,
    ) {
        self.cancel_anim(id);
        if !self.config.animations.enabled {
            let _ = self.send_frame(id);
            return;
        }
        let total = match phase {
            Phase::Cross => self.config.animations.navigation_frames(),
            _ => self.config.animations.transition_frames(),
        };
        let ms = self.config.animations.frame_ms();
        let handle = self.handle.clone();
        let timer = handle
            .insert_source(
                Timer::from_duration(Duration::from_millis(ms)),
                move |_, _, daemon| daemon.on_anim_tick(id),
            )
            .ok();
        if let Some(client) = self.clients.get_mut(&id) {
            client.transition = Some(Transition {
                from,
                to,
                dir,
                phase,
                frame: 0,
                total,
                region,
                frame_ms: ms,
            });
            client.anim_timer = timer;
        }
        let _ = self.send_frame(id);
    }

    fn on_anim_tick(&mut self, id: u64) -> TimeoutAction {
        let still = self
            .clients
            .get_mut(&id)
            .and_then(|client| client.transition.as_mut())
            .map(|transition| transition.advance())
            .unwrap_or(false);
        if !still {
            self.cancel_anim(id);
            if !self.send_frame(id) {
                self.drop_client(id, true);
            }
            return TimeoutAction::Drop;
        }
        if !self.send_frame(id) {
            self.drop_client(id, true);
            return TimeoutAction::Drop;
        }
        let ms = self
            .clients
            .get(&id)
            .and_then(|client| client.transition.as_ref())
            .map(|transition| transition.frame_ms)
            .unwrap_or_else(|| self.config.animations.frame_ms());
        TimeoutAction::ToDuration(Duration::from_millis(ms))
    }

    fn cancel_anim(&mut self, id: u64) {
        let token = self
            .clients
            .get_mut(&id)
            .and_then(|client| client.anim_timer.take());
        if let Some(token) = token {
            self.handle.remove(token);
        }
        if let Some(client) = self.clients.get_mut(&id) {
            client.transition = None;
        }
    }

    fn yield_toast(&mut self) {
        if self.notify.toast.take().is_none() && self.notify.empty_until.take().is_none() {
            return;
        }
        self.clear_toast_timer();
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            self.cancel_anim(id);
            let _ = self.send_frame(id);
        }
    }

    fn clear_toast_timer(&mut self) {
        if let Some(token) = self.toast_timer.take() {
            self.handle.remove(token);
        }
    }

    fn show_toast(&mut self, item: &NotifyItem) {
        if !self.config.notifications.enabled {
            return;
        }
        if self.notify.inbox.is_some()
            || self.apps.is_some()
            || self.tray.session.is_some()
            || self.menu.session.is_some()
        {
            return;
        }
        self.notify.empty_until = None;
        let from = self.snapshot_all_lines();
        self.notify.toast = Some(ToastState {
            id: item.id,
            text: item.toast_text(),
        });
        let timeout =
            Duration::from_millis(self.config.notifications.toast_ms(item.expire_timeout_ms));
        self.clear_toast_timer();
        let handle = self.handle.clone();
        self.toast_timer = handle
            .insert_source(Timer::from_duration(timeout), |_, _, daemon| {
                daemon.expire_toast();
                TimeoutAction::Drop
            })
            .ok();
        let side = self.config.notifications.side;
        let dir = match side {
            NotificationSide::Left => SlideDir::Left,
            NotificationSide::Right => SlideDir::Right,
        };
        let ids: Vec<u64> = from.iter().map(|(id, _)| *id).collect();
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                let (start, width) = to.side_region(side);
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Out,
                    Region::Side { start, width },
                );
            }
        }
        if ids.is_empty() {
            self.broadcast_frames();
        }
    }

    fn expire_toast(&mut self) {
        if self.notify.toast.is_none() {
            return;
        }
        self.toast_timer = None;
        let side = self.config.notifications.side;
        let from: Vec<(u64, FrozenLine, usize, usize)> = self
            .clients
            .keys()
            .copied()
            .filter_map(|id| {
                let line = self.compose_live(id)?;
                let (start, width) = line.side_region(side);
                Some((id, FrozenLine::from_composed(&line), start, width))
            })
            .collect();
        self.notify.toast = None;
        let dir = match side {
            NotificationSide::Left => SlideDir::Left,
            NotificationSide::Right => SlideDir::Right,
        };
        if !self.config.animations.enabled {
            self.broadcast_frames();
            return;
        }
        for (id, from_line, start, width) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Out,
                    Region::Side { start, width },
                );
            }
        }
    }

    fn on_notify_event(&mut self, event: NotifyEvent) {
        if !self.config.notifications.enabled {
            return;
        }
        match event {
            NotifyEvent::Notify(item) => {
                let cap = self.config.notifications.history();
                self.notify.upsert(item.clone(), cap);
                if self.notify.inbox.is_none() {
                    self.show_toast(&item);
                } else {
                    self.broadcast_frames();
                }
            }
            NotifyEvent::Close(id) => {
                let showing = self
                    .notify
                    .toast
                    .as_ref()
                    .is_some_and(|toast| toast.id == id);
                let had_inbox = self.notify.inbox.is_some();
                self.notify.remove(id);
                if had_inbox && self.notify.inbox.is_none() {
                    self.sync_apps_keyboard();
                }
                if showing {
                    self.yield_toast();
                } else {
                    self.broadcast_frames();
                }
            }
        }
    }

    fn toggle_notifications(&mut self) {
        if !self.config.notifications.enabled {
            if self.notify.inbox.is_some() {
                self.close_inbox(true);
            }
            return;
        }
        self.yield_toast();
        if self.notify.inbox.is_some() {
            self.close_inbox(true);
            return;
        }
        if self.notify.items.is_empty() {
            self.show_empty_inbox();
            return;
        }
        self.open_inbox();
    }

    fn show_empty_inbox(&mut self) {
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.tray.session = None;
        self.tray.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = None;
        self.notify.inbox = None;
        self.notify.empty_until = Some(Instant::now() + Duration::from_secs(1));
        self.sync_apps_keyboard();
        self.clear_toast_timer();
        let handle = self.handle.clone();
        self.toast_timer = handle
            .insert_source(
                Timer::from_duration(Duration::from_secs(1)),
                |_, _, daemon| {
                    daemon.toast_timer = None;
                    let from = daemon.snapshot_all_lines();
                    daemon.notify.empty_until = None;
                    daemon.animate_all(from, SlideDir::Right, Region::Full);
                    TimeoutAction::Drop
                },
            )
            .ok();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn open_inbox(&mut self) {
        info!("notifications inbox open");
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.tray.session = None;
        self.tray.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = None;
        self.note_grab_window();
        self.notify.inbox = Some(InboxState {
            index: self.notify.items.len().saturating_sub(1),
            confirm_clear: false,
        });
        self.notify.empty_until = None;
        self.sync_apps_keyboard();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn close_inbox(&mut self, animate: bool) {
        info!("notifications inbox close");
        if self.notify.inbox.is_none() {
            return;
        }
        let from = if animate {
            self.snapshot_all_lines()
        } else {
            Vec::new()
        };
        self.notify.inbox = None;
        self.sync_apps_keyboard();
        if animate {
            self.animate_all(from, SlideDir::Right, Region::Full);
        } else {
            self.broadcast_frames();
        }
    }

    fn handle_inbox_key(&mut self, name: Option<String>) {
        let Some(name) = name else {
            return;
        };
        match name.as_str() {
            "escape" => {
                if self
                    .notify
                    .inbox
                    .as_ref()
                    .is_some_and(|inbox| inbox.confirm_clear)
                {
                    if let Some(inbox) = self.notify.inbox.as_mut() {
                        inbox.confirm_clear = false;
                    }
                    self.broadcast_frames();
                } else {
                    self.close_inbox(true);
                }
            }
            "return" => {
                if self
                    .notify
                    .inbox
                    .as_ref()
                    .is_some_and(|inbox| inbox.confirm_clear)
                {
                    self.notify.items.clear();
                    self.notify.inbox = None;
                    self.sync_apps_keyboard();
                    self.broadcast_frames();
                    return;
                }
                self.focus_current_notification();
            }
            "left" => self.inbox_step(-1),
            "right" => self.inbox_step(1),
            "down" => self.inbox_clear_current(),
            "up" => {
                if let Some(inbox) = self.notify.inbox.as_mut() {
                    inbox.confirm_clear = true;
                }
                self.broadcast_frames();
            }
            _ => {}
        }
    }

    fn inbox_step(&mut self, delta: i32) {
        let n = self.notify.items.len();
        if n == 0 {
            self.close_inbox(true);
            return;
        }
        let Some(inbox) = self.notify.inbox.as_mut() else {
            return;
        };
        inbox.confirm_clear = false;
        let from_idx = inbox.index;
        let next = (from_idx as i32 + delta).rem_euclid(n as i32) as usize;
        if next == from_idx {
            return;
        }
        let from = self.snapshot_all_lines();
        if let Some(inbox) = self.notify.inbox.as_mut() {
            inbox.index = next;
        }
        let dir = if delta > 0 {
            SlideDir::Left
        } else {
            SlideDir::Right
        };
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Cross,
                    Region::Full,
                );
            }
        }
    }

    fn inbox_clear_current(&mut self) {
        let id = match self.notify.current() {
            Some(item) => item.id,
            None => {
                self.close_inbox(true);
                return;
            }
        };
        let from = self.snapshot_all_lines();
        self.notify.remove(id);
        if self.notify.items.is_empty() {
            self.notify.inbox = None;
            self.sync_apps_keyboard();
            self.animate_all(from, SlideDir::Right, Region::Full);
            return;
        }
        for (bar_id, from_line) in from {
            if let Some(to) = self.compose_live(bar_id) {
                self.begin_transition(
                    bar_id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    SlideDir::Left,
                    Phase::Cross,
                    Region::Full,
                );
            }
        }
    }

    fn on_tray_event(&mut self, event: TrayEvent) {
        if !self.config.tray.enabled {
            return;
        }
        match event {
            TrayEvent::Items(items) => {
                let had_session = self.tray.session.is_some();
                self.tray.items = items;
                self.tray.clamp_session();
                if had_session && self.tray.session.is_none() {
                    self.sync_apps_keyboard();
                    self.broadcast_frames();
                } else if self.tray.session.is_some() {
                    self.broadcast_frames();
                }
            }
            TrayEvent::Menu {
                key,
                parent_id,
                entries,
            } => self.on_tray_menu(key, parent_id, entries),
        }
    }

    fn on_tray_menu(&mut self, key: String, parent_id: i32, entries: Vec<crate::tray::MenuEntry>) {
        {
            let Some(session) = self.tray.session.as_mut() else {
                return;
            };
            if session.pending_menu.as_ref() != Some(&(key, parent_id)) {
                return;
            }
            session.pending_menu = None;
        }
        let from = self.snapshot_all_lines();
        let Some(session) = self.tray.session.as_mut() else {
            return;
        };
        if entries.is_empty() && parent_id == 0 {
            session.view = TrayView::NoMenu;
        } else if parent_id == 0 {
            session.view = TrayView::Menu {
                stack: vec![crate::tray::MenuLevel {
                    parent_id,
                    entries,
                    index: 0,
                }],
            };
        } else if let TrayView::Menu { stack } = &mut session.view {
            stack.push(crate::tray::MenuLevel {
                parent_id,
                entries,
                index: 0,
            });
        } else {
            session.view = TrayView::Menu {
                stack: vec![crate::tray::MenuLevel {
                    parent_id,
                    entries,
                    index: 0,
                }],
            };
        }
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    SlideDir::Left,
                    Phase::Cross,
                    Region::Full,
                );
            }
        }
    }

    fn toggle_tray(&mut self) {
        if !self.config.tray.enabled {
            if self.tray.session.is_some() {
                self.close_tray(true);
            }
            return;
        }
        self.yield_toast();
        if self.tray.session.is_some() {
            self.close_tray(true);
            return;
        }
        if self.tray.items.is_empty() {
            self.show_empty_tray();
            return;
        }
        self.open_tray();
    }

    fn show_empty_tray(&mut self) {
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.notify.inbox = None;
        self.notify.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = None;
        self.tray.session = None;
        self.tray.empty_until = Some(Instant::now() + Duration::from_secs(1));
        self.sync_apps_keyboard();
        self.clear_toast_timer();
        let handle = self.handle.clone();
        self.toast_timer = handle
            .insert_source(
                Timer::from_duration(Duration::from_secs(1)),
                |_, _, daemon| {
                    daemon.toast_timer = None;
                    let from = daemon.snapshot_all_lines();
                    daemon.tray.empty_until = None;
                    daemon.animate_all(from, SlideDir::Right, Region::Full);
                    TimeoutAction::Drop
                },
            )
            .ok();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn open_tray(&mut self) {
        info!("tray inbox open");
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.notify.inbox = None;
        self.notify.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = None;
        self.tray.empty_until = None;
        self.note_grab_window();
        self.tray.session = Some(TraySession::new(self.tray.items.len().saturating_sub(1)));
        self.sync_apps_keyboard();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn close_tray(&mut self, animate: bool) {
        info!("tray inbox close");
        if self.tray.session.is_none() {
            return;
        }
        let from = if animate {
            self.snapshot_all_lines()
        } else {
            Vec::new()
        };
        self.tray.session = None;
        self.sync_apps_keyboard();
        if animate {
            self.animate_all(from, SlideDir::Right, Region::Full);
        } else {
            self.broadcast_frames();
        }
    }

    fn handle_tray_key(&mut self, name: Option<String>) {
        let Some(name) = name else {
            return;
        };
        match name.as_str() {
            "escape" => {
                let close = self
                    .tray
                    .session
                    .as_mut()
                    .map(TraySession::esc)
                    .unwrap_or(true);
                if close {
                    self.close_tray(true);
                } else {
                    self.broadcast_frames();
                }
            }
            "return" => self.tray_enter(),
            "left" => self.tray_step(-1),
            "right" => self.tray_step(1),
            "down" => self.tray_open_menu(),
            "up" => {
                if let Some(session) = self.tray.session.as_mut() {
                    session.confirm_kill = true;
                }
                self.broadcast_frames();
            }
            _ => {}
        }
    }

    fn tray_step(&mut self, delta: i32) {
        if self.tray.items.is_empty() {
            self.close_tray(true);
            return;
        }
        let n = {
            let Some(session) = self.tray.session.as_mut() else {
                return;
            };
            session.confirm_kill = false;
            match &session.view {
                TrayView::Menu { stack } => {
                    stack.last().map(|level| level.entries.len()).unwrap_or(0)
                }
                TrayView::NoMenu => 0,
                TrayView::List => self.tray.items.len(),
            }
        };
        if n == 0 {
            return;
        }
        let from = self.snapshot_all_lines();
        let Some(session) = self.tray.session.as_mut() else {
            return;
        };
        match &mut session.view {
            TrayView::Menu { stack } => {
                if let Some(level) = stack.last_mut() {
                    level.index = wrap_index(level.index, delta, n);
                }
            }
            _ => {
                session.view = TrayView::List;
                session.index = wrap_index(session.index, delta, self.tray.items.len());
            }
        }
        let dir = if delta > 0 {
            SlideDir::Left
        } else {
            SlideDir::Right
        };
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Cross,
                    Region::Full,
                );
            }
        }
    }

    fn tray_open_menu(&mut self) {
        let Some(item) = self.tray.current().cloned() else {
            return;
        };
        let Some(session) = self.tray.session.as_mut() else {
            return;
        };
        if !matches!(session.view, TrayView::List) {
            return;
        }
        if item.menu_path.is_none() {
            session.view = TrayView::NoMenu;
            self.broadcast_frames();
            return;
        }
        session.pending_menu = Some((item.key.clone(), 0));
        if let Some(handle) = &self.tray_cmd {
            handle.get_menu(&item.key, 0);
        }
    }

    fn tray_enter(&mut self) {
        let Some(session) = self.tray.session.as_ref() else {
            return;
        };
        if session.confirm_kill {
            let pid = self.tray.current().and_then(|item| item.pid);
            if let Some(pid) = pid {
                terminate_pid(pid);
            }
            if let Some(session) = self.tray.session.as_mut() {
                session.confirm_kill = false;
            }
            self.broadcast_frames();
            return;
        }
        match &session.view {
            TrayView::NoMenu => {}
            TrayView::Menu { .. } => self.tray_menu_enter(),
            TrayView::List => self.tray_activate_current(),
        }
    }

    fn tray_menu_enter(&mut self) {
        let Some(item) = self.tray.current().cloned() else {
            return;
        };
        let Some(session) = self.tray.session.as_mut() else {
            return;
        };
        let Some(level) = session.current_menu() else {
            return;
        };
        let Some(entry) = level.entries.get(level.index).cloned() else {
            return;
        };
        if !entry.enabled {
            return;
        }
        if entry.children {
            session.pending_menu = Some((item.key.clone(), entry.id));
            if let Some(handle) = &self.tray_cmd {
                handle.get_menu(&item.key, entry.id);
            }
            return;
        }
        if let Some(handle) = &self.tray_cmd {
            handle.click(&item.key, entry.id);
        }
        self.focus_tray_item_and_close(&item);
    }

    fn tray_activate_current(&mut self) {
        let Some(item) = self.tray.current().cloned() else {
            return;
        };
        if item.item_is_menu {
            self.tray_open_menu();
            return;
        }
        if let Some(handle) = &self.tray_cmd {
            handle.activate(&item.key);
        }
        self.focus_tray_item_and_close(&item);
    }

    fn focus_tray_item_and_close(&mut self, item: &TrayItem) {
        let focused_ws = self
            .workspaces
            .iter()
            .find(|ws| ws.is_focused)
            .map(|ws| ws.id);
        if let Some(window_id) =
            crate::tray::focus_tray_window(self.windows.values(), item, focused_ws)
        {
            self.grab_window = Some(window_id);
            if let Some(niri) = self.niri.as_mut() {
                niri.focus_window(window_id);
            }
        }
        self.close_tray(true);
    }

    fn toggle_menu(&mut self) {
        if !self.config.menu.enabled {
            if self.menu.session.is_some() {
                self.close_menu(true);
            }
            return;
        }
        self.yield_toast();
        if self.menu.session.is_some() {
            self.close_menu(true);
            return;
        }
        if usable_items(&self.config.menu.items).is_empty() {
            self.show_empty_menu();
            return;
        }
        self.open_menu();
    }

    fn show_empty_menu(&mut self) {
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.notify.inbox = None;
        self.notify.empty_until = None;
        self.tray.session = None;
        self.tray.empty_until = None;
        self.menu.session = None;
        self.menu.empty_until = Some(Instant::now() + Duration::from_secs(1));
        self.sync_apps_keyboard();
        self.clear_toast_timer();
        let handle = self.handle.clone();
        self.toast_timer = handle
            .insert_source(
                Timer::from_duration(Duration::from_secs(1)),
                |_, _, daemon| {
                    daemon.toast_timer = None;
                    let from = daemon.snapshot_all_lines();
                    daemon.menu.empty_until = None;
                    daemon.animate_all(from, SlideDir::Right, Region::Full);
                    TimeoutAction::Drop
                },
            )
            .ok();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn open_menu(&mut self) {
        info!("menu overlay open");
        let from = self.snapshot_all_lines();
        self.apps = None;
        self.notify.inbox = None;
        self.notify.empty_until = None;
        self.tray.session = None;
        self.tray.empty_until = None;
        self.menu.empty_until = None;
        self.note_grab_window();
        self.menu.session = Some(MenuSession { index: 0 });
        self.sync_apps_keyboard();
        self.animate_all(from, SlideDir::Left, Region::Full);
    }

    fn close_menu(&mut self, animate: bool) {
        info!("menu overlay close");
        if self.menu.session.is_none() {
            return;
        }
        let from = if animate {
            self.snapshot_all_lines()
        } else {
            Vec::new()
        };
        self.menu.session = None;
        self.sync_apps_keyboard();
        if animate {
            self.animate_all(from, SlideDir::Right, Region::Full);
        } else {
            self.broadcast_frames();
        }
    }

    fn handle_menu_key(&mut self, name: Option<String>) {
        let Some(name) = name else {
            return;
        };
        match name.as_str() {
            "escape" => self.close_menu(true),
            "return" => self.menu_enter(),
            "left" => self.menu_step(-1),
            "right" => self.menu_step(1),
            _ => {}
        }
    }

    fn menu_step(&mut self, delta: i32) {
        let n = usable_items(&self.config.menu.items).len();
        if n == 0 {
            self.close_menu(true);
            return;
        }
        let index = match self.menu.session.as_ref() {
            Some(session) => session.index,
            None => return,
        };
        let next = crate::menu::step_index(index, delta, n);
        if next == index {
            return;
        }
        let from = self.snapshot_all_lines();
        if let Some(session) = self.menu.session.as_mut() {
            session.index = next;
        }
        let dir = if delta > 0 {
            SlideDir::Left
        } else {
            SlideDir::Right
        };
        for (id, from_line) in from {
            if let Some(to) = self.compose_live(id) {
                self.begin_transition(
                    id,
                    from_line,
                    FrozenLine::from_composed(&to),
                    dir,
                    Phase::Cross,
                    Region::Full,
                );
            }
        }
    }

    fn menu_enter(&mut self) {
        let command = self
            .menu
            .current(&self.config.menu.items)
            .map(|item| item.command.clone());
        let Some(command) = command else {
            return;
        };
        if let Err(err) = spawn_menu_command(&command) {
            warn!("menu command failed: {err}");
        }
        self.close_menu(true);
    }

    fn focus_current_notification(&mut self) {
        let Some(item) = self.notify.current().cloned() else {
            return;
        };
        let focused_ws = self
            .workspaces
            .iter()
            .find(|ws| ws.is_focused)
            .map(|ws| ws.id);
        let window_id = match_window(
            self.windows.values(),
            item.desktop_entry.as_deref(),
            item.pid,
            focused_ws,
        );
        if let Some(window_id) = window_id {
            self.grab_window = Some(window_id);
            if let Some(niri) = self.niri.as_mut() {
                niri.focus_window(window_id);
            }
            self.close_inbox(true);
        }
    }

    fn drop_client(&mut self, id: u64, remove_source: bool) {
        self.cancel_anim(id);
        self.clear_mode_timer(id);
        if let Some(mut client) = self.clients.remove(&id) {
            info!("bar client {id} disconnected");
            if remove_source {
                if let Some(token) = client.token.take() {
                    self.handle.remove(token);
                }
            }
        }
    }
}
