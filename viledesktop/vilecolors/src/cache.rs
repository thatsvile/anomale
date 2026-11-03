use anyhow::{Context, Result};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::map::Palette;
use crate::recipe::Theme;
use crate::template::{collect_templates, render};
use vile_protocol::{color_cache_dir, expand_home_path, load_config, wal_compat_dir};

pub fn export_palette(palette: &Palette) -> Result<()> {
    let cache = color_cache_dir();
    write_cache(palette, &cache)?;
    link_wal_compat(&cache, &wal_compat_dir())?;
    if let Ok(seq) = fs::read(cache.join("sequences")) {
        apply_sequences(&seq);
    }
    run_colors_apply_commands(palette.theme);
    Ok(())
}

fn theme_label(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
    }
}

/// Run theme-resolved `colors_apply_commands*` after a successful colors export.
/// Failures are logged only.
///
/// Child stdout/stderr are discarded so chatty hooks (e.g. papirus-folders via
/// vile-refresh) cannot corrupt an in-process colorlab / ratatui alternate screen.
/// Commands are spawned in the background so colorlab is not blocked on
/// `vile-refresh.sh` sleeps / papirus-folders / pkexec.
/// Each child gets `VILE_THEME=dark|light` from the applied palette Theme base.
fn run_colors_apply_commands(theme: Theme) {
    let Ok(cfg) = load_config() else {
        return;
    };
    let theme_str = theme_label(theme);
    for raw in cfg.colors_apply_commands_for(theme_str) {
        let cmd = raw.trim();
        if cmd.is_empty() {
            continue;
        }
        let expanded = expand_command(cmd);
        match Command::new("sh")
            .arg("-c")
            .arg(&expanded)
            .env("VILE_THEME", theme_str)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let expanded = expanded.clone();
                std::thread::spawn(move || match child.wait() {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        if !io::stderr().is_terminal() {
                            eprintln!(
                                "vilecolors: colors_apply_commands failed ({status}): {expanded}"
                            );
                        }
                    }
                    Err(err) => {
                        if !io::stderr().is_terminal() {
                            eprintln!(
                                "vilecolors: colors_apply_commands wait failed: {err}: {expanded}"
                            );
                        }
                    }
                });
            }
            Err(err) => {
                if !io::stderr().is_terminal() {
                    eprintln!(
                        "vilecolors: colors_apply_commands spawn failed: {err}: {expanded}"
                    );
                }
            }
        }
    }
}

fn expand_command(cmd: &str) -> String {
    if let Some(rest) = cmd.strip_prefix("~/") {
        return expand_home_path(&format!("~/{rest}"))
            .to_string_lossy()
            .into_owned();
    }
    if cmd == "~" {
        return expand_home_path("~").to_string_lossy().into_owned();
    }
    // Expand a leading ~/ inside a longer shell command (e.g. "~/.local/bin/vile-refresh.sh").
    if cmd.contains("~/") {
        let home = expand_home_path("~").to_string_lossy().into_owned();
        return cmd.replace("~/", &format!("{home}/"));
    }
    cmd.to_string()
}

/// Push OSC sequences to this TTY (if any) and every writable `/dev/pts/*`, like pywal.
pub fn apply_sequences(sequences: &[u8]) {
    if sequences.is_empty() {
        return;
    }
    let stdout = io::stdout();
    if stdout.is_terminal() {
        let mut out = stdout.lock();
        let _ = out.write_all(sequences);
        let _ = out.flush();
    }
    if let Ok(mut tty) = fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(sequences);
        let _ = tty.flush();
    }
    let Ok(entries) = fs::read_dir("/dev/pts") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(mut pts) = fs::OpenOptions::new().write(true).open(entry.path()) {
            let _ = pts.write_all(sequences);
        }
    }
}

pub fn write_cache(palette: &Palette, cache_dir: &Path) -> Result<()> {
    fs::create_dir_all(cache_dir).with_context(|| format!("creating {}", cache_dir.display()))?;
    let templates = collect_templates()?;
    for (name, body) in templates {
        let rendered =
            render(&body, palette).with_context(|| format!("rendering template {name}"))?;
        let path = cache_dir.join(&name);
        fs::write(&path, rendered).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

pub fn link_wal_compat(cache_dir: &Path, wal_dir: &Path) -> Result<()> {
    match fs::symlink_metadata(wal_dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = wal_dir.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            symlink(cache_dir, wal_dir).with_context(|| {
                format!("linking {} -> {}", wal_dir.display(), cache_dir.display())
            })?;
            return Ok(());
        }
        Err(err) => {
            return Err(err).with_context(|| format!("stat {}", wal_dir.display()));
        }
        Ok(meta) if meta.file_type().is_symlink() => return Ok(()),
        Ok(_) => {}
    }
    if !wal_dir.is_dir() {
        anyhow::bail!("{} exists and is not a directory", wal_dir.display());
    }
    for entry in fs::read_dir(cache_dir)
        .with_context(|| format!("reading {}", cache_dir.display()))?
        .flatten()
    {
        let dest = wal_dir.join(entry.file_name());
        if dest.symlink_metadata().is_ok() {
            fs::remove_file(&dest).ok();
        }
        symlink(entry.path(), &dest).with_context(|| format!("linking {}", dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Palette;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn pal() -> Palette {
        Palette {
            colors: std::array::from_fn(|i| format!("#{i:02X}8080")),
            background: "#000000".into(),
            foreground: "#FFFFFF".into(),
            cursor: "#FFFFFF".into(),
            wallpaper: "/tmp/a.png".into(),
            alpha: "100".into(),
            theme: crate::recipe::Theme::Dark,
        }
    }

    #[test]
    fn writes_cache_and_symlinks_missing_wal() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vilecolors-cache-{stamp}"));
        let cache = root.join("vile").join("wal");
        let wal = root.join("wal");
        write_cache(&pal(), &cache).unwrap();
        assert!(cache.join("colors.json").is_file());
        assert!(cache.join("colors.sh").is_file());
        assert!(cache.join("sequences").is_file());
        link_wal_compat(&cache, &wal).unwrap();
        assert!(wal.symlink_metadata().unwrap().file_type().is_symlink());
        let json = fs::read_to_string(wal.join("colors.json")).unwrap();
        assert!(json.contains("#000000"));
        assert!(json.contains("\"theme\": \"dark\""));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_sequences_ignores_empty() {
        apply_sequences(&[]);
    }
}
