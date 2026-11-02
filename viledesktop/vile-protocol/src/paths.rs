use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::{Config, WallpaperState};

/// Expand `~` and `~/…`. Other paths are returned as-is.
pub fn expand_home_path(raw: &str) -> PathBuf {
    let raw = raw.trim();
    if raw == "~" {
        return home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(raw)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn wallpaper_directory(raw: &str) -> Option<PathBuf> {
    if raw.trim().is_empty() {
        return None;
    }
    Some(expand_home_path(raw))
}

pub fn config_dir() -> PathBuf {
    xdg_config_home().join("vile")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn vilefox_config_path() -> PathBuf {
    config_dir().join("vilefox.json")
}

pub fn colors_dir() -> PathBuf {
    config_dir().join("colors")
}

pub fn palettes_dir() -> PathBuf {
    colors_dir().join("palettes")
}

pub fn templates_dir() -> PathBuf {
    colors_dir().join("templates")
}

pub fn wal_templates_dir() -> PathBuf {
    xdg_config_home().join("wal").join("templates")
}

fn xdg_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
}

pub fn color_cache_dir() -> PathBuf {
    cache_home().join("vile").join("wal")
}

pub fn wal_compat_dir() -> PathBuf {
    cache_home().join("wal")
}

pub fn wallpaper_state_path() -> PathBuf {
    cache_home().join("vile").join("wallpaper.json")
}

pub fn load_wallpaper_state() -> Option<WallpaperState> {
    let text = fs::read_to_string(wallpaper_state_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_wallpaper_state(state: &WallpaperState) -> Result<()> {
    let path = wallpaper_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, format!("{json}\n")).with_context(|| format!("writing {}", path.display()))?;
    write_wal_path(&state.image)?;
    Ok(())
}

/// Update the pywal-compat `wal` path file (wallpaper path + newline) without re-exporting colors.
pub fn write_wal_path(image: &str) -> Result<()> {
    let cache = color_cache_dir();
    fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    let path = cache.join("wal");
    fs::write(&path, format!("{image}\n")).with_context(|| format!("writing {}", path.display()))?;
    let wal_dir = wal_compat_dir();
    match fs::symlink_metadata(&wal_dir) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(()),
        Ok(_) if wal_dir.is_dir() => {
            let dest = wal_dir.join("wal");
            if dest.symlink_metadata().is_ok() {
                let _ = fs::remove_file(&dest);
            }
            std::os::unix::fs::symlink(&path, &dest)
                .with_context(|| format!("linking {}", dest.display()))?;
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = wal_dir.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::os::unix::fs::symlink(&cache, &wal_dir).with_context(|| {
                format!("linking {} -> {}", wal_dir.display(), cache.display())
            })?;
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("stat {}", wal_dir.display())),
    }
}

/// Stem used as `~/.config/vile/colors/palettes/{name}.json`. Empty means invalid.
pub fn sanitize_recipe_name(name: &str) -> String {
    let stem = name.trim().replace('\\', "/");
    let stem = stem.rsplit('/').next().unwrap_or("");
    let stem = if stem.len() >= 5 && stem[stem.len() - 5..].eq_ignore_ascii_case(".json") {
        &stem[..stem.len() - 5]
    } else {
        stem
    };
    let mut cleaned = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            cleaned.push(ch);
        } else if !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let cleaned = cleaned.trim_matches('-');
    cleaned.chars().take(64).collect()
}

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("vile"))
        .join("vile")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("vile.sock")
}

pub fn load_config() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut config: Config =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if config.bars.is_empty() {
        config.bars = Config::default().bars;
    }
    Ok(config)
}

pub fn ensure_config_file() -> Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = config_path();
    if !path.exists() {
        let json = serde_json::to_string_pretty(&Config::default())?;
        let mut file =
            fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
    }
    Ok(path)
}

pub fn ensure_runtime_dir() -> Result<PathBuf> {
    let dir = runtime_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::sanitize_recipe_name;

    #[test]
    fn expands_tilde_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        assert_eq!(
            super::expand_home_path("~"),
            std::path::PathBuf::from(&home)
        );
        assert_eq!(
            super::expand_home_path("~/Wallpapers"),
            std::path::PathBuf::from(&home).join("Wallpapers")
        );
        assert_eq!(
            super::expand_home_path("/tmp/x"),
            std::path::PathBuf::from("/tmp/x")
        );
        assert!(super::wallpaper_directory("").is_none());
        assert!(super::wallpaper_directory("  ").is_none());
    }

    #[test]
    fn sanitizes_recipe_stems() {
        assert_eq!(sanitize_recipe_name("exported"), "exported");
        assert_eq!(sanitize_recipe_name("exported.json"), "exported");
        assert_eq!(sanitize_recipe_name("  Foo Bar.JSON  "), "Foo-Bar");
        assert_eq!(sanitize_recipe_name("/tmp/weird name.json"), "weird-name");
        assert_eq!(sanitize_recipe_name("***"), "");
        let long = "a".repeat(80);
        assert_eq!(sanitize_recipe_name(&long).len(), 64);
    }
}
