use async_channel;
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Label, Orientation};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy)]
struct TagState {
    selected: bool,
    occupied: bool,
    urgent: bool,
}

fn tag_id_from_key(key: &str) -> Option<i32> {
    key.parse::<i32>()
        .ok()
        .or_else(|| {
            key.strip_prefix("tag_")
                .and_then(|suffix| suffix.parse::<i32>().ok())
        })
        .or_else(|| {
            key.strip_prefix("tag")
                .and_then(|suffix| suffix.parse::<i32>().ok())
        })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    if let Some(num) = value.as_i64() {
        return Some(num);
    }

    value.as_str()?.parse::<i64>().ok()
}

fn value_as_usize(value: &Value) -> Option<usize> {
    if let Some(items) = value.as_array() {
        return Some(items.len());
    }

    if let Some(obj) = value.as_object() {
        return Some(obj.len());
    }

    value_as_i64(value).map(|num| num.max(0) as usize)
}

fn value_as_bool(value: &Value) -> Option<bool> {
    if let Some(bool_value) = value.as_bool() {
        return Some(bool_value);
    }

    if let Some(num) = value.as_i64() {
        return Some(num != 0);
    }

    let text = value.as_str()?.to_ascii_lowercase();
    Some(matches!(
        text.as_str(),
        "true" | "1" | "yes" | "active" | "selected" | "occupied" | "urgent"
    ))
}

fn int_field(value: &Value, keys: &[&str]) -> Option<i64> {
    let obj = value.as_object()?;
    for key in keys {
        if let Some(field) = obj.get(*key) {
            if let Some(num) = value_as_i64(field) {
                return Some(num);
            }
        }
    }
    None
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    let obj = value.as_object()?;
    for key in keys {
        if let Some(field) = obj.get(*key) {
            if let Some(bool_value) = value_as_bool(field) {
                return Some(bool_value);
            }
        }
    }
    None
}

fn tag_state_from_value(value: &Value) -> Option<TagState> {
    let state = int_field(value, &["state", "status"]);
    let state_text = value
        .as_object()
        .and_then(|obj| obj.get("state").or_else(|| obj.get("status")))
        .and_then(|value| value.as_str())
        .map(|text| text.to_ascii_lowercase());

    let selected = bool_field(
        value,
        &[
            "selected",
            "active",
            "focused",
            "current",
            "visible",
            "is_selected",
            "is_active",
            "is_focused",
            "is_current",
            "is_visible",
        ],
    );
    let selected = match selected {
        Some(selected) => selected,
        None => {
            if let Some(state) = state {
                state == 1
            } else {
                matches!(
                    state_text.as_deref(),
                    Some("active" | "selected" | "focused")
                )
            }
        }
    };

    let mut occupied = bool_field(value, &["occupied", "has_clients", "is_occupied"]);
    if occupied.is_none() {
        if let Some(obj) = value.as_object() {
            for key in [
                "clients",
                "client_count",
                "clients_count",
                "num_clients",
                "client_num",
            ] {
                if let Some(field) = obj.get(key) {
                    occupied = value_as_usize(field).map(|clients| clients > 0);
                    if occupied.is_some() {
                        break;
                    }
                }
            }
        }
    }

    let urgent = match bool_field(value, &["urgent", "is_urgent"]) {
        Some(urgent) => urgent,
        None => state == Some(2) || matches!(state_text.as_deref(), Some("urgent")),
    };

    Some(TagState {
        selected,
        occupied: occupied.unwrap_or(false),
        urgent,
    })
}

fn collect_mask_tag_states(value: &Value, states: &mut Vec<(i32, TagState)>) -> bool {
    let selected_mask = int_field(value, &["selected", "active", "focused", "current"]);
    let occupied_mask = int_field(value, &["occupied"]);
    let urgent_mask = int_field(value, &["urgent"]);

    if selected_mask.is_none() && occupied_mask.is_none() && urgent_mask.is_none() {
        return false;
    }

    for id in 1..=9 {
        let bit = 1_i64 << (id - 1);
        states.push((
            id,
            TagState {
                selected: selected_mask.map(|mask| mask & bit != 0).unwrap_or(false),
                occupied: occupied_mask.map(|mask| mask & bit != 0).unwrap_or(false),
                urgent: urgent_mask.map(|mask| mask & bit != 0).unwrap_or(false),
            },
        ));
    }

    true
}

fn collect_tag_states(value: &Value, hinted_id: Option<i32>, states: &mut Vec<(i32, TagState)>) {
    match value {
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                collect_tag_states(item, Some((idx + 1) as i32), states);
            }
        }
        Value::Object(obj) => {
            let explicit_id = int_field(value, &["id", "idx", "index", "tag"]).map(|id| id as i32);
            let tag_id = explicit_id.or(hinted_id);

            if tag_id.is_none() && collect_mask_tag_states(value, states) {
                return;
            }

            if let Some(id) = tag_id {
                if let Some(state) = tag_state_from_value(value) {
                    states.push((id, state));
                    return;
                }
            }

            if let Some(tags) = obj.get("tags") {
                collect_tag_states(tags, None, states);
            }

            for (key, nested) in obj {
                if key == "tags" {
                    continue;
                }

                let nested_id = tag_id_from_key(key);
                if nested_id.is_some() || nested.is_object() || nested.is_array() {
                    collect_tag_states(nested, nested_id, states);
                }
            }
        }
        _ => {}
    }
}

fn parse_tag_states(raw: &str) -> Result<Vec<(i32, TagState)>, serde_json::Error> {
    let value: Value = serde_json::from_str(raw)?;
    let mut states = Vec::new();
    collect_tag_states(&value, None, &mut states);
    Ok(states)
}

fn send_tag_states(
    raw: &str,
    sender: &async_channel::Sender<(i32, TagState)>,
    context: &str,
) -> bool {
    match parse_tag_states(raw) {
        Ok(states) if states.is_empty() => {
            eprintln!(
                "mmsg {} returned no recognizable tag states: {}",
                context, raw
            );
            true
        }
        Ok(states) => {
            for state in states {
                let _ = sender.send_blocking(state);
            }
            true
        }
        Err(err) => {
            if err.is_eof() {
                return false;
            }
            eprintln!("Failed to parse mmsg {} JSON: {} ({})", context, err, raw);
            true
        }
    }
}

pub fn build(monitor: &gtk4::gdk::Monitor) -> Box {
    let container = Box::new(Orientation::Horizontal, 0);
    container.add_css_class("tags-container");

    let monitor_name = monitor.connector().unwrap_or_else(|| "Unknown".into());
    let buttons = Arc::new(Mutex::new(HashMap::new()));

    // Create 9 tag buttons initially (1-9)
    for i in 1..=9 {
        let button = Button::builder().has_frame(false).build();
        button.add_css_class("tag");
        button.set_visible(false);

        // Inner layout: Dot + Number
        let bbox = Box::new(Orientation::Horizontal, 2); // spacing 2px
        let dot = Label::new(Some("●"));
        dot.add_css_class("dot");
        dot.set_valign(Align::Center);

        let num = Label::new(Some(&i.to_string()));
        num.add_css_class("num");
        num.set_valign(Align::Center);

        bbox.append(&dot);
        bbox.append(&num);
        button.set_child(Some(&bbox));

        let tag_id = i;
        button.connect_clicked(move |_| {
            let _ = Command::new("mmsg")
                .arg("dispatch")
                .arg(format!("view,{}", tag_id))
                .spawn();
        });

        container.append(&button);
        buttons.lock().unwrap().insert(i, button);
    }

    // Use async-channel
    let (sender, receiver) = async_channel::unbounded();

    let monitor_name_clone = monitor_name.clone();
    let watch_sender = sender.clone();

    // Spawn mmsg watcher
    thread::spawn(move || {
        let child = Command::new("mmsg")
            .arg("watch")
            .arg("tags")
            .arg(&monitor_name_clone)
            .stdout(Stdio::piped())
            .spawn();

        if let Ok(mut child) = child {
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                let mut pending = String::new();
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let line = line.trim();
                        if !line.is_empty() {
                            pending.push_str(line);
                            pending.push('\n');

                            if send_tag_states(&pending, &watch_sender, "watch tags") {
                                pending.clear();
                            }
                        }
                    }
                }
            }
        }
    });

    // Handle updates in main thread
    gtk4::glib::MainContext::default().spawn_local(async move {
        while let Ok((id, state)) = receiver.recv().await {
            if let Ok(buttons) = buttons.lock() {
                if let Some(button) = buttons.get(&id) {
                    if state.selected {
                        button.add_css_class("selected");
                    } else {
                        button.remove_css_class("selected");
                    }

                    if state.occupied {
                        button.add_css_class("occupied");
                    } else {
                        button.remove_css_class("occupied");
                    }

                    if state.urgent {
                        button.add_css_class("urgent");
                    } else {
                        button.remove_css_class("urgent");
                    }

                    if !state.occupied && !state.selected {
                        button.set_visible(false);
                    } else {
                        button.set_visible(true);
                    }
                }
            }
        }
    });
    // Trigger initial state
    let initial_sender = sender.clone();
    let initial_monitor_name = monitor_name.clone();
    thread::spawn(move || {
        match Command::new("mmsg")
            .arg("get")
            .arg("tags")
            .arg(&initial_monitor_name)
            .output()
        {
            Ok(output) => {
                let raw = String::from_utf8_lossy(&output.stdout);
                let raw = raw.trim();
                if !raw.is_empty() {
                    send_tag_states(raw, &initial_sender, "get tags");
                }
            }
            Err(err) => {
                eprintln!("Failed to run mmsg get tags: {}", err);
            }
        }
    });

    container
}
