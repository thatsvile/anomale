use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

use calloop::channel::Sender;
use log::{info, warn};
use zbus::fdo::RequestNameFlags;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::notify::match_window;
use niri_ipc::Window;

const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_KDE: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_FDO: &str = "org.freedesktop.StatusNotifierWatcher";
const SNI_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";

#[derive(Debug, Clone)]
pub struct TrayItem {
    pub key: String,
    pub dest: String,
    pub path: String,
    pub title: String,
    pub id: String,
    pub tooltip: String,
    pub item_is_menu: bool,
    pub menu_path: Option<String>,
    pub pid: Option<i32>,
}

impl TrayItem {
    pub fn display_title(&self) -> String {
        human_tray_name(&self.title, &self.id, &self.tooltip, self.pid)
    }
}

#[derive(Debug, Clone)]
pub struct MenuEntry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub children: bool,
}

#[derive(Debug, Clone)]
pub struct MenuPropSet {
    pub type_: Option<String>,
    pub label: Option<String>,
    pub enabled: bool,
    pub visible: bool,
    pub children_display: Option<String>,
}

impl Default for MenuPropSet {
    fn default() -> Self {
        Self {
            type_: None,
            label: None,
            enabled: true,
            visible: true,
            children_display: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MenuLevel {
    pub parent_id: i32,
    pub entries: Vec<MenuEntry>,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub enum TrayView {
    List,
    Menu { stack: Vec<MenuLevel> },
    NoMenu,
}

#[derive(Debug, Clone)]
pub struct TraySession {
    pub index: usize,
    pub view: TrayView,
    pub pending_menu: Option<(String, i32)>,
    pub confirm_kill: bool,
}

impl TraySession {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            view: TrayView::List,
            pending_menu: None,
            confirm_kill: false,
        }
    }

    pub fn esc(&mut self) -> bool {
        if self.confirm_kill {
            self.confirm_kill = false;
            return false;
        }
        match &mut self.view {
            TrayView::NoMenu => {
                self.view = TrayView::List;
                self.pending_menu = None;
                false
            }
            TrayView::Menu { stack } => {
                if stack.len() > 1 {
                    stack.pop();
                } else {
                    self.view = TrayView::List;
                }
                self.pending_menu = None;
                false
            }
            TrayView::List => true,
        }
    }

    pub fn current_menu(&self) -> Option<&MenuLevel> {
        match &self.view {
            TrayView::Menu { stack } => stack.last(),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct TrayCenter {
    pub items: Vec<TrayItem>,
    pub session: Option<TraySession>,
    pub empty_until: Option<Instant>,
}

impl TrayCenter {
    pub fn line(&self) -> Option<String> {
        if let Some(until) = self.empty_until {
            if Instant::now() < until {
                return Some("no tray apps".into());
            }
        }
        let session = self.session.as_ref()?;
        overlay_line(&self.items, session)
    }

    pub fn current(&self) -> Option<&TrayItem> {
        let session = self.session.as_ref()?;
        self.items.get(session.index)
    }

    pub fn clamp_session(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if self.items.is_empty() {
            self.session = None;
            return;
        }
        session.index = session.index.min(self.items.len() - 1);
        if let TrayView::Menu { .. } | TrayView::NoMenu = session.view {
            if session
                .pending_menu
                .as_ref()
                .is_some_and(|(key, _)| self.items.iter().all(|item| item.key != *key))
            {
                session.view = TrayView::List;
                session.pending_menu = None;
            }
        }
    }
}

pub fn overlay_line(items: &[TrayItem], session: &TraySession) -> Option<String> {
    if session.confirm_kill {
        let title = items
            .get(session.index)
            .map(TrayItem::display_title)
            .unwrap_or_else(|| "app".into());
        return Some(format!("kill {title}?  enter / esc"));
    }
    match &session.view {
        TrayView::NoMenu => Some("no menu".into()),
        TrayView::List => {
            let item = items.get(session.index)?;
            Some(format!(
                "{}  {}/{}",
                item.display_title(),
                session.index + 1,
                items.len()
            ))
        }
        TrayView::Menu { stack } => {
            let item = items.get(session.index)?;
            let level = stack.last()?;
            if level.entries.is_empty() {
                return Some("no menu".into());
            }
            let entry = level.entries.get(level.index)?;
            let mark = if entry.children { " >" } else { "" };
            Some(format!(
                "{} > {}{}  {}/{}",
                item.display_title(),
                entry.label,
                mark,
                level.index + 1,
                level.entries.len()
            ))
        }
    }
}

pub fn wrap_index(index: usize, delta: i32, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (index as i32 + delta).rem_euclid(n as i32) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSpec {
    pub dest: String,
    pub path: String,
}

impl ItemSpec {
    pub fn key(&self) -> String {
        format!("{}{}", self.dest, self.path)
    }
}

pub fn parse_item_service(service: &str, sender: &str) -> ItemSpec {
    let service = service.trim();
    if service.starts_with('/') {
        return ItemSpec {
            dest: sender.to_string(),
            path: service.to_string(),
        };
    }
    if let Some((dest, path)) = service.split_once('/') {
        ItemSpec {
            dest: dest.to_string(),
            path: format!("/{path}"),
        }
    } else if service.is_empty() {
        ItemSpec {
            dest: sender.to_string(),
            path: "/StatusNotifierItem".into(),
        }
    } else {
        ItemSpec {
            dest: service.to_string(),
            path: "/StatusNotifierItem".into(),
        }
    }
}

pub fn strip_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                out.push('_');
            }
            continue;
        }
        out.push(c);
    }
    out
}

pub fn menu_entry_from_props(
    id: i32,
    type_: Option<&str>,
    label: Option<&str>,
    enabled: bool,
    visible: bool,
    children: bool,
) -> Option<MenuEntry> {
    if !visible {
        return None;
    }
    if type_.is_some_and(|ty| ty.eq_ignore_ascii_case("separator")) {
        return None;
    }
    let mut label = strip_mnemonic(label.unwrap_or("")).trim().to_string();
    if label.is_empty() {
        label = "item".into();
    }
    Some(MenuEntry {
        id,
        label,
        enabled,
        children,
    })
}

pub fn flatten_menu_level(
    nodes: impl IntoIterator<Item = (i32, MenuPropSet, bool)>,
) -> Vec<MenuEntry> {
    nodes
        .into_iter()
        .filter_map(|(id, props, has_child_nodes)| {
            let children = has_child_nodes
                || props
                    .children_display
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case("submenu"));
            menu_entry_from_props(
                id,
                props.type_.as_deref(),
                props.label.as_deref(),
                props.enabled,
                props.visible,
                children,
            )
        })
        .collect()
}

#[derive(Debug, Clone)]
pub enum TrayEvent {
    Items(Vec<TrayItem>),
    Menu {
        key: String,
        parent_id: i32,
        entries: Vec<MenuEntry>,
    },
}

enum Work {
    Register(ItemSpec),
    NameGone(String),
    Cmd(TrayCmd),
}

enum TrayCmd {
    Activate(String),
    GetMenu { key: String, parent_id: i32 },
    Click { key: String, id: i32 },
}

#[derive(Clone)]
pub struct TrayHandle {
    tx: mpsc::Sender<Work>,
}

impl TrayHandle {
    pub fn activate(&self, key: &str) {
        let _ = self.tx.send(Work::Cmd(TrayCmd::Activate(key.to_string())));
    }

    pub fn get_menu(&self, key: &str, parent_id: i32) {
        let _ = self.tx.send(Work::Cmd(TrayCmd::GetMenu {
            key: key.to_string(),
            parent_id,
        }));
    }

    pub fn click(&self, key: &str, id: i32) {
        let _ = self.tx.send(Work::Cmd(TrayCmd::Click {
            key: key.to_string(),
            id,
        }));
    }
}

struct Watcher {
    registered: Arc<Mutex<Vec<String>>>,
    work: mpsc::Sender<Work>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &mut self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        service: String,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .map(|name| name.as_str().to_string())
            .unwrap_or_default();
        let spec = parse_item_service(&service, &sender);
        if spec.dest.is_empty() {
            return Ok(());
        }
        let key = spec.key();
        {
            let mut items = self.registered.lock().unwrap();
            if !items.iter().any(|existing| existing == &key) {
                items.push(key.clone());
            }
        }
        let _ = self.work.send(Work::Register(spec));
        let _ = Watcher::status_notifier_item_registered(&emitter, &key).await;
        let _ = self
            .registered_status_notifier_items_changed(&emitter)
            .await;
        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        _service: String,
    ) -> zbus::fdo::Result<()> {
        let _ = Watcher::status_notifier_host_registered(&emitter).await;
        let _ = self
            .is_status_notifier_host_registered_changed(&emitter)
            .await;
        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registered.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

pub fn spawn_watcher(event_tx: Sender<TrayEvent>) -> TrayHandle {
    let (work_tx, work_rx) = mpsc::channel();
    let handle = TrayHandle {
        tx: work_tx.clone(),
    };
    thread::Builder::new()
        .name("vile-tray".into())
        .spawn(move || {
            if let Err(err) = run_watcher(event_tx, work_tx, work_rx) {
                warn!("status notifier watcher: {err}");
            }
        })
        .ok();
    handle
}

fn run_watcher(
    event_tx: Sender<TrayEvent>,
    work_tx: mpsc::Sender<Work>,
    work_rx: mpsc::Receiver<Work>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    let registered = Arc::new(Mutex::new(Vec::new()));
    conn.object_server().at(
        WATCHER_PATH,
        Watcher {
            registered: registered.clone(),
            work: work_tx.clone(),
        },
    )?;
    request_watcher_names(&conn);
    if let Ok(iface) = conn.object_server().interface::<_, Watcher>(WATCHER_PATH) {
        let _ = zbus::block_on(Watcher::status_notifier_host_registered(
            iface.signal_emitter(),
        ));
        let watcher = iface.get();
        let _ = zbus::block_on(
            watcher.is_status_notifier_host_registered_changed(iface.signal_emitter()),
        );
    }
    info!("status notifier watcher on {WATCHER_KDE}");
    let conn_watch = conn.clone();
    let gone_tx = work_tx;
    thread::Builder::new()
        .name("vile-tray-names".into())
        .spawn(move || watch_name_owners(conn_watch, gone_tx))
        .ok();

    let mut items: HashMap<String, TrayItem> = HashMap::new();
    while let Ok(work) = work_rx.recv() {
        match work {
            Work::Register(spec) => {
                match fetch_item(&conn, &spec) {
                    Some(item) => {
                        items.insert(item.key.clone(), item);
                    }
                    None => {
                        drop_registered(&conn, &registered, &spec.key());
                    }
                }
                publish_items(&event_tx, &items);
            }
            Work::NameGone(name) => {
                let gone: Vec<String> = items
                    .values()
                    .filter(|item| item.dest == name)
                    .map(|item| item.key.clone())
                    .collect();
                if gone.is_empty() {
                    continue;
                }
                for key in &gone {
                    items.remove(key);
                    drop_registered(&conn, &registered, key);
                }
                publish_items(&event_tx, &items);
            }
            Work::Cmd(TrayCmd::Activate(key)) => {
                if let Some(item) = items.get(&key) {
                    if let Some(proxy) = sni_proxy(&conn, item) {
                        let _: Result<(), _> = proxy.call("Activate", &(0i32, 0i32));
                    }
                }
            }
            Work::Cmd(TrayCmd::GetMenu { key, parent_id }) => {
                let entries = items
                    .get(&key)
                    .and_then(|item| load_menu(&conn, item, parent_id))
                    .unwrap_or_default();
                let _ = event_tx.send(TrayEvent::Menu {
                    key,
                    parent_id,
                    entries,
                });
            }
            Work::Cmd(TrayCmd::Click { key, id }) => {
                if let Some(item) = items.get(&key) {
                    click_menu(&conn, item, id);
                }
            }
        }
    }
    Ok(())
}

fn request_watcher_names(conn: &zbus::blocking::Connection) {
    let flags = RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue;
    for name in [WATCHER_KDE, WATCHER_FDO] {
        if let Err(err) = conn.request_name_with_flags(name, flags.into()) {
            warn!("request {name}: {err}");
        }
    }
}

fn watch_name_owners(conn: zbus::blocking::Connection, work: mpsc::Sender<Work>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
        return;
    };
    let Ok(iter) = dbus.receive_name_owner_changed() else {
        return;
    };
    for signal in iter {
        let Ok(args) = signal.args() else {
            continue;
        };
        let new_owner = args.new_owner();
        let gone = new_owner
            .as_ref()
            .map(|name| name.as_str())
            .unwrap_or("")
            .is_empty();
        if gone {
            let _ = work.send(Work::NameGone(args.name().to_string()));
        }
    }
}

fn publish_items(tx: &Sender<TrayEvent>, items: &HashMap<String, TrayItem>) {
    let mut list: Vec<TrayItem> = items.values().cloned().collect();
    list.sort_by(|a, b| {
        a.display_title()
            .cmp(&b.display_title())
            .then(a.key.cmp(&b.key))
    });
    let _ = tx.send(TrayEvent::Items(list));
}

fn drop_registered(
    conn: &zbus::blocking::Connection,
    registered: &Arc<Mutex<Vec<String>>>,
    key: &str,
) {
    {
        let mut items = registered.lock().unwrap();
        items.retain(|existing| existing != key);
    }
    let Ok(iface) = conn.object_server().interface::<_, Watcher>(WATCHER_PATH) else {
        return;
    };
    let _ = zbus::block_on(Watcher::status_notifier_item_unregistered(
        iface.signal_emitter(),
        key,
    ));
    let watcher = iface.get();
    let _ =
        zbus::block_on(watcher.registered_status_notifier_items_changed(iface.signal_emitter()));
}

fn sni_proxy(
    conn: &zbus::blocking::Connection,
    item: &TrayItem,
) -> Option<zbus::blocking::Proxy<'static>> {
    zbus::blocking::Proxy::new_owned(
        conn.clone(),
        item.dest.clone(),
        item.path.clone(),
        SNI_IFACE.to_string(),
    )
    .ok()
}

fn fetch_item(conn: &zbus::blocking::Connection, spec: &ItemSpec) -> Option<TrayItem> {
    let proxy =
        zbus::blocking::Proxy::new(conn, spec.dest.as_str(), spec.path.as_str(), SNI_IFACE).ok()?;
    let id = proxy.get_property::<String>("Id").unwrap_or_default();
    let title = proxy.get_property::<String>("Title").unwrap_or_default();
    let tooltip = tooltip_title(&proxy);
    let item_is_menu = proxy.get_property::<bool>("ItemIsMenu").unwrap_or(false);
    let menu_path = proxy
        .get_property::<zbus::zvariant::OwnedObjectPath>("Menu")
        .ok()
        .map(|path| path.to_string())
        .filter(|path| path != "/" && !path.is_empty());
    let pid = connection_pid(conn, &spec.dest);
    Some(TrayItem {
        key: spec.key(),
        dest: spec.dest.clone(),
        path: spec.path.clone(),
        title,
        id,
        tooltip,
        item_is_menu,
        menu_path,
        pid,
    })
}

type SniToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

fn tooltip_title(proxy: &zbus::blocking::Proxy<'_>) -> String {
    proxy
        .get_property::<SniToolTip>("ToolTip")
        .ok()
        .map(|tip| tip.2)
        .unwrap_or_default()
}

pub fn human_tray_name(title: &str, id: &str, tooltip: &str, pid: Option<i32>) -> String {
    for candidate in [title, tooltip, id] {
        let cleaned = clean_sni_label(candidate);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    if let Some(pid) = pid {
        if let Some(proc) = process_label(pid) {
            return proc;
        }
    }
    let fallback = clean_sni_label(id);
    if !fallback.is_empty() {
        return fallback;
    }
    let title = title.trim();
    if !title.is_empty() {
        title.into()
    } else {
        "app".into()
    }
}

pub fn clean_sni_label(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    let lower = s.to_ascii_lowercase();
    for needle in ["_status_icon", "-status-icon", "_tray_icon", "-tray-icon"] {
        if let Some(at) = lower.find(needle) {
            let head = s[..at].trim();
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    s.to_string()
}

fn is_generic_proc(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "electron" | "chromium" | "chrome" | "plugin-container"
    )
}

fn process_label(pid: i32) -> Option<String> {
    if pid <= 1 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    if !comm.is_empty() && !is_generic_proc(comm) {
        return Some(comm.to_string());
    }
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let base = exe.file_name()?.to_string_lossy();
    let base = base.trim();
    if !base.is_empty() && !is_generic_proc(base) {
        Some(base.to_string())
    } else {
        None
    }
}

fn connection_pid(conn: &zbus::blocking::Connection, dest: &str) -> Option<i32> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    let name = zbus::names::BusName::try_from(dest).ok()?;
    dbus.get_connection_unix_process_id(name)
        .ok()
        .and_then(|pid| i32::try_from(pid).ok())
}

fn menu_proxy(
    conn: &zbus::blocking::Connection,
    item: &TrayItem,
) -> Option<zbus::blocking::Proxy<'static>> {
    let path = item.menu_path.clone()?;
    zbus::blocking::Proxy::new_owned(
        conn.clone(),
        item.dest.clone(),
        path,
        MENU_IFACE.to_string(),
    )
    .ok()
}

fn load_menu(
    conn: &zbus::blocking::Connection,
    item: &TrayItem,
    parent_id: i32,
) -> Option<Vec<MenuEntry>> {
    let proxy = menu_proxy(conn, item)?;
    let _: Result<bool, _> = proxy.call("AboutToShow", &(parent_id,));
    let names: Vec<String> = Vec::new();
    let (_rev, layout): (u32, LayoutNode) =
        proxy.call("GetLayout", &(parent_id, -1i32, names)).ok()?;
    Some(entries_from_layout(&layout))
}

type LayoutNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn entries_from_layout(layout: &LayoutNode) -> Vec<MenuEntry> {
    layout
        .2
        .iter()
        .filter_map(|child| decode_layout(child.clone()))
        .filter_map(|(id, props, children)| {
            let set = props_from_map(&props);
            let has_children = !children.is_empty()
                || set
                    .children_display
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case("submenu"));
            menu_entry_from_props(
                id,
                set.type_.as_deref(),
                set.label.as_deref(),
                set.enabled,
                set.visible,
                has_children,
            )
        })
        .collect()
}

fn decode_layout(value: OwnedValue) -> Option<LayoutNode> {
    if let Ok(node) = LayoutNode::try_from(value.clone()) {
        return Some(node);
    }
    match &*value {
        zbus::zvariant::Value::Value(inner) => {
            decode_layout(OwnedValue::try_from(inner.as_ref()).ok()?)
        }
        _ => None,
    }
}

fn props_from_map(props: &HashMap<String, OwnedValue>) -> MenuPropSet {
    MenuPropSet {
        type_: prop_string(props, "type"),
        label: prop_string(props, "label"),
        enabled: prop_bool(props, "enabled", true),
        visible: prop_bool(props, "visible", true),
        children_display: prop_string(props, "children-display"),
    }
}

fn prop_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let value = props.get(key)?;
    <&str>::try_from(value)
        .ok()
        .map(str::to_string)
        .or_else(|| String::try_from(value.clone()).ok())
}

fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str, default: bool) -> bool {
    let Some(value) = props.get(key) else {
        return default;
    };
    bool::try_from(value)
        .ok()
        .or_else(|| u8::try_from(value).ok().map(|n| n != 0))
        .or_else(|| i32::try_from(value).ok().map(|n| n != 0))
        .unwrap_or(default)
}

fn click_menu(conn: &zbus::blocking::Connection, item: &TrayItem, id: i32) {
    let Some(proxy) = menu_proxy(conn, item) else {
        return;
    };
    let data = zbus::zvariant::Value::from(0i32);
    let _: Result<(), _> = proxy.call("Event", &(id, "clicked", data, 0u32));
}

pub fn terminate_pid(pid: i32) {
    if pid <= 1 {
        return;
    }
    if pid as u32 == std::process::id() {
        return;
    }
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}

pub fn focus_tray_window<'a>(
    windows: impl IntoIterator<Item = &'a Window>,
    item: &TrayItem,
    focused_workspace: Option<u64>,
) -> Option<u64> {
    let id = clean_sni_label(&item.id);
    let id = if id.is_empty() { item.id.clone() } else { id };
    match_window(
        windows,
        Some(id.as_str()).filter(|id| !id.is_empty()),
        item.pid,
        focused_workspace,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        flatten_menu_level, human_tray_name, overlay_line, parse_item_service, strip_mnemonic,
        wrap_index, ItemSpec, MenuPropSet, TrayItem, TraySession, TrayView,
    };
    use crate::notify::match_window;
    use niri_ipc::{Window, WindowLayout};

    fn item(title: &str, id: &str) -> TrayItem {
        TrayItem {
            key: format!("{id}/StatusNotifierItem"),
            dest: id.into(),
            path: "/StatusNotifierItem".into(),
            title: title.into(),
            id: id.into(),
            tooltip: String::new(),
            item_is_menu: false,
            menu_path: None,
            pid: Some(42),
        }
    }

    fn win(id: u64, app_id: &str, pid: Option<i32>) -> Window {
        Window {
            id,
            title: Some(app_id.into()),
            app_id: Some(app_id.into()),
            pid,
            workspace_id: Some(1),
            is_focused: false,
            is_floating: false,
            is_urgent: false,
            layout: WindowLayout {
                pos_in_scrolling_layout: None,
                tile_size: (0.0, 0.0),
                window_size: (0, 0),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0.0, 0.0),
            },
            focus_timestamp: None,
        }
    }

    #[test]
    fn parses_service_name_and_path() {
        let from_path = parse_item_service("/StatusNotifierItem", ":1.9");
        assert_eq!(from_path.dest, ":1.9");
        assert_eq!(from_path.path, "/StatusNotifierItem");
        let unique = parse_item_service(":1.42/org/ayatana/NotificationItem/nm", ":1.1");
        assert_eq!(unique.dest, ":1.42");
        assert_eq!(unique.path, "/org/ayatana/NotificationItem/nm");
        let well_known = parse_item_service("org.kde.StatusNotifierItem-12-1", ":1.8");
        assert_eq!(
            well_known,
            ItemSpec {
                dest: "org.kde.StatusNotifierItem-12-1".into(),
                path: "/StatusNotifierItem".into(),
            }
        );
    }

    #[test]
    fn flatten_skips_separators_and_hidden() {
        let entries = flatten_menu_level([
            (
                1,
                MenuPropSet {
                    type_: Some("separator".into()),
                    visible: true,
                    enabled: true,
                    ..MenuPropSet::default()
                },
                false,
            ),
            (
                2,
                MenuPropSet {
                    label: Some("_Mute".into()),
                    visible: true,
                    enabled: true,
                    ..MenuPropSet::default()
                },
                false,
            ),
            (
                3,
                MenuPropSet {
                    label: Some("Hidden".into()),
                    visible: false,
                    enabled: true,
                    ..MenuPropSet::default()
                },
                false,
            ),
            (
                4,
                MenuPropSet {
                    label: Some("_More".into()),
                    visible: true,
                    enabled: true,
                    children_display: Some("submenu".into()),
                    ..MenuPropSet::default()
                },
                false,
            ),
        ]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "Mute");
        assert!(!entries[0].children);
        assert_eq!(entries[1].label, "More");
        assert!(entries[1].children);
    }

    #[test]
    fn wrap_and_confirm_kill_cancel() {
        assert_eq!(wrap_index(0, 1, 3), 1);
        assert_eq!(wrap_index(2, 1, 3), 0);
        assert_eq!(wrap_index(0, -1, 3), 2);
        let items = vec![item("Discord", "discord"), item("Wire", "nm-applet")];
        let mut session = TraySession::new(0);
        session.index = wrap_index(session.index, 1, items.len());
        assert_eq!(session.index, 1);
        session.confirm_kill = true;
        assert!(!session.esc());
        assert!(matches!(session.view, TrayView::List));
        assert!(!session.confirm_kill);
        assert_eq!(overlay_line(&items, &session).as_deref(), Some("Wire  2/2"));
        session.confirm_kill = true;
        assert_eq!(
            overlay_line(&items, &session).as_deref(),
            Some("kill Wire?  enter / esc")
        );
        assert!(!session.esc());
        assert!(session.esc());
    }

    #[test]
    fn confirm_kill_from_menu_returns_to_menu() {
        let items = vec![item("vesktop", "vesktop_status_icon_1")];
        let mut session = TraySession::new(0);
        session.view = TrayView::Menu {
            stack: vec![super::MenuLevel {
                parent_id: 0,
                entries: vec![super::MenuEntry {
                    id: 1,
                    label: "Quit".into(),
                    enabled: true,
                    children: false,
                }],
                index: 0,
            }],
        };
        session.confirm_kill = true;
        assert_eq!(
            overlay_line(&items, &session).as_deref(),
            Some("kill vesktop?  enter / esc")
        );
        assert!(!session.esc());
        assert!(matches!(session.view, TrayView::Menu { .. }));
        assert_eq!(
            overlay_line(&items, &session).as_deref(),
            Some("vesktop > Quit  1/1")
        );
    }

    #[test]
    fn strips_status_icon_ids() {
        assert_eq!(
            human_tray_name("", "vesktop_status_icon_1", "", None),
            "vesktop"
        );
        assert_eq!(
            human_tray_name("vesktop_status_icon_1", "vesktop_status_icon_1", "", None),
            "vesktop"
        );
        assert_eq!(
            human_tray_name("Vesktop", "vesktop_status_icon_1", "", None),
            "Vesktop"
        );
        assert_eq!(
            human_tray_name("", "chrome_status_icon_2", "Chrome", None),
            "Chrome"
        );
    }

    #[test]
    fn match_window_uses_tray_pid_and_id() {
        let windows = vec![win(1, "firefox", Some(10)), win(2, "nm-applet", Some(99))];
        assert_eq!(
            match_window(&windows, Some("nm-applet"), Some(99), Some(1)),
            Some(2)
        );
        assert_eq!(
            match_window(&windows, Some("nm-applet"), None, Some(1)),
            Some(2)
        );
    }

    #[test]
    fn clamp_session_drops_when_empty() {
        let mut tray = super::TrayCenter {
            items: vec![item("WirePlumber", "nm-applet")],
            session: Some(TraySession::new(0)),
            empty_until: None,
        };
        tray.clamp_session();
        assert!(tray.session.is_some());
        tray.items.clear();
        tray.clamp_session();
        assert!(tray.session.is_none());
    }

    #[test]
    fn mnemonic_and_line_copy() {
        assert_eq!(strip_mnemonic("_Open __file"), "Open _file");
        let items = vec![item("WirePlumber", "nm-applet")];
        let mut session = TraySession::new(0);
        session.view = TrayView::Menu {
            stack: vec![super::MenuLevel {
                parent_id: 0,
                entries: vec![
                    super::MenuEntry {
                        id: 1,
                        label: "Mute".into(),
                        enabled: true,
                        children: false,
                    },
                    super::MenuEntry {
                        id: 2,
                        label: "More".into(),
                        enabled: true,
                        children: true,
                    },
                ],
                index: 1,
            }],
        };
        assert_eq!(session.current_menu().unwrap().parent_id, 0);
        assert_eq!(
            overlay_line(&items, &session).as_deref(),
            Some("WirePlumber > More >  2/2")
        );
    }
}
