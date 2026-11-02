use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use log::{info, warn};
use niri_ipc::{Action, Event, Request, Response, Window, Workspace, WorkspaceReferenceArg};

pub struct Niri {
    actions: UnixStream,
    buf: Vec<u8>,
}

impl Niri {
    pub fn connect() -> Option<(Self, UnixStream)> {
        let path = socket_path()?;
        info!("niri socket {}", path.display());

        let mut events = UnixStream::connect(&path).ok()?;
        events.set_nonblocking(false).ok()?;
        write_request(&mut events, &Request::EventStream).ok()?;
        let hello: niri_ipc::Reply = read_json(&mut events).ok()?;
        if let Err(err) = hello {
            warn!("niri event stream: {err}");
            return None;
        }
        events.set_nonblocking(true).ok()?;

        let actions = UnixStream::connect(&path).ok()?;
        actions.set_nonblocking(false).ok()?;
        Some((
            Self {
                actions,
                buf: Vec::new(),
            },
            events,
        ))
    }

    pub fn refresh(&mut self) -> Option<(Vec<Workspace>, Vec<Window>)> {
        let workspaces = self.request(Request::Workspaces, |response| match response {
            Response::Workspaces(list) => Some(list),
            _ => None,
        })?;
        let windows = self
            .request(Request::Windows, |response| match response {
                Response::Windows(list) => Some(list),
                _ => None,
            })
            .unwrap_or_default();
        Some((workspaces, windows))
    }

    fn request<T>(
        &mut self,
        request: Request,
        take: impl FnOnce(Response) -> Option<T>,
    ) -> Option<T> {
        if let Err(err) = write_request(&mut self.actions, &request) {
            warn!("niri request write: {err}");
            return None;
        }
        match read_json::<niri_ipc::Reply>(&mut self.actions) {
            Ok(Ok(response)) => take(response),
            Ok(Err(err)) => {
                warn!("niri request: {err}");
                None
            }
            Err(err) => {
                warn!("niri request read: {err}");
                None
            }
        }
    }

    pub fn read_events(&mut self, stream: &UnixStream) -> io::Result<Vec<Event>> {
        let mut stream = stream;
        vile_protocol::read_json_lines(&mut stream, &mut self.buf)
    }

    pub fn focus_workspace(&mut self, output: &str, id: u64) {
        info!("niri focus workspace id={id} output={output}");
        self.action(Action::FocusMonitor {
            output: output.to_string(),
        });
        self.action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(id),
        });
    }

    pub fn focus_window(&mut self, id: u64) {
        info!("niri focus window id={id}");
        self.action(Action::FocusWindow { id });
    }
}

pub fn occupied_workspace_ids(windows: &HashMap<u64, Window>) -> HashSet<u64> {
    windows
        .values()
        .filter_map(|win| win.workspace_id)
        .collect()
}

pub fn workspaces_for<'a>(
    state: &'a [Workspace],
    output: &str,
    occupied: &HashSet<u64>,
) -> Vec<&'a Workspace> {
    let mut list: Vec<&Workspace> = state
        .iter()
        .filter(|ws| {
            ws.output.as_deref() == Some(output) && (ws.is_active || occupied.contains(&ws.id))
        })
        .collect();
    list.sort_by_key(|ws| ws.idx);
    list
}

pub fn apply_event(
    workspaces: &mut Vec<Workspace>,
    windows: &mut HashMap<u64, Window>,
    event: &Event,
) -> bool {
    match event {
        Event::WorkspacesChanged {
            workspaces: next, ..
        } => {
            *workspaces = next.clone();
            true
        }
        Event::WorkspaceActivated { id, focused } => {
            let output = workspaces
                .iter()
                .find(|ws| ws.id == *id)
                .and_then(|ws| ws.output.clone());
            let mut changed = false;
            for ws in workspaces.iter_mut() {
                if output.is_some() && ws.output == output {
                    let active = ws.id == *id;
                    if ws.is_active != active {
                        ws.is_active = active;
                        changed = true;
                    }
                }
                if *focused {
                    let is_focused = ws.id == *id;
                    if ws.is_focused != is_focused {
                        ws.is_focused = is_focused;
                        changed = true;
                    }
                }
            }
            changed
        }
        Event::WorkspaceUrgencyChanged { id, urgent } => {
            if let Some(ws) = workspaces.iter_mut().find(|ws| ws.id == *id) {
                if ws.is_urgent != *urgent {
                    ws.is_urgent = *urgent;
                    return true;
                }
            }
            false
        }
        Event::WorkspaceActiveWindowChanged {
            workspace_id,
            active_window_id,
        } => {
            if let Some(ws) = workspaces.iter_mut().find(|ws| ws.id == *workspace_id) {
                if ws.active_window_id != *active_window_id {
                    ws.active_window_id = *active_window_id;
                    return true;
                }
            }
            false
        }
        Event::WindowsChanged { windows: next, .. } => {
            *windows = next.iter().cloned().map(|win| (win.id, win)).collect();
            true
        }
        Event::WindowOpenedOrChanged { window } => {
            let moved = windows
                .get(&window.id)
                .map(|old| old.workspace_id != window.workspace_id)
                .unwrap_or(true);
            windows.insert(window.id, window.clone());
            moved
        }
        Event::WindowClosed { id } => windows.remove(id).is_some(),
        Event::WindowFocusChanged { id } => {
            let mut changed = false;
            for win in windows.values_mut() {
                let focused = Some(win.id) == *id;
                if win.is_focused != focused {
                    win.is_focused = focused;
                    changed = true;
                }
            }
            changed
        }
        _ => false,
    }
}

/// Whether a niri window-focus event means the user clicked away from an exclusive bar prompt.
pub fn should_yield_grab(
    grab_window: Option<u64>,
    lost_window_focus: bool,
    new_focus: Option<u64>,
) -> bool {
    match new_focus {
        None => false,
        Some(id) => lost_window_focus || grab_window != Some(id),
    }
}

impl Niri {
    fn action(&mut self, action: Action) {
        if let Err(err) = write_request(&mut self.actions, &Request::Action(action)) {
            warn!("niri action write: {err}");
            return;
        }
        match read_json::<niri_ipc::Reply>(&mut self.actions) {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => warn!("niri action: {err}"),
            Err(err) => warn!("niri action read: {err}"),
        }
    }
}

fn socket_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(niri_ipc::socket::SOCKET_PATH_ENV) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let mut matches: Vec<PathBuf> = fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("niri.") && n.ends_with(".sock"))
        })
        .collect();
    matches.sort();
    matches.pop()
}

fn write_request(stream: &mut UnixStream, request: &Request) -> io::Result<()> {
    let mut line = serde_json::to_vec(request)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    line.push(b'\n');
    stream.write_all(&line)
}

fn read_json<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> io::Result<T> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "niri closed"));
            }
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => buf.push(byte[0]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    serde_json::from_slice(&buf).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
mod tests {
    use super::{apply_event, should_yield_grab, workspaces_for};
    use niri_ipc::{Event, Workspace};
    use std::collections::{HashMap, HashSet};

    fn ws(id: u64, idx: u8, output: &str, active: bool) -> Workspace {
        Workspace {
            id,
            idx,
            name: None,
            output: Some(output.into()),
            is_urgent: false,
            is_active: active,
            is_focused: active,
            active_window_id: None,
        }
    }

    #[test]
    fn hides_empty_inactive_workspaces() {
        let all = vec![
            ws(1, 1, "eDP-1", true),
            ws(2, 2, "eDP-1", false),
            ws(3, 1, "HDMI-A-1", true),
        ];
        let occupied = HashSet::from([1, 3]);
        let edp = workspaces_for(&all, "eDP-1", &occupied);
        assert_eq!(edp.len(), 1);
        assert_eq!(edp[0].idx, 1);
    }

    #[test]
    fn keeps_empty_workspace_while_it_is_active() {
        let all = vec![ws(1, 1, "eDP-1", false), ws(2, 2, "eDP-1", true)];
        let occupied = HashSet::from([1]);
        let edp = workspaces_for(&all, "eDP-1", &occupied);
        assert_eq!(edp.iter().map(|ws| ws.idx).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn activation_updates_that_output_only() {
        let mut all = vec![
            ws(1, 1, "eDP-1", true),
            ws(2, 2, "eDP-1", false),
            ws(3, 1, "HDMI-A-1", true),
        ];
        let mut windows = HashMap::new();
        apply_event(
            &mut all,
            &mut windows,
            &Event::WorkspaceActivated {
                id: 2,
                focused: true,
            },
        );
        assert!(!all[0].is_active);
        assert!(all[1].is_active);
        assert!(all[1].is_focused);
        assert!(all[2].is_active);
        assert!(!all[2].is_focused);
    }

    #[test]
    fn yield_grab_ignores_restated_focus() {
        assert!(!should_yield_grab(Some(3), false, Some(3)));
        assert!(!should_yield_grab(Some(3), false, None));
        assert!(should_yield_grab(Some(3), false, Some(9)));
        assert!(should_yield_grab(Some(3), true, Some(3)));
        assert!(should_yield_grab(None, false, Some(1)));
    }
}
