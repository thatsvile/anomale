use async_channel::Sender;
use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::thread;

#[derive(Debug, Clone, Copy)]
pub struct TagState {
    pub selected: bool,
    pub occupied: bool,
    pub urgent: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NiriWorkspace {
    #[allow(dead_code)]
    pub id: u64,
    pub idx: u64,
    pub output: Option<String>,
    pub is_urgent: bool,
    pub is_active: bool,
    #[allow(dead_code)]
    pub is_focused: bool,
    pub active_window_id: Option<u64>,
}

pub fn fetch_workspaces() -> Vec<NiriWorkspace> {
    match Command::new("niri").args(["msg", "--json", "workspaces"]).output() {
        Ok(output) if output.status.success() => {
            serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
                eprintln!(
                    "Failed to parse niri workspaces JSON: {} ({})",
                    err,
                    String::from_utf8_lossy(&output.stdout)
                );
                Vec::new()
            })
        }
        Ok(output) => {
            eprintln!(
                "niri msg workspaces failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Vec::new()
        }
        Err(err) => {
            eprintln!("Failed to run niri msg workspaces: {}", err);
            Vec::new()
        }
    }
}

pub fn workspaces_to_tag_states(workspaces: &[NiriWorkspace], monitor: &str) -> Vec<(i32, TagState)> {
    (1..=9)
        .map(|id| {
            let state = workspaces
                .iter()
                .find(|workspace| {
                    workspace.output.as_deref() == Some(monitor) && workspace.idx == id as u64
                })
                .map(|workspace| TagState {
                    selected: workspace.is_active,
                    occupied: workspace.active_window_id.is_some(),
                    urgent: workspace.is_urgent,
                })
                .unwrap_or(TagState {
                    selected: false,
                    occupied: false,
                    urgent: false,
                });

            (id, state)
        })
        .collect()
}

pub fn focus_workspace(idx: i32) {
    let _ = Command::new("niri")
        .args(["msg", "action", "focus-workspace", &idx.to_string()])
        .spawn();
}

fn send_tag_states_for_monitor(
    workspaces: &[NiriWorkspace],
    monitor: &str,
    sender: &Sender<(i32, TagState)>,
) {
    for (id, state) in workspaces_to_tag_states(workspaces, monitor) {
        let _ = sender.send_blocking((id, state));
    }
}

fn workspaces_from_event(value: &Value) -> Option<Vec<NiriWorkspace>> {
    value
        .get("WorkspacesChanged")?
        .get("workspaces")
        .and_then(|workspaces| serde_json::from_value(workspaces.clone()).ok())
}

fn is_workspace_event(value: &Value) -> bool {
    value.get("WorkspacesChanged").is_some()
        || value.get("WorkspaceActivated").is_some()
        || value.get("WorkspaceUrgencyChanged").is_some()
        || value.get("WorkspaceActiveWindowChanged").is_some()
}

pub fn spawn_workspace_watcher(monitor: String, sender: Sender<(i32, TagState)>) {
    thread::spawn(move || {
        let mut workspaces = fetch_workspaces();
        send_tag_states_for_monitor(&workspaces, &monitor, &sender);

        let child = Command::new("niri")
            .args(["msg", "--json", "event-stream"])
            .stdout(Stdio::piped())
            .spawn();

        let Ok(mut child) = child else {
            eprintln!("Failed to start niri event-stream");
            return;
        };

        let Some(stdout) = child.stdout.take() else {
            return;
        };

        let reader = BufReader::new(stdout);

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };

            if let Some(updated) = workspaces_from_event(&value) {
                workspaces = updated;
            } else if is_workspace_event(&value) {
                workspaces = fetch_workspaces();
            } else {
                continue;
            }

            send_tag_states_for_monitor(&workspaces, &monitor, &sender);
        }
    });
}
