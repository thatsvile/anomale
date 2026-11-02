use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Instant;

use vile_protocol::MenuItem;

use crate::tray::wrap_index;

#[derive(Debug, Clone)]
pub struct MenuSession {
    pub index: usize,
}

#[derive(Debug, Default)]
pub struct MenuCenter {
    pub session: Option<MenuSession>,
    pub empty_until: Option<Instant>,
}

impl MenuCenter {
    pub fn line(&self, items: &[MenuItem]) -> Option<String> {
        if let Some(until) = self.empty_until {
            if Instant::now() < until {
                return Some("no menu".into());
            }
        }
        let session = self.session.as_ref()?;
        overlay_line(items, session.index)
    }

    pub fn current<'a>(&self, items: &'a [MenuItem]) -> Option<&'a MenuItem> {
        let session = self.session.as_ref()?;
        usable_items(items).get(session.index).copied()
    }
}

pub fn usable_items(items: &[MenuItem]) -> Vec<&MenuItem> {
    items
        .iter()
        .filter(|item| !item.label.trim().is_empty() && !item.command.trim().is_empty())
        .collect()
}

pub fn overlay_line(items: &[MenuItem], index: usize) -> Option<String> {
    let items = usable_items(items);
    let item = items.get(index)?;
    Some(format!("{}  {}/{}", item.label, index + 1, items.len()))
}

pub fn step_index(index: usize, delta: i32, n: usize) -> usize {
    wrap_index(index, delta, n)
}

pub fn spawn_menu_command(command: &str) -> io::Result<()> {
    let command = command.trim();
    if command.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
    }
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}

extern "C" {
    fn setsid() -> i32;
}

#[cfg(test)]
mod tests {
    use super::{overlay_line, spawn_menu_command, step_index, usable_items, MenuSession};
    use vile_protocol::MenuItem;

    fn item(label: &str, command: &str) -> MenuItem {
        MenuItem {
            label: label.into(),
            command: command.into(),
        }
    }

    #[test]
    fn wraps_index() {
        assert_eq!(step_index(0, 1, 3), 1);
        assert_eq!(step_index(2, 1, 3), 0);
        assert_eq!(step_index(0, -1, 3), 2);
        assert_eq!(step_index(0, 1, 0), 0);
    }

    #[test]
    fn skips_blank_rows_and_empty_does_not_run() {
        let items = vec![
            item(" ", "systemctl poweroff"),
            item("reboot", ""),
            item("log out", "niri msg action quit --skip-confirmation"),
        ];
        let usable = usable_items(&items);
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].label, "log out");
        assert_eq!(overlay_line(&items, 0).as_deref(), Some("log out  1/1"));
        assert!(overlay_line(&[], 0).is_none());
        let session = MenuSession { index: 0 };
        assert!(usable_items(&[]).get(session.index).is_none());
        assert!(spawn_menu_command("").is_err());
        assert!(spawn_menu_command("   ").is_err());
    }
}
