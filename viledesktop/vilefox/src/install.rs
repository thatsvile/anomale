use std::env;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use vile_protocol::vilefox_config_path;

const HOST_NAME: &str = "vilefox";
const ADDON_ID: &str = "vilefox@vile";
const USER_CHROME_TEMPLATE: &str = include_str!("../extension/userChrome.css");
const MANIFEST: &str = include_str!("../extension/manifest.json");
const BACKGROUND: &str = include_str!("../extension/background.js");
const SIGNED_XPI: &[u8] = include_bytes!("../vilefox-signed.xpi");

pub fn run() -> Result<()> {
    let exe = env::current_exe().context("current exe")?;
    let dest = home()?.join(".local/bin").join(HOST_NAME);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let already = dest.is_file() && fs::canonicalize(&exe).ok() == fs::canonicalize(&dest).ok();
    if !already {
        fs::copy(&exe, &dest)
            .with_context(|| format!("copy {} -> {}", exe.display(), dest.display()))?;
        let mut perms = fs::metadata(&dest)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            fs::set_permissions(&dest, perms)?;
        }
    }
    let dest = dest.canonicalize().unwrap_or(dest);
    let manifest = native_manifest(&dest);
    let mozilla = home()?
        .join(".mozilla")
        .join("native-messaging-hosts")
        .join(format!("{HOST_NAME}.json"));
    write_json(&mozilla, &manifest)?;
    for dir in [
        home()?.join(".librewolf").join("native-messaging-hosts"),
        xdg_config()?.join("librewolf").join("native-messaging-hosts"),
    ] {
        fs::create_dir_all(&dir)?;
        let link = dir.join(format!("{HOST_NAME}.json"));
        let _ = fs::remove_file(&link);
        if symlink(&mozilla, &link).is_err() {
            write_json(&link, &manifest)?;
        }
    }
    let mut chrome_dirs = 0usize;
    for profile in browser_profiles()? {
        if !profile.is_dir() {
            continue;
        }
        let chrome = profile.join("chrome");
        fs::create_dir_all(&chrome)?;
        let cfg = load_cfg();
        write_user_chrome(&chrome.join("userChrome.css"), &cfg)
            .with_context(|| format!("writing {}", chrome.display()))?;
        write_user_js(&profile.join("user.js"))?;
        chrome_dirs += 1;
    }
    install_extension()?;
    let xpi = install_signed_xpi()?;
    let cfg = write_config()?;
    println!("installed native host {}", dest.display());
    println!("manifest {}", mozilla.display());
    println!("wrote userChrome.css to {chrome_dirs} profile chrome/ dirs");
    println!("edit chrome mapping {}", cfg.display());
    println!("signed add-on {}", xpi.display());
    println!("LibreWolf installs it via policies (normal_installed); Firefox: open that .xpi once");
    println!("user.js enables toolkit.legacyUserProfileCustomizations.stylesheets for userChrome.css");
    println!("disable pywalfox if it is still theming the browser");
    Ok(())
}

fn native_manifest(path: &Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "Theme Firefox from vile colors",
        "path": path,
        "type": "stdio",
        "allowed_extensions": [ADDON_ID],
    }))
    .expect("manifest json")
        + "\n"
}

pub fn user_chrome(cfg: &crate::map::FoxConfig) -> String {
    let outline = crate::map::clamp_outline_px(cfg.tab_outline_px);
    let compact = if cfg.compact_tabs {
        ":root {\n  --tab-min-height: 28px !important;\n}\n.tab-background {\n  margin-block: 1px !important;\n}\n"
    } else {
        ""
    };
    let tab_close = match cfg.tab_close {
        crate::map::TabClose::Always => "",
        crate::map::TabClose::Hover => {
            ".tabbrowser-tab:not([pinned]):not(:hover) .tab-close-button {\n  display: none !important;\n}\n"
        }
    };
    USER_CHROME_TEMPLATE
        .replace("__OUTLINE_PX__", &outline.to_string())
        .replace("__LINE_PX__", &(outline + 1).to_string())
        .replace("__RADIUS_PX__", &cfg.corner_radius_px.min(16).to_string())
        .replace(
            "__URLBAR_BORDER_PX__",
            &cfg.urlbar_border_px.min(4).to_string(),
        )
        .replace(
            "__TAB_BAR_MARGIN_PX__",
            &cfg.tab_bar_margin_px.min(24).to_string(),
        )
        .replace("__COMPACT__", compact)
        .replace("__TAB_CLOSE__", tab_close)
}

fn load_cfg() -> crate::map::FoxConfig {
    crate::map::load_from(&fs::read_to_string(vilefox_config_path()).unwrap_or_default())
}

pub fn write_all_user_chrome(cfg: &crate::map::FoxConfig) -> Result<usize> {
    let mut n = 0usize;
    for profile in browser_profiles()? {
        if !profile.is_dir() {
            continue;
        }
        let chrome = profile.join("chrome");
        fs::create_dir_all(&chrome)?;
        write_user_chrome(&chrome.join("userChrome.css"), cfg)?;
        write_user_js(&profile.join("user.js"))?;
        n += 1;
    }
    Ok(n)
}

fn write_user_chrome(path: &Path, cfg: &crate::map::FoxConfig) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let body = merge_user_chrome(&existing, &user_chrome(cfg));
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

const USER_JS: &str = "\
/* BEGIN VILEFOX */\n\
user_pref(\"widget.gtk.native-context-menus\", false);\n\
user_pref(\"toolkit.legacyUserProfileCustomizations.stylesheets\", true);\n\
/* END VILEFOX */\n";

fn write_user_js(path: &Path) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let body = merge_user_chrome(&existing, USER_JS);
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

fn merge_user_chrome(existing: &str, snippet: &str) -> String {
    const BEGIN: &str = "/* BEGIN VILEFOX */";
    const END: &str = "/* END VILEFOX */";
    if let (Some(start), Some(end)) = (existing.find(BEGIN), existing.find(END)) {
        if end >= start {
            let mut out = String::new();
            out.push_str(&existing[..start]);
            out.push_str(snippet.trim_end());
            out.push('\n');
            out.push_str(&existing[end + END.len()..]);
            return out;
        }
    }
    if existing.trim().is_empty() {
        return snippet.to_string();
    }
    let mut out = existing.trim_end().to_string();
    out.push('\n');
    out.push_str(snippet);
    if !snippet.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn write_json(path: &Path, body: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

pub fn write_config() -> Result<PathBuf> {
    let path = vilefox_config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if !path.is_file() {
        fs::write(&path, crate::map::default_config_json())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    let example = xdg_data()?.join("vilefox").join("vilefox.example.json");
    if let Some(dir) = example.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&example, crate::map::default_config_json())?;
    Ok(path)
}

fn install_extension() -> Result<PathBuf> {
    let dest = xdg_data()?.join("vilefox").join("extension");
    fs::create_dir_all(&dest)?;
    fs::write(dest.join("manifest.json"), MANIFEST)?;
    fs::write(dest.join("background.js"), BACKGROUND)?;
    fs::write(dest.join("userChrome.css"), user_chrome(&load_cfg()))?;
    Ok(dest)
}

fn install_signed_xpi() -> Result<PathBuf> {
    let dest = xdg_data()?.join("vilefox").join("vilefox-signed.xpi");
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vilefox-signed.xpi");
    if src.is_file() {
        fs::copy(&src, &dest)
            .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    } else {
        fs::write(&dest, SIGNED_XPI)
            .with_context(|| format!("writing {}", dest.display()))?;
    }
    Ok(dest)
}

fn xdg_data() -> Result<PathBuf> {
    Ok(env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().unwrap_or_else(|_| PathBuf::from("/")).join(".local/share")))
}

fn browser_profiles() -> Result<Vec<PathBuf>> {
    let home = home()?;
    let config = xdg_config()?;
    let mut roots = vec![
        config.join("librewolf").join("librewolf"),
        home.join(".librewolf"),
        home.join(".mozilla").join("firefox"),
    ];
    roots.push(config.join("librewolf"));
    let mut out = Vec::new();
    for root in roots {
        out.extend(profiles_in(&root));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn profiles_in(root: &Path) -> Vec<PathBuf> {
    let ini = root.join("profiles.ini");
    let Ok(text) = fs::read_to_string(&ini) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(path) = line.trim().strip_prefix("Path=") else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let p = PathBuf::from(path);
        out.push(if p.is_absolute() {
            p
        } else {
            root.join(p)
        });
    }
    out
}

fn home() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is unset")
}

fn xdg_config() -> Result<PathBuf> {
    Ok(env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().unwrap_or_else(|_| PathBuf::from("/")).join(".config")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_marked_block() {
        let existing = "a\n/* BEGIN VILEFOX */\nold\n/* END VILEFOX */\nb\n";
        let out = merge_user_chrome(existing, "/* BEGIN VILEFOX */\nnew\n/* END VILEFOX */\n");
        assert!(out.contains("new"));
        assert!(!out.contains("old"));
        assert!(out.starts_with("a\n"));
        assert!(out.contains("\nb\n"));
    }

    #[test]
    fn user_chrome_fills_outline_px() {
        let mut cfg = crate::map::default_config();
        cfg.tab_outline_px = 3;
        let css = user_chrome(&cfg);
        assert!(css.contains("outline-width: 3px"));
        assert!(css.contains("outline-offset: -3px"));
        assert!(css.contains("height: 4px"));
        assert!(css.contains("--tab-border-radius: 0px"));
        assert!(css.contains("--urlbar-border-radius: 0px"));
        assert!(css.contains("--focus-outline-color"));
        assert!(css.contains("padding-inline: 2px"));
        assert!(css.contains("#urlbar-background"));
        assert!(css.contains("--menu-background-color"));
        assert!(css.contains(".urlbarView"));
        assert!(!css.contains("__OUTLINE_PX__"));
        assert!(!css.contains("__TAB_BAR_MARGIN_PX__"));
        assert!(!css.contains("__COMPACT__"));
    }

    #[test]
    fn user_chrome_compact_and_hover_close() {
        let mut cfg = crate::map::default_config();
        cfg.compact_tabs = true;
        cfg.tab_close = crate::map::TabClose::Hover;
        cfg.corner_radius_px = 8;
        let css = user_chrome(&cfg);
        assert!(css.contains("--tab-min-height: 28px"));
        assert!(css.contains(".tab-close-button"));
        assert!(css.contains("--tab-border-radius: 8px"));
        assert!(css.contains("--urlbar-border-radius: 8px"));
    }

    #[test]
    fn chrome_defaults_include_field_focus_border() {
        let cfg = crate::map::default_config();
        assert_eq!(cfg.dark.get("toolbar_field_border_focus").unwrap(), "color10");
        assert_eq!(cfg.dark.get("toolbar_field_text_focus").unwrap(), "color15");
    }
}
