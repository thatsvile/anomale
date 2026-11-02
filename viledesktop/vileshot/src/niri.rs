use std::collections::HashMap;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use niri_ipc::{LogicalOutput, Output, Request, Response};
use vile_protocol::{read_json_line, write_json_line};

pub fn pick_output_name() -> Result<String> {
    let path = socket_path().context("niri socket not found")?;
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connecting to {}", path.display()))?;

    write_json_line(&mut stream, &Request::Outputs)?;
    let outputs = match read_json_line::<niri_ipc::Reply>(&mut stream)? {
        Ok(Response::Outputs(map)) => map,
        Ok(_) => bail!("niri Outputs reply was unexpected"),
        Err(err) => bail!("niri Outputs: {err}"),
    };

    if let Some(name) = pointer_output(&mut stream, &outputs) {
        return Ok(name);
    }

    write_json_line(&mut stream, &Request::FocusedOutput)?;
    match read_json_line::<niri_ipc::Reply>(&mut stream)? {
        Ok(Response::FocusedOutput(Some(output))) if output.logical.is_some() => {
            return Ok(output.name);
        }
        Ok(Response::FocusedOutput(_)) => {}
        Ok(_) => {}
        Err(err) => log::warn!("niri FocusedOutput: {err}"),
    }

    first_enabled(&outputs).context("no enabled niri outputs")
}

fn pointer_output(stream: &mut UnixStream, outputs: &HashMap<String, Output>) -> Option<String> {
    if write_json_line(stream, &"Pointer").is_err() {
        return None;
    }
    let value: serde_json::Value = read_json_line(stream).ok()?;
    let location = value.get("Ok")?.get("Pointer")?.get("location")?;
    let (x, y) = parse_location(location)?;
    hit_test(outputs, x, y)
}

fn parse_location(location: &serde_json::Value) -> Option<(f64, f64)> {
    if let Some(obj) = location.as_object() {
        let x = obj.get("x")?.as_f64()?;
        let y = obj.get("y")?.as_f64()?;
        return Some((x, y));
    }
    let arr = location.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    Some((arr[0].as_f64()?, arr[1].as_f64()?))
}

pub fn hit_test(outputs: &HashMap<String, Output>, x: f64, y: f64) -> Option<String> {
    let mut matches: Vec<(String, LogicalOutput)> = outputs
        .values()
        .filter_map(|output| {
            let logical = output.logical?;
            Some((output.name.clone(), logical))
        })
        .filter(|(_, logical)| contains(*logical, x, y))
        .collect();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches.into_iter().next().map(|(name, _)| name)
}

fn contains(logical: LogicalOutput, x: f64, y: f64) -> bool {
    let x0 = logical.x as f64;
    let y0 = logical.y as f64;
    let x1 = x0 + logical.width as f64;
    let y1 = y0 + logical.height as f64;
    x >= x0 && x < x1 && y >= y0 && y < y1
}

fn first_enabled(outputs: &HashMap<String, Output>) -> Option<String> {
    let mut names: Vec<String> = outputs
        .values()
        .filter(|output| output.logical.is_some())
        .map(|output| output.name.clone())
        .collect();
    names.sort();
    names.into_iter().next()
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
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("niri.") && name.ends_with(".sock"))
        })
        .collect();
    matches.sort();
    matches.pop()
}

#[cfg(test)]
mod tests {
    use super::{contains, hit_test, parse_location};
    use niri_ipc::{LogicalOutput, Output, Transform};
    use std::collections::HashMap;

    fn output(name: &str, x: i32, y: i32, width: u32, height: u32) -> Output {
        Output {
            name: name.into(),
            make: String::new(),
            model: String::new(),
            serial: None,
            physical_size: None,
            modes: Vec::new(),
            current_mode: Some(0),
            is_custom_mode: false,
            vrr_supported: false,
            vrr_enabled: false,
            logical: Some(LogicalOutput {
                x,
                y,
                width,
                height,
                scale: 1.0,
                transform: Transform::Normal,
            }),
        }
    }

    #[test]
    fn pointer_hits_the_output_under_the_cursor() {
        let mut outputs = HashMap::new();
        outputs.insert("eDP-1".into(), output("eDP-1", 0, 0, 1920, 1080));
        outputs.insert("DP-3".into(), output("DP-3", 1920, 0, 2560, 1440));
        assert_eq!(hit_test(&outputs, 10.0, 10.0).as_deref(), Some("eDP-1"));
        assert_eq!(hit_test(&outputs, 2000.0, 20.0).as_deref(), Some("DP-3"));
        assert!(hit_test(&outputs, -1.0, 0.0).is_none());
    }

    #[test]
    fn logical_contains_is_half_open() {
        let logical = LogicalOutput {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale: 1.0,
            transform: Transform::Normal,
        };
        assert!(contains(logical, 0.0, 0.0));
        assert!(!contains(logical, 100.0, 0.0));
        assert!(!contains(logical, 0.0, 100.0));
        assert!(contains(logical, 99.9, 99.9));
    }

    #[test]
    fn pointer_location_object_and_array() {
        let obj = serde_json::json!({"x": 12.5, "y": 8.0});
        assert_eq!(parse_location(&obj), Some((12.5, 8.0)));
        let arr = serde_json::json!([12.5, 8.0]);
        assert_eq!(parse_location(&arr), Some((12.5, 8.0)));
    }
}
