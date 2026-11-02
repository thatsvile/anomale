use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub exec: String,
    pub keywords: Vec<String>,
    pub terminal: bool,
}

#[derive(Debug, Clone)]
pub struct AppsPrompt {
    pub query: String,
    pub selected: usize,
    entries: Vec<DesktopApp>,
}

impl AppsPrompt {
    pub fn open() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            entries: scan_applications(),
        }
    }

    pub fn matches(&self) -> Vec<&DesktopApp> {
        rank_apps(&self.query, &self.entries)
    }

    pub fn selected_match(&self) -> Option<&DesktopApp> {
        let matches = self.matches();
        if matches.is_empty() {
            None
        } else {
            Some(matches[self.selected.min(matches.len() - 1)])
        }
    }

    #[cfg(test)]
    pub fn from_entries(query: &str, selected: usize, entries: Vec<DesktopApp>) -> Self {
        let mut prompt = Self {
            query: query.to_string(),
            selected,
            entries,
        };
        prompt.clamp_selected();
        prompt
    }

    pub fn clamp_selected(&mut self) {
        let n = self.matches().len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }
}

pub fn command_remainder(query: &str) -> Option<&str> {
    query.strip_prefix(">>").map(|rest| rest.trim_start())
}

pub fn scan_applications() -> Vec<DesktopApp> {
    let mut by_id = HashMap::new();
    for dir in application_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() || by_id.contains_key(&id) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = parse_desktop_entry(&id, &text) {
                by_id.insert(id, app);
            }
        }
    }
    let mut apps: Vec<DesktopApp> = by_id.into_values().collect();
    apps.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    apps
}

pub fn rank_apps<'a>(query: &str, entries: &'a [DesktopApp]) -> Vec<&'a DesktopApp> {
    let needle = query.trim();
    if needle.is_empty() || command_remainder(needle).is_some() {
        return Vec::new();
    }
    let needle = needle.to_ascii_lowercase();
    let mut scored: Vec<(u8, &DesktopApp)> = entries
        .iter()
        .filter_map(|app| {
            let name = app.name.to_ascii_lowercase();
            let rank = if name.starts_with(&needle) {
                0
            } else if contains_query(app, &needle) {
                1
            } else {
                return None;
            };
            Some((rank, app))
        })
        .collect();
    scored.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    scored.into_iter().map(|(_, app)| app).collect()
}

fn contains_query(app: &DesktopApp, needle: &str) -> bool {
    let name = app.name.to_ascii_lowercase();
    if name.contains(needle) {
        return true;
    }
    if app
        .generic_name
        .as_ref()
        .is_some_and(|g| g.to_ascii_lowercase().contains(needle))
    {
        return true;
    }
    if app
        .keywords
        .iter()
        .any(|k| k.to_ascii_lowercase().contains(needle))
    {
        return true;
    }
    exec_basename(&app.exec)
        .to_ascii_lowercase()
        .contains(needle)
}

fn exec_basename(exec: &str) -> String {
    let stripped = strip_field_codes(exec);
    let argv = split_exec(&stripped);
    argv.first()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

pub fn parse_desktop_entry(id: &str, text: &str) -> Option<DesktopApp> {
    let mut in_entry = false;
    let mut name = None;
    let mut generic_name = None;
    let mut exec = None;
    let mut try_exec = None;
    let mut keywords = Vec::new();
    let mut terminal = false;
    let mut hidden = false;
    let mut no_display = false;
    let mut entry_type = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_entry = section == "Desktop Entry";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.contains('[') {
            continue;
        }
        let value = value.trim();
        match key.trim() {
            "Name" => name = Some(value.to_string()),
            "GenericName" => generic_name = Some(value.to_string()),
            "Exec" => exec = Some(value.to_string()),
            "TryExec" => try_exec = Some(value.to_string()),
            "Keywords" => {
                keywords = value
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "Terminal" => terminal = parse_bool(value),
            "Hidden" => hidden = parse_bool(value),
            "NoDisplay" => no_display = parse_bool(value),
            "Type" => entry_type = Some(value.to_string()),
            _ => {}
        }
    }
    if hidden || no_display {
        return None;
    }
    if let Some(kind) = entry_type {
        if !kind.eq_ignore_ascii_case("Application") {
            return None;
        }
    }
    let name = name.filter(|s| !s.is_empty())?;
    let exec = exec.filter(|s| !s.is_empty())?;
    if let Some(try_exec) = try_exec {
        if !command_exists(&try_exec) {
            return None;
        }
    }
    Some(DesktopApp {
        id: id.to_string(),
        name,
        generic_name,
        exec,
        keywords,
        terminal,
    })
}

fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

pub fn strip_field_codes(exec: &str) -> String {
    let chars: Vec<char> = exec.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '%' {
                out.push('%');
                i += 2;
                continue;
            }
            if next.is_ascii_alphabetic() {
                i += 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut chars = exec.chars();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
    }
    args
}

pub fn parse_exec(exec: &str) -> Vec<String> {
    split_exec(&strip_field_codes(exec))
}

pub fn launch_app(app: &DesktopApp, terminal: &str) -> io::Result<()> {
    let argv = parse_exec(&app.exec);
    if argv.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty Exec"));
    }
    if app.terminal {
        spawn_argv(&terminal_exec_argv(terminal, &argv, &home_dir()))
    } else {
        spawn_argv(&argv)
    }
}

pub fn spawn_terminal(terminal: &str, command: Option<&str>) -> io::Result<()> {
    let shell = user_shell();
    spawn_argv(&terminal_command_argv(
        terminal,
        command,
        &shell,
        &home_dir(),
    ))
}

/// Open a real terminal at $HOME, then run `cmd` in an interactive user shell
/// the same way typing it would: `foot -D $HOME -- $SHELL -i -c 'cmd; exec $SHELL -i'`.
/// Flags stay unbundled (`-i` then `-c`); fish treats `-ic -c` as a command named `-c`.
pub fn terminal_command_argv(
    terminal: &str,
    command: Option<&str>,
    shell: &str,
    home: &str,
) -> Vec<String> {
    match command.filter(|s| !s.is_empty()) {
        None => vec![terminal.to_string(), "-D".into(), home.to_string()],
        Some(cmd) => vec![
            terminal.to_string(),
            "-D".into(),
            home.to_string(),
            "--".into(),
            shell.to_string(),
            "-i".into(),
            "-c".into(),
            format!("{cmd}; exec {shell} -i"),
        ],
    }
}

/// Terminal=true .desktop entries: `foot -D $HOME -- /abs/path` like rofi drun.
pub fn terminal_exec_argv(terminal: &str, argv: &[String], home: &str) -> Vec<String> {
    let mut out = vec![
        terminal.to_string(),
        "-D".into(),
        home.to_string(),
        "--".into(),
    ];
    out.extend(argv.iter().cloned());
    out
}

fn home_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".into())
}

/// Foot's `shell=` from foot.ini, not the daemon's `$SHELL`. A daemon started
/// from Cursor/niri can have SHELL=bash while foot is configured to run fish.
fn user_shell() -> String {
    foot_config_shell()
        .or_else(|| env::var("SHELL").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/bin/sh".into())
}

fn foot_config_shell() -> Option<String> {
    let path = xdg_config_home()?.join("foot/foot.ini");
    parse_foot_shell(&fs::read_to_string(path).ok()?)
}

fn xdg_config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

pub fn parse_foot_shell(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "shell" {
            continue;
        }
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn spawn_argv(argv: &[String]) -> io::Result<()> {
    let Some((bin, args)) = argv.split_first() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
    };
    let mut cmd = Command::new(bin);
    let shell = user_shell();
    cmd.env("SHELL", &shell);
    if let Ok(home) = env::var("HOME") {
        cmd.current_dir(&home);
        let mut dirs = vec![PathBuf::from(&home).join(".local/bin")];
        dirs.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = env::join_paths(dirs) {
            cmd.env("PATH", path);
        }
    }
    cmd.args(args)
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

fn command_exists(cmd: &str) -> bool {
    let path = Path::new(cmd);
    if path.is_absolute() || cmd.contains('/') {
        return path.is_file();
    }
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|dir| dir.join(cmd).is_file())
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    if let Some(home) = data_home {
        dirs.push(home.join("applications"));
    }
    let data_dirs = env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty());
    let defaults = ["/usr/local/share", "/usr/share"];
    let extra: Vec<PathBuf> = match data_dirs {
        Some(value) => value
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect(),
        None => defaults.iter().map(PathBuf::from).collect(),
    };
    for dir in extra {
        dirs.push(dir.join("applications"));
    }
    dirs
}

extern "C" {
    fn setsid() -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, exec: &str, keywords: &[&str]) -> DesktopApp {
        DesktopApp {
            id: name.to_ascii_lowercase(),
            name: name.into(),
            generic_name: None,
            exec: exec.into(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            terminal: false,
        }
    }

    #[test]
    fn foot_ini_shell_ignores_comments_and_passwd_bash() {
        let ini = "# shell=/bin/bash\n[main]\nshell=/usr/bin/fish\n";
        assert_eq!(parse_foot_shell(ini).as_deref(), Some("/usr/bin/fish"));
        assert!(parse_foot_shell("# shell=/bin/bash\n").is_none());
    }

    #[test]
    fn terminal_argv_uses_login_shell_not_plain_sh() {
        assert_eq!(
            terminal_command_argv("foot", None, "/usr/bin/fish", "/home/jor"),
            vec!["foot", "-D", "/home/jor"]
        );
        assert_eq!(
            terminal_command_argv("foot", Some("fresh"), "/usr/bin/fish", "/home/jor"),
            vec![
                "foot",
                "-D",
                "/home/jor",
                "--",
                "/usr/bin/fish",
                "-i",
                "-c",
                "fresh; exec /usr/bin/fish -i"
            ]
        );
        assert_eq!(
            terminal_exec_argv("foot", &["/home/jor/.local/bin/fresh".into()], "/home/jor"),
            vec![
                "foot",
                "-D",
                "/home/jor",
                "--",
                "/home/jor/.local/bin/fresh"
            ]
        );
    }

    #[test]
    fn classifies_command_prefix() {
        assert_eq!(command_remainder(">>cmatrix"), Some("cmatrix"));
        assert_eq!(command_remainder(">> cmatrix"), Some("cmatrix"));
        assert_eq!(command_remainder(">>"), Some(""));
        assert_eq!(command_remainder("firefox"), None);
        assert_eq!(command_remainder(">firefox"), None);
    }

    #[test]
    fn strips_exec_field_codes_and_quotes() {
        assert_eq!(
            parse_exec(r#""/usr/bin/foo" --bar %u %%percent"#),
            vec!["/usr/bin/foo", "--bar", "%percent"]
        );
        assert_eq!(parse_exec("foot -e htop %F"), vec!["foot", "-e", "htop"]);
    }

    #[test]
    fn ranks_prefix_before_contains() {
        let entries = [
            app("Firefox", "firefox %u", &["browser"]),
            app("Firejail", "firejail", &[]),
            app("LibreOffice", "libreoffice", &["office", "fire"]),
        ];
        let names: Vec<&str> = rank_apps("fire", &entries)
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, vec!["Firefox", "Firejail", "LibreOffice"]);
        assert!(rank_apps("", &entries).is_empty());
        assert!(rank_apps(">>cmatrix", &entries).is_empty());
    }

    #[test]
    fn parse_skips_hidden_and_links() {
        let hidden = "[Desktop Entry]\nName=Nope\nExec=nope\nHidden=true\n";
        assert!(parse_desktop_entry("nope", hidden).is_none());
        let link = "[Desktop Entry]\nType=Link\nName=Web\nExec=true\n";
        assert!(parse_desktop_entry("web", link).is_none());
        let ok = "[Desktop Entry]\nName=True\nExec=/bin/true %f\nKeywords=ok;yes;\n";
        let app = parse_desktop_entry("true", ok).unwrap();
        assert_eq!(app.name, "True");
        assert_eq!(app.keywords, vec!["ok", "yes"]);
    }
}
