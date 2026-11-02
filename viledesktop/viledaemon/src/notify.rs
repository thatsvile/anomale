use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use calloop::channel::Sender;
use log::warn;
use niri_ipc::Window;

#[derive(Debug, Clone)]
pub struct NotifyItem {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub desktop_entry: Option<String>,
    pub pid: Option<i32>,
    pub urgency: u8,
    pub expire_timeout_ms: Option<i32>,
}

impl NotifyItem {
    pub fn toast_text(&self) -> String {
        let summary = collapse_ws(&self.summary);
        let body = collapse_ws(&self.body);
        if !summary.is_empty() && !body.is_empty() && !eq_ignore_ascii(&summary, &body) {
            format!("{summary}  {body}")
        } else if !summary.is_empty() {
            summary
        } else if !body.is_empty() {
            body
        } else {
            let app = self.app_name.trim();
            if is_tool_app(app) {
                String::new()
            } else {
                app.to_string()
            }
        }
    }

    pub fn inbox_text(&self, index: usize, total: usize) -> String {
        let n = format!("{}/{}", index + 1, total);
        let mut text = self.toast_text();
        if self.urgency >= 2 {
            text = if text.is_empty() {
                "!".into()
            } else {
                format!("! {text}")
            };
        }
        if text.is_empty() {
            n
        } else {
            format!("{text}  {n}")
        }
    }
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn eq_ignore_ascii(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn is_tool_app(app: &str) -> bool {
    matches!(
        app.to_ascii_lowercase().as_str(),
        "" | "notify-send" | "gdbus" | "busctl" | "dbus-send" | "notify" | "unknown"
    )
}

#[derive(Debug, Clone)]
pub enum NotifyEvent {
    Notify(NotifyItem),
    Close(u32),
}

#[derive(Debug, Clone)]
pub struct ToastState {
    pub id: u32,
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct InboxState {
    pub index: usize,
    pub confirm_clear: bool,
}

#[derive(Debug, Default)]
pub struct NotifyCenter {
    pub items: Vec<NotifyItem>,
    pub toast: Option<ToastState>,
    pub inbox: Option<InboxState>,
    pub empty_until: Option<Instant>,
}

impl NotifyCenter {
    pub fn upsert(&mut self, item: NotifyItem, cap: usize) {
        if let Some(existing) = self.items.iter_mut().find(|it| it.id == item.id) {
            *existing = item;
            return;
        }
        self.items.push(item);
        let cap = cap.max(1);
        while self.items.len() > cap {
            self.items.remove(0);
        }
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        if let Some(inbox) = self.inbox.as_mut() {
            if self.items.is_empty() {
                self.inbox = None;
            } else {
                inbox.index = inbox.index.min(self.items.len() - 1);
            }
        }
        before != self.items.len()
    }

    pub fn current(&self) -> Option<&NotifyItem> {
        let inbox = self.inbox.as_ref()?;
        self.items.get(inbox.index)
    }
}

struct Notifications {
    tx: Sender<NotifyEvent>,
    next_id: Arc<AtomicU32>,
}

pub fn spawn_server(tx: Sender<NotifyEvent>) {
    thread::Builder::new()
        .name("vile-notify".into())
        .spawn(move || {
            if let Err(err) = run_server(tx) {
                warn!("notification bus: {err}");
            }
        })
        .ok();
}

fn run_server(tx: Sender<NotifyEvent>) -> Result<(), Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    conn.object_server().at(
        "/org/freedesktop/Notifications",
        Notifications {
            tx,
            next_id: Arc::new(AtomicU32::new(1)),
        },
    )?;
    conn.request_name("org.freedesktop.Notifications")?;
    loop {
        thread::park();
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id == 0 {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            replaces_id
        };
        let item = NotifyItem {
            id,
            app_name,
            summary,
            body,
            desktop_entry: hint_string(&hints, "desktop-entry"),
            pid: hint_i32(&hints, "sender-pid"),
            urgency: hint_u8(&hints, "urgency").unwrap_or(1),
            expire_timeout_ms: (expire_timeout > 0).then_some(expire_timeout),
        };
        let _ = self.tx.send(NotifyEvent::Notify(item));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.tx.send(NotifyEvent::Close(id));
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into(), "actions".into()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("vile".into(), "vile".into(), "0.1.0".into(), "1.2".into())
    }
}

fn hint_string(hints: &HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<String> {
    let value = hints.get(key)?;
    <&str>::try_from(value).ok().map(str::to_string)
}

fn hint_u8(hints: &HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<u8> {
    let value = hints.get(key)?;
    u8::try_from(value)
        .ok()
        .or_else(|| u32::try_from(value).ok().map(|n| n as u8))
}

fn hint_i32(hints: &HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<i32> {
    let value = hints.get(key)?;
    i32::try_from(value)
        .ok()
        .or_else(|| i64::try_from(value).ok().map(|n| n as i32))
        .or_else(|| u32::try_from(value).ok().map(|n| n as i32))
}

pub fn app_id_matches(app_id: &str, entry: &str) -> bool {
    let app = app_id.trim().to_ascii_lowercase();
    let entry = entry
        .trim()
        .trim_end_matches(".desktop")
        .to_ascii_lowercase();
    if app.is_empty() || entry.is_empty() {
        return false;
    }
    if app == entry {
        return true;
    }
    let app_tail = app.rsplit('.').next().unwrap_or(&app);
    let entry_tail = entry.rsplit('.').next().unwrap_or(&entry);
    app_tail == entry_tail
        || app.ends_with(&format!(".{entry}"))
        || entry.ends_with(&format!(".{app}"))
}

pub fn match_window<'a>(
    windows: impl IntoIterator<Item = &'a Window>,
    desktop_entry: Option<&str>,
    pid: Option<i32>,
    focused_workspace: Option<u64>,
) -> Option<u64> {
    let mut best: Option<(u32, u64)> = None;
    for win in windows {
        let mut score = 0u32;
        if let (Some(want), Some(have)) = (pid, win.pid) {
            if want == have {
                score += 8;
            }
        }
        if let (Some(entry), Some(app_id)) = (desktop_entry, win.app_id.as_deref()) {
            if app_id_matches(app_id, entry) {
                score += 4;
            }
        }
        if score == 0 {
            continue;
        }
        if win.is_urgent {
            score += 2;
        }
        if focused_workspace.is_some() && win.workspace_id == focused_workspace {
            score += 1;
        }
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, win.id));
        }
    }
    best.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::{app_id_matches, match_window, InboxState, NotifyCenter, NotifyItem};
    use niri_ipc::{Window, WindowLayout};

    fn win(
        id: u64,
        app_id: &str,
        pid: Option<i32>,
        workspace_id: Option<u64>,
        urgent: bool,
    ) -> Window {
        Window {
            id,
            title: None,
            app_id: Some(app_id.into()),
            pid,
            workspace_id,
            is_focused: false,
            is_floating: false,
            is_urgent: urgent,
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
    fn desktop_entry_matches_app_id_tails() {
        assert!(app_id_matches("firefox", "firefox"));
        assert!(app_id_matches("org.mozilla.firefox", "firefox"));
        assert!(app_id_matches("firefox", "org.mozilla.firefox"));
        assert!(!app_id_matches("firefox", "discord"));
    }

    #[test]
    fn prefers_pid_then_urgent_workspace() {
        let windows = vec![
            win(1, "firefox", Some(10), Some(1), false),
            win(2, "firefox", Some(11), Some(2), true),
            win(3, "discord", Some(12), Some(1), true),
        ];
        assert_eq!(
            match_window(&windows, Some("firefox"), Some(11), Some(1)),
            Some(2)
        );
        assert_eq!(
            match_window(&windows, Some("discord"), None, Some(1)),
            Some(3)
        );
        assert_eq!(match_window(&windows, Some("missing"), None, Some(1)), None);
    }

    #[test]
    fn toast_shows_title_and_body_not_sender_tool() {
        let item = NotifyItem {
            id: 1,
            app_name: "notify-send".into(),
            summary: "inbox".into(),
            body: "hello from dbus\nmore detail".into(),
            desktop_entry: None,
            pid: None,
            urgency: 1,
            expire_timeout_ms: None,
        };
        assert_eq!(item.toast_text(), "inbox  hello from dbus more detail");
        assert!(!item.toast_text().contains("notify-send"));
        let title_only = NotifyItem {
            app_name: "gdbus".into(),
            summary: "just a title".into(),
            body: String::new(),
            ..item.clone()
        };
        assert_eq!(title_only.toast_text(), "just a title");
    }

    #[test]
    fn inbox_wrap_and_clear_last_exits() {
        let mut center = NotifyCenter::default();
        center.upsert(
            NotifyItem {
                id: 1,
                app_name: "a".into(),
                summary: "one".into(),
                body: String::new(),
                desktop_entry: None,
                pid: None,
                urgency: 1,
                expire_timeout_ms: None,
            },
            24,
        );
        center.upsert(
            NotifyItem {
                id: 2,
                app_name: "b".into(),
                summary: "two".into(),
                body: String::new(),
                desktop_entry: None,
                pid: None,
                urgency: 1,
                expire_timeout_ms: None,
            },
            24,
        );
        center.inbox = Some(InboxState {
            index: 0,
            confirm_clear: false,
        });
        let n = center.items.len();
        let inbox = center.inbox.as_mut().unwrap();
        inbox.index = (inbox.index + 1) % n;
        assert_eq!(inbox.index, 1);
        inbox.index = (inbox.index + 1) % n;
        assert_eq!(inbox.index, 0);
        assert!(center.remove(1));
        assert!(center.remove(2));
        assert!(center.items.is_empty());
        assert!(center.inbox.is_none());
    }
}
