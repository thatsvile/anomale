mod install;
mod map;
mod theme;

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use vile_protocol::{color_cache_dir, vilefox_config_path};

use crate::map::{self as foxmap};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("install") => install::run(),
        Some("config") => install::write_config().map(|path| {
            println!("{}", path.display());
        }),
        Some("-h" | "--help") => {
            println!("usage: vilefox | vilefox install | vilefox config");
            Ok(())
        }
        Some(other) if other.starts_with('-') => {
            anyhow::bail!("usage: vilefox | vilefox install | vilefox config");
        }
        _ => native_host(),
    }
}

fn native_host() -> Result<()> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            match read_message(&mut stdin) {
                Ok(None) => {
                    let _ = tx.send(HostEvent::Eof);
                    break;
                }
                Ok(Some(_)) => {}
                Err(_) => {
                    let _ = tx.send(HostEvent::Eof);
                    break;
                }
            }
        }
    });
    let mut last = None;
    let mut last_chrome = None;
    let mut stdout = io::stdout();
    push_theme(&mut stdout, &mut last, &mut last_chrome)?;
    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(HostEvent::Eof) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        push_theme(&mut stdout, &mut last, &mut last_chrome)?;
    }
}

enum HostEvent {
    Eof,
}

fn push_theme(
    stdout: &mut io::Stdout,
    last: &mut Option<String>,
    last_chrome: &mut Option<String>,
) -> Result<()> {
    let path = color_cache_dir().join("colors.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) if !text.is_empty() => text,
        _ => return Ok(()),
    };
    let cfg_text = std::fs::read_to_string(vilefox_config_path()).unwrap_or_default();
    let stamp = format!("{text}\n--\n{cfg_text}");
    if last.as_ref() == Some(&stamp) {
        return Ok(());
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(());
    };
    let cfg = foxmap::load_from(&cfg_text);
    let chrome = install::user_chrome(&cfg);
    if last_chrome.as_ref() != Some(&chrome) {
        let _ = install::write_all_user_chrome(&cfg);
        *last_chrome = Some(chrome);
    }
    let Some((_, colors)) = foxmap::resolve(&json, &cfg) else {
        return Ok(());
    };
    let payload = json!({ "colors": colors });
    write_message(stdout, &payload)?;
    *last = Some(stamp);
    Ok(())
}

fn read_message(stdin: &mut impl Read) -> io::Result<Option<serde_json::Value>> {
    let mut len_buf = [0u8; 4];
    if stdin.read_exact(&mut len_buf).is_err() {
        return Ok(None);
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 8 * 1024 * 1024 {
        return Ok(None);
    }
    let mut buf = vec![0u8; len];
    stdin.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf).ok())
}

fn write_message(stdout: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let len = bytes.len() as u32;
    stdout.write_all(&len.to_le_bytes())?;
    stdout.write_all(&bytes)?;
    stdout.flush()
}
