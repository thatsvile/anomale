use async_channel::{Receiver, Sender};
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Label, ListBox, ListBoxRow, Orientation, SelectionMode,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy, SignalContext};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DBUSMENU_INTERFACE: &str = "com.canonical.dbusmenu";
const MAX_MENU_ITEMS: usize = 512;
const MAX_MENU_DEPTH: i32 = 8;
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(600);

#[derive(Clone, Debug, PartialEq)]
pub struct TrayItem {
    pub id: String,
    pub service: String,
    pub path: String,
    pub owner: String,
    pub owner_pid: Option<u32>,
    pub title: String,
    pub menu_path: Option<String>,
}

#[derive(Clone, Debug)]
struct Endpoint {
    service: String,
    path: String,
    owner: String,
    menu_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MangoClients {
    pids: HashSet<u32>,
    app_ids: HashSet<String>,
}

#[derive(Debug)]
pub enum TrayCommand {
    Activate(String),
    Exit(String),
}

#[derive(Default)]
struct WatcherState {
    items: Mutex<HashMap<String, String>>,
    host_registered: Mutex<bool>,
}

struct StatusNotifierWatcher {
    state: Arc<WatcherState>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcher {
    async fn register_status_notifier_item(
        &self,
        service: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .map(|name| name.as_str().to_string())
            .ok_or_else(|| zbus::fdo::Error::Failed("Registration has no D-Bus sender".into()))?;
        let (canonical, owner) = normalize_registration(&service, &sender);
        let inserted = self
            .state
            .items
            .lock()
            .await
            .insert(canonical.clone(), owner)
            .is_none();
        if inserted {
            Self::status_notifier_item_registered(&ctxt, &canonical).await?;
        }
        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        _service: String,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        let mut registered = self.state.host_registered.lock().await;
        if !*registered {
            *registered = true;
            Self::status_notifier_host_registered(&ctxt).await?;
        }
        Ok(())
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        let mut items: Vec<_> = self.state.items.lock().await.keys().cloned().collect();
        items.sort();
        items
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        *self.state.host_registered.lock().await
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        ctxt: &SignalContext<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        ctxt: &SignalContext<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

pub async fn run(
    updates: Sender<Vec<TrayItem>>,
    commands: Receiver<TrayCommand>,
) -> anyhow::Result<()> {
    let connection = Connection::session().await?;
    let state = Arc::new(WatcherState::default());
    connection
        .object_server()
        .at(
            WATCHER_PATH,
            StatusNotifierWatcher {
                state: state.clone(),
            },
        )
        .await?;

    let host_name = format!("org.kde.StatusNotifierHost.anomale-{}", std::process::id());
    connection.request_name(host_name.as_str()).await?;

    let mut endpoints: HashMap<String, Endpoint> = HashMap::new();
    let mut last_items = Vec::new();
    let (mango_tx, mango_rx) = async_channel::unbounded();
    tokio::spawn(watch_mango_clients(mango_tx));
    let mut mango_clients = MangoClients::default();
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(error) = refresh(
                    &connection,
                    &state,
                    &host_name,
                    &updates,
                    &mango_clients,
                    &mut endpoints,
                    &mut last_items,
                ).await {
                    eprintln!("Tray refresh failed: {error}");
                }
            }
            clients = mango_rx.recv() => {
                if let Ok(clients) = clients {
                    mango_clients = clients;
                    if let Err(error) = refresh(
                        &connection,
                        &state,
                        &host_name,
                        &updates,
                        &mango_clients,
                        &mut endpoints,
                        &mut last_items,
                    ).await {
                        eprintln!("Tray refresh after MangoWM update failed: {error}");
                    }
                }
            }
            command = commands.recv() => {
                let Ok(command) = command else { break };
                if let Err(error) = handle_command(&connection, &endpoints, command).await {
                    eprintln!("Tray action failed: {error}");
                }
            }
        }
    }
    Ok(())
}

async fn refresh(
    connection: &Connection,
    state: &Arc<WatcherState>,
    host_name: &str,
    updates: &Sender<Vec<TrayItem>>,
    mango_clients: &MangoClients,
    endpoints: &mut HashMap<String, Endpoint>,
    last_items: &mut Vec<TrayItem>,
) -> anyhow::Result<()> {
    let ownership = connection
        .request_name_with_flags(WATCHER_NAME, RequestNameFlags::DoNotQueue.into())
        .await?;
    let owns_watcher = matches!(
        ownership,
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner
    );

    let watcher = Proxy::new(connection, WATCHER_NAME, WATCHER_PATH, WATCHER_INTERFACE).await?;
    let _: Result<(), _> = watcher
        .call("RegisterStatusNotifierHost", &(host_name.to_string()))
        .await;

    if owns_watcher {
        remove_stale_registrations(connection, state).await;
    }

    let registrations: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .await
        .unwrap_or_default();

    let mut next_items = Vec::new();
    let mut next_endpoints = HashMap::new();
    for registration in registrations {
        let Some((service, path)) = split_registration(&registration) else {
            continue;
        };
        if let Some(item) = read_item(connection, &service, &path).await {
            if item_has_mango_tile(&item, mango_clients) {
                continue;
            }
            next_endpoints.insert(
                item.id.clone(),
                Endpoint {
                    service: item.service.clone(),
                    path: item.path.clone(),
                    owner: item.owner.clone(),
                    menu_path: item.menu_path.clone(),
                },
            );
            next_items.push(item);
        }
    }
    next_items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

    *endpoints = next_endpoints;
    if *last_items != next_items {
        *last_items = next_items.clone();
        let _ = updates.send(next_items).await;
    }
    Ok(())
}

async fn remove_stale_registrations(connection: &Connection, state: &Arc<WatcherState>) {
    let Ok(dbus) = DBusProxy::new(connection).await else {
        return;
    };
    let entries: Vec<(String, String)> = state
        .items
        .lock()
        .await
        .iter()
        .map(|(item, owner)| (item.clone(), owner.clone()))
        .collect();
    let mut stale = Vec::new();
    for (item, owner) in entries {
        let Ok(name) = BusName::try_from(owner.as_str()) else {
            stale.push(item);
            continue;
        };
        if !dbus.name_has_owner(name).await.unwrap_or(false) {
            stale.push(item);
        }
    }
    if stale.is_empty() {
        return;
    }
    let mut items = state.items.lock().await;
    for item in stale {
        items.remove(&item);
        if let Ok(ctxt) = SignalContext::new(connection, WATCHER_PATH) {
            let _ = StatusNotifierWatcher::status_notifier_item_unregistered(&ctxt, &item).await;
        }
    }
}

async fn read_item(connection: &Connection, service: &str, path: &str) -> Option<TrayItem> {
    let proxy = tokio::time::timeout(
        Duration::from_secs(2),
        Proxy::new(connection, service, path, ITEM_INTERFACE),
    )
    .await
    .ok()?
    .ok()?;

    let dbus = DBusProxy::new(connection).await.ok()?;
    let bus_name = BusName::try_from(service).ok()?;
    let owner = dbus
        .get_name_owner(bus_name)
        .await
        .map(|name| name.to_string())
        .unwrap_or_else(|_| service.to_string());
    let owner_pid = match BusName::try_from(owner.as_str()) {
        Ok(name) => dbus.get_connection_unix_process_id(name).await.ok(),
        Err(_) => None,
    };

    let raw_title = get_property_timeout::<String>(&proxy, "Title")
        .await
        .unwrap_or_default();
    let item_id = get_property_timeout::<String>(&proxy, "Id")
        .await
        .unwrap_or_default();
    let desktop_entry = get_property_timeout::<String>(&proxy, "DesktopEntry")
        .await
        .unwrap_or_default();
    let title = resolve_item_title(&raw_title, &item_id, &desktop_entry, owner_pid, service);
    let menu_path = get_property_timeout::<OwnedObjectPath>(&proxy, "Menu")
        .await
        .map(|path| path.to_string())
        .filter(|path| path != "/");

    Some(TrayItem {
        id: format!("{service}{path}"),
        service: service.to_string(),
        path: path.to_string(),
        owner,
        owner_pid,
        title,
        menu_path,
    })
}

async fn get_property_timeout<T>(proxy: &Proxy<'_>, property: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    tokio::time::timeout(Duration::from_secs(2), proxy.get_property(property))
        .await
        .ok()?
        .ok()
}

async fn handle_command(
    connection: &Connection,
    endpoints: &HashMap<String, Endpoint>,
    command: TrayCommand,
) -> anyhow::Result<()> {
    let id = match &command {
        TrayCommand::Activate(id) | TrayCommand::Exit(id) => id,
    };
    let Some(endpoint) = endpoints.get(id) else {
        return Ok(());
    };

    match command {
        TrayCommand::Activate(_) => {
            let proxy = Proxy::new(
                connection,
                endpoint.service.as_str(),
                endpoint.path.as_str(),
                ITEM_INTERFACE,
            )
            .await?;
            tokio::time::timeout(
                Duration::from_secs(3),
                proxy.call::<_, _, ()>("Activate", &(0i32, 0i32)),
            )
            .await??;
        }
        TrayCommand::Exit(_) => {
            if !invoke_native_quit(connection, endpoint).await {
                terminate_owner(connection, &endpoint.owner).await?;
            }
        }
    }
    Ok(())
}

async fn invoke_native_quit(connection: &Connection, endpoint: &Endpoint) -> bool {
    let Some(menu_path) = endpoint.menu_path.as_deref() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        connection,
        endpoint.service.as_str(),
        menu_path,
        DBUSMENU_INTERFACE,
    )
    .await
    else {
        return false;
    };

    let _: Result<bool, _> = proxy.call("AboutToShow", &(0i32)).await;
    let properties = vec![
        "label".to_string(),
        "enabled".to_string(),
        "visible".to_string(),
    ];
    type MenuLayout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
    let Ok(Ok((_revision, layout))) = tokio::time::timeout(
        Duration::from_secs(2),
        proxy.call::<_, _, (u32, MenuLayout)>("GetLayout", &(0i32, MAX_MENU_DEPTH, properties)),
    )
    .await
    else {
        return false;
    };
    let Some(item_id) = find_quit_item(&layout) else {
        return false;
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        proxy.call::<_, _, ()>("Event", &(item_id, "clicked", OwnedValue::from(0u32), 0u32)),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false)
}

async fn terminate_owner(connection: &Connection, owner: &str) -> anyhow::Result<()> {
    let dbus = DBusProxy::new(connection).await?;
    let name = BusName::try_from(owner)?;
    let pid = dbus.get_connection_unix_process_id(name).await?;
    if pid == 0 || pid == std::process::id() {
        anyhow::bail!("refusing to terminate invalid tray owner PID {pid}");
    }
    kill(Pid::from_raw(pid as i32), Signal::SIGTERM)?;
    Ok(())
}

type MenuLayout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn find_quit_item(layout: &MenuLayout) -> Option<i32> {
    fn visit(layout: &MenuLayout, seen: &mut usize) -> Option<i32> {
        if *seen >= MAX_MENU_ITEMS {
            return None;
        }
        *seen += 1;
        let enabled = layout
            .1
            .get("enabled")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(true);
        let visible = layout
            .1
            .get("visible")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(true);
        let label = layout
            .1
            .get("label")
            .and_then(|value| <&str>::try_from(value).ok())
            .map(str::to_owned)
            .unwrap_or_default()
            .replace('_', "")
            .trim()
            .to_lowercase();
        if enabled
            && visible
            && (label == "quit"
                || label == "exit"
                || label.starts_with("quit ")
                || label.starts_with("exit "))
        {
            return Some(layout.0);
        }
        for child in &layout.2 {
            if let Ok(value) = child.try_clone() {
                if let Ok(child_layout) = MenuLayout::try_from(value) {
                    if let Some(id) = visit(&child_layout, seen) {
                        return Some(id);
                    }
                }
            }
        }
        None
    }
    visit(layout, &mut 0)
}

fn normalize_registration(service: &str, sender: &str) -> (String, String) {
    if service.starts_with('/') {
        (format!("{sender}{service}"), sender.to_string())
    } else {
        (format!("{service}/StatusNotifierItem"), service.to_string())
    }
}

fn split_registration(registration: &str) -> Option<(String, String)> {
    let slash = registration.find('/')?;
    let service = registration[..slash].to_string();
    let path = registration[slash..].to_string();
    if service.is_empty() || path.is_empty() {
        None
    } else {
        Some((service, path))
    }
}

fn friendly_service_name(service: &str) -> String {
    service
        .rsplit('.')
        .next()
        .unwrap_or(service)
        .trim_start_matches(':')
        .to_string()
}

#[derive(serde::Deserialize)]
struct MangoClientList {
    clients: Vec<MangoClient>,
}

#[derive(serde::Deserialize)]
struct MangoClient {
    pid: u32,
    appid: String,
    #[serde(default)]
    is_minimized: bool,
}

fn parse_mango_clients(raw: &str) -> Option<MangoClients> {
    let snapshot: MangoClientList = serde_json::from_str(raw).ok()?;
    let mut clients = MangoClients::default();
    for client in snapshot.clients {
        if client.is_minimized {
            continue;
        }
        clients.pids.insert(client.pid);
        let app_id = normalize_app_identity(&client.appid);
        if !app_id.is_empty() {
            clients.app_ids.insert(app_id);
        }
    }
    Some(clients)
}

async fn watch_mango_clients(sender: Sender<MangoClients>) {
    loop {
        if let Ok(Ok(output)) = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new("mmsg")
                .args(["get", "all-clients"])
                .output(),
        )
        .await
        {
            if let Some(clients) = parse_mango_clients(&String::from_utf8_lossy(&output.stdout)) {
                if sender.send(clients).await.is_err() {
                    return;
                }
            }
        }

        let child = tokio::process::Command::new("mmsg")
            .args(["watch", "all-clients"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let Some(stdout) = child.stdout.take() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let mut lines = BufReader::new(stdout).lines();
        let mut pending = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            pending.push_str(line.trim());
            if let Some(clients) = parse_mango_clients(&pending) {
                pending.clear();
                if sender.send(clients).await.is_err() {
                    let _ = child.kill().await;
                    return;
                }
            } else if pending.len() > 1024 * 1024 {
                pending.clear();
            }
        }
        let _ = child.wait().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn item_has_mango_tile(item: &TrayItem, clients: &MangoClients) -> bool {
    if let Some(owner_pid) = item.owner_pid {
        if clients.pids.contains(&owner_pid)
            || clients.pids.iter().any(|client_pid| {
                process_is_ancestor(owner_pid, *client_pid)
                    || process_is_ancestor(*client_pid, owner_pid)
            })
        {
            return true;
        }
    }
    let title = normalize_app_identity(&item.title);
    !title.is_empty() && clients.app_ids.contains(&title)
}

fn process_is_ancestor(ancestor: u32, mut process: u32) -> bool {
    for _ in 0..16 {
        if process == ancestor {
            return true;
        }
        if process <= 1 {
            return false;
        }
        let Ok(status) = std::fs::read_to_string(format!("/proc/{process}/status")) else {
            return false;
        };
        let Some(parent) = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|value| value.trim().parse::<u32>().ok())
        else {
            return false;
        };
        process = parent;
    }
    false
}

fn normalize_app_identity(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn resolve_item_title(
    title: &str,
    item_id: &str,
    desktop_entry: &str,
    owner_pid: Option<u32>,
    service: &str,
) -> String {
    if !title.trim().is_empty() && !is_generic_tray_name(title) {
        return title.trim().to_string();
    }
    if !desktop_entry.trim().is_empty() {
        return pretty_identifier(desktop_entry);
    }
    if let Some(name) = owner_pid.and_then(process_display_name) {
        return name;
    }
    if !item_id.trim().is_empty() && !is_generic_tray_name(item_id) {
        return pretty_identifier(item_id);
    }
    friendly_service_name(service)
}

fn is_generic_tray_name(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("chrome_status_icon_")
        || value.starts_with("chromium_status_icon_")
        || value == "statusnotifieritem"
}

fn process_display_name(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let arguments: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    display_name_from_commandline(&arguments)
}

fn display_name_from_commandline(arguments: &[String]) -> Option<String> {
    for argument in arguments {
        let path = std::path::Path::new(argument);
        if path.file_name().and_then(|name| name.to_str()) == Some("app.asar") {
            if let Some(parent) = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
            {
                return Some(pretty_identifier(parent));
            }
        }
    }

    let executable = arguments
        .first()
        .and_then(|argument| std::path::Path::new(argument).file_stem())
        .and_then(|name| name.to_str())?;
    let generic_executables = [
        "electron",
        "electron-bin",
        "chrome",
        "chromium",
        "chromium-browser",
    ];
    if generic_executables
        .iter()
        .any(|generic| executable.eq_ignore_ascii_case(generic))
    {
        None
    } else {
        Some(pretty_identifier(executable))
    }
}

fn pretty_identifier(value: &str) -> String {
    value
        .trim_end_matches(".desktop")
        .split(|character: char| character == '-' || character == '_' || character == '.')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyboardAction {
    Open,
    Exit,
}

struct PendingTap {
    action: KeyboardAction,
    started: Instant,
    released: bool,
}

pub struct TrayMenu {
    window: ApplicationWindow,
    list_box: ListBox,
    item_ids: RefCell<Vec<Option<String>>>,
    pending_tap: RefCell<Option<PendingTap>>,
    command_tx: Sender<TrayCommand>,
}

impl TrayMenu {
    pub fn new(
        app: &Application,
        css_provider: &gtk4::CssProvider,
        command_tx: Sender<TrayCommand>,
    ) -> Rc<RefCell<Self>> {
        let config = crate::config::AppConfig::load().unwrap_or_default();
        css_provider.load_from_data(&config.generate_css(None));

        let window = ApplicationWindow::builder()
            .application(app)
            .title("Anomale System Tray")
            .decorated(false)
            .visible(false)
            .build();
        window.init_layer_shell();
        window.set_namespace("anomale-tray");
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_exclusive_zone(-1);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.add_css_class("action-menu-window");

        let overlay = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .halign(Align::Center)
            .valign(Align::Center)
            .build();
        let content = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .width_request(config.search_width + 10)
            .build();
        content.add_css_class("launcher-box");
        let list_box = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        list_box.add_css_class("app-list");
        content.append(&list_box);
        overlay.append(&content);
        window.set_child(Some(&overlay));

        let menu = Rc::new(RefCell::new(Self {
            window,
            list_box,
            item_ids: RefCell::new(Vec::new()),
            pending_tap: RefCell::new(None),
            command_tx,
        }));

        let menu_activate = menu.clone();
        menu.borrow().list_box.connect_row_activated(move |_, row| {
            let menu = menu_activate.borrow();
            let Some(Some(id)) = menu.item_ids.borrow().get(row.index() as usize).cloned() else {
                return;
            };
            let _ = menu.command_tx.try_send(TrayCommand::Activate(id));
            menu.window.set_visible(false);
        });

        let key = gtk4::EventControllerKey::new();
        key.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let menu_key = menu.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            let menu = menu_key.borrow();
            match key {
                key if key == gtk4::gdk::Key::Escape => {
                    *menu.pending_tap.borrow_mut() = None;
                    menu.window.set_visible(false);
                    gtk4::glib::Propagation::Stop
                }
                key if key == gtk4::gdk::Key::Down || key == gtk4::gdk::Key::Up => {
                    *menu.pending_tap.borrow_mut() = None;
                    let item_count = menu.item_ids.borrow().len();
                    if item_count == 0 {
                        return gtk4::glib::Propagation::Stop;
                    }
                    let current = menu.list_box.selected_row().map(|row| row.index());
                    let target = if key == gtk4::gdk::Key::Down {
                        current.map_or(0, |index| (index + 1).min(item_count as i32 - 1))
                    } else {
                        current.map_or(item_count as i32 - 1, |index| (index - 1).max(0))
                    };
                    if menu
                        .item_ids
                        .borrow()
                        .get(target as usize)
                        .and_then(|id| id.as_ref())
                        .is_some()
                    {
                        if let Some(row) = menu.list_box.row_at_index(target) {
                            menu.list_box.select_row(Some(&row));
                            row.grab_focus();
                        }
                    }
                    gtk4::glib::Propagation::Stop
                }
                key if key == gtk4::gdk::Key::Right || key == gtk4::gdk::Key::Left => {
                    let action = if key == gtk4::gdk::Key::Right {
                        KeyboardAction::Open
                    } else {
                        KeyboardAction::Exit
                    };
                    let Some(row) = menu.list_box.selected_row() else {
                        return gtk4::glib::Propagation::Stop;
                    };
                    let Some(Some(id)) = menu.item_ids.borrow().get(row.index() as usize).cloned()
                    else {
                        return gtk4::glib::Propagation::Stop;
                    };

                    let now = Instant::now();
                    let execute = {
                        let pending = menu.pending_tap.borrow();
                        pending.as_ref().is_some_and(|pending| {
                            pending.action == action
                                && pending.released
                                && now.duration_since(pending.started) <= DOUBLE_TAP_WINDOW
                        })
                    };
                    if execute {
                        *menu.pending_tap.borrow_mut() = None;
                        let command = match action {
                            KeyboardAction::Open => TrayCommand::Activate(id),
                            KeyboardAction::Exit => TrayCommand::Exit(id),
                        };
                        let _ = menu.command_tx.try_send(command);
                        menu.window.set_visible(false);
                    } else {
                        let mut pending = menu.pending_tap.borrow_mut();
                        if pending.as_ref().is_none_or(|pending| {
                            pending.action != action
                                || pending.released
                                || now.duration_since(pending.started) > DOUBLE_TAP_WINDOW
                        }) {
                            *pending = Some(PendingTap {
                                action,
                                started: now,
                                released: false,
                            });
                        }
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => {
                    *menu.pending_tap.borrow_mut() = None;
                    gtk4::glib::Propagation::Proceed
                }
            }
        });
        let menu_key_release = menu.clone();
        key.connect_key_released(move |_, key, _, _| {
            let action = if key == gtk4::gdk::Key::Right {
                Some(KeyboardAction::Open)
            } else if key == gtk4::gdk::Key::Left {
                Some(KeyboardAction::Exit)
            } else {
                None
            };
            if let Some(action) = action {
                if let Some(pending) = menu_key_release.borrow().pending_tap.borrow_mut().as_mut() {
                    if pending.action == action {
                        pending.released = true;
                    }
                }
            }
        });
        menu.borrow().window.add_controller(key);

        let click = gtk4::GestureClick::new();
        let menu_click = menu.clone();
        click.connect_released(move |_, _, x, y| {
            let menu = menu_click.borrow();
            if let Some(overlay) = menu.window.child() {
                if let Some(content) = overlay.first_child() {
                    let allocation = content.allocation();
                    let left = allocation.x() as f64;
                    let top = allocation.y() as f64;
                    let right = left + allocation.width() as f64;
                    let bottom = top + allocation.height() as f64;
                    if x < left || x > right || y < top || y > bottom {
                        menu.window.set_visible(false);
                    }
                }
            }
        });
        menu.borrow().window.add_controller(click);
        menu.borrow().update(Vec::new());
        menu
    }

    pub fn update(&self, items: Vec<TrayItem>) {
        while let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.remove(&row);
        }
        let mut ids = self.item_ids.borrow_mut();
        ids.clear();

        if items.is_empty() {
            let row = ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&Label::new(Some("No tray applications"))));
            self.list_box.append(&row);
            ids.push(None);
            return;
        }

        for item in items {
            let row = ListBoxRow::new();
            let row_box = gtk4::Box::builder()
                .orientation(Orientation::Horizontal)
                .halign(Align::Center)
                .build();

            let label = Label::new(Some(&item.title));
            label.set_xalign(0.5);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row_box.append(&label);

            row.set_child(Some(&row_box));
            self.list_box.append(&row);
            ids.push(Some(item.id));
        }
    }

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            *self.pending_tap.borrow_mut() = None;
            self.window.set_visible(true);
            self.list_box.unselect_all();
            self.list_box.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_both_registration_forms() {
        assert_eq!(
            normalize_registration("/StatusNotifierItem", ":1.42"),
            (":1.42/StatusNotifierItem".to_string(), ":1.42".to_string())
        );
        assert_eq!(
            normalize_registration("org.example.Tray", ":1.42"),
            (
                "org.example.Tray/StatusNotifierItem".to_string(),
                "org.example.Tray".to_string()
            )
        );
    }

    #[test]
    fn finds_enabled_quit_entry() {
        let mut props = HashMap::new();
        props.insert(
            "label".to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::from("_Quit Discord")).unwrap(),
        );
        let root: MenuLayout = (9, props, Vec::new());
        assert_eq!(find_quit_item(&root), Some(9));
    }

    #[test]
    fn derives_vesktop_name_from_electron_commandline() {
        let arguments = vec![
            "/usr/lib/electron40/electron".to_string(),
            "/usr/lib/vesktop/app.asar".to_string(),
        ];
        assert_eq!(
            display_name_from_commandline(&arguments),
            Some("Vesktop".to_string())
        );
    }

    #[test]
    fn generic_chromium_title_uses_desktop_entry() {
        assert_eq!(
            resolve_item_title("", "chrome_status_icon_1", "discord.desktop", None, ":1.42"),
            "Discord"
        );
    }

    #[test]
    fn mango_snapshot_excludes_minimized_clients() {
        let clients = parse_mango_clients(
            r#"{"clients":[
                {"pid":10,"appid":"vesktop","is_minimized":false},
                {"pid":20,"appid":"steam","is_minimized":true}
            ]}"#,
        )
        .unwrap();
        assert!(clients.pids.contains(&10));
        assert!(clients.app_ids.contains("vesktop"));
        assert!(!clients.pids.contains(&20));
        assert!(!clients.app_ids.contains("steam"));
    }
}
