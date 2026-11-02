use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::theme::{theme_kind, ThemeKind};

/// Firefox Theme API color → palette slot (`color0`…`color15`, `background`,
/// `foreground`, `cursor`) or a `#rrggbb` hex.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabClose {
    #[default]
    Always,
    Hover,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FoxConfig {
    /// Selected-tab outline width in CSS px. Written into userChrome.css.
    #[serde(default = "default_outline_px")]
    pub tab_outline_px: u32,
    /// Chrome corner radius. `0` is square (tabs, url bar, panels, buttons).
    #[serde(default)]
    pub corner_radius_px: u32,
    /// URL bar border width. `0` hides the extra stroke.
    #[serde(default = "default_urlbar_border_px")]
    pub urlbar_border_px: u32,
    /// Shorter tab strip.
    #[serde(default)]
    pub compact_tabs: bool,
    /// When to show the tab close button.
    #[serde(default)]
    pub tab_close: TabClose,
    /// Extra left/right padding on the tab strip.
    #[serde(default = "default_tab_bar_margin_px")]
    pub tab_bar_margin_px: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dark: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub light: BTreeMap<String, String>,
}

impl Default for FoxConfig {
    fn default() -> Self {
        Self {
            tab_outline_px: default_outline_px(),
            corner_radius_px: 0,
            urlbar_border_px: default_urlbar_border_px(),
            compact_tabs: false,
            tab_close: TabClose::Always,
            tab_bar_margin_px: default_tab_bar_margin_px(),
            dark: BTreeMap::new(),
            light: BTreeMap::new(),
        }
    }
}

fn default_outline_px() -> u32 {
    2
}

fn default_urlbar_border_px() -> u32 {
    1
}

fn default_tab_bar_margin_px() -> u32 {
    2
}

pub fn clamp_outline_px(n: u32) -> u32 {
    if n == 0 {
        default_outline_px()
    } else {
        n.clamp(1, 8)
    }
}

pub fn default_config() -> FoxConfig {
    FoxConfig {
        dark: default_dark(),
        light: default_light(),
        ..FoxConfig::default()
    }
}

pub fn default_config_json() -> String {
    serde_json::to_string_pretty(&default_config()).expect("vilefox defaults") + "\n"
}

pub fn load_from(text: &str) -> FoxConfig {
    let parsed: FoxConfig = serde_json::from_str(text.trim()).unwrap_or_default();
    FoxConfig {
        tab_outline_px: clamp_outline_px(parsed.tab_outline_px),
        corner_radius_px: parsed.corner_radius_px.min(16),
        urlbar_border_px: parsed.urlbar_border_px.min(4),
        compact_tabs: parsed.compact_tabs,
        tab_close: parsed.tab_close,
        tab_bar_margin_px: parsed.tab_bar_margin_px.min(24),
        dark: merge(default_dark(), parsed.dark),
        light: merge(default_light(), parsed.light),
    }
}

pub fn resolve(wal: &Value, cfg: &FoxConfig) -> Option<(ThemeKind, BTreeMap<String, String>)> {
    let kind = theme_kind(wal)?;
    let slots = match kind {
        ThemeKind::Dark => &cfg.dark,
        ThemeKind::Light => &cfg.light,
    };
    let mut colors = BTreeMap::new();
    for (field, slot) in slots {
        if let Some(hex) = lookup(wal, slot) {
            colors.insert(field.clone(), hex);
        }
    }
    if colors.is_empty() {
        return None;
    }
    Some((kind, colors))
}

fn merge(mut base: BTreeMap<String, String>, over: BTreeMap<String, String>) -> BTreeMap<String, String> {
    for (key, value) in over {
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        base.insert(key, value);
    }
    base
}

fn lookup(wal: &Value, slot: &str) -> Option<String> {
    let slot = slot.trim();
    if slot.starts_with('#') {
        return Some(slot.to_string());
    }
    if let Some(rest) = slot.strip_prefix("color") {
        if let Ok(n) = rest.parse::<u8>() {
            if n < 16 {
                return wal
                    .get("colors")
                    .and_then(|c| c.get(slot))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
    }
    wal.get("special")
        .and_then(|s| s.get(slot))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            wal.get("colors")
                .and_then(|c| c.get(slot))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn chrome(frame: &str, raised: &str, ink: &str, accent: &str, accent2: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("frame".into(), frame.into()),
        ("frame_inactive".into(), frame.into()),
        ("tab_background_text".into(), ink.into()),
        ("tab_text".into(), accent.into()),
        ("tab_line".into(), accent.into()),
        ("tab_loading".into(), accent.into()),
        ("tab_selected".into(), frame.into()),
        ("toolbar".into(), frame.into()),
        ("toolbar_text".into(), ink.into()),
        ("toolbar_field".into(), frame.into()),
        ("toolbar_field_text".into(), ink.into()),
        ("toolbar_field_border".into(), raised.into()),
        // Same as normal field fill — raised/color8 reads too light in the urlbar.
        ("toolbar_field_focus".into(), frame.into()),
        // Without these, Firefox falls back to brand blue (#0061e0 / #00ddff) on focus.
        ("toolbar_field_border_focus".into(), accent.into()),
        ("toolbar_field_text_focus".into(), ink.into()),
        ("toolbar_field_highlight".into(), accent.into()),
        ("toolbar_field_highlight_text".into(), frame.into()),
        ("toolbar_top_separator".into(), frame.into()),
        ("toolbar_bottom_separator".into(), frame.into()),
        ("toolbar_vertical_separator".into(), frame.into()),
        ("ntp_background".into(), frame.into()),
        ("ntp_text".into(), ink.into()),
        ("popup".into(), frame.into()),
        ("popup_text".into(), ink.into()),
        ("popup_border".into(), raised.into()),
        ("popup_highlight".into(), accent.into()),
        ("popup_highlight_text".into(), frame.into()),
        ("sidebar".into(), frame.into()),
        ("sidebar_text".into(), ink.into()),
        ("sidebar_highlight".into(), accent.into()),
        ("sidebar_border".into(), raised.into()),
        ("icons".into(), accent.into()),
        ("icons_attention".into(), accent2.into()),
        ("button_background_hover".into(), raised.into()),
        ("button_background_active".into(), raised.into()),
    ])
}

fn default_dark() -> BTreeMap<String, String> {
    chrome("color0", "color8", "color15", "color10", "color13")
}

fn default_light() -> BTreeMap<String, String> {
    chrome("color0", "color8", "color15", "color4", "color5")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wal() -> Value {
        json!({
            "vile": { "theme": "dark" },
            "special": { "background": "#111111", "foreground": "#eeeeee" },
            "colors": {
                "color0": "#170602",
                "color4": "#aa4444",
                "color8": "#505354",
                "color10": "#44aaaa",
                "color13": "#aa44aa",
                "color15": "#f3ebe6"
            }
        })
    }

    #[test]
    fn omit_safe_override() {
        let cfg = load_from(r#"{ "dark": { "icons": "color4" } }"#);
        assert_eq!(cfg.dark.get("icons").unwrap(), "color4");
        assert_eq!(cfg.dark.get("frame").unwrap(), "color0");
        let (_, colors) = resolve(&wal(), &cfg).unwrap();
        assert_eq!(colors.get("icons").unwrap(), "#aa4444");
        assert_eq!(colors.get("frame").unwrap(), "#170602");
    }

    #[test]
    fn hex_passthrough() {
        let cfg = load_from(r##"{ "dark": { "tab_line": "#ff00aa" } }"##);
        let (_, colors) = resolve(&wal(), &cfg).unwrap();
        assert_eq!(colors.get("tab_line").unwrap(), "#ff00aa");
    }

    #[test]
    fn empty_file_is_defaults() {
        let cfg = load_from("");
        assert_eq!(cfg, default_config());
        assert_eq!(cfg.tab_outline_px, 2);
    }

    #[test]
    fn outline_px_override() {
        let cfg = load_from(r#"{ "tab_outline_px": 4 }"#);
        assert_eq!(cfg.tab_outline_px, 4);
        assert_eq!(cfg.dark.get("frame").unwrap(), "color0");
    }

    #[test]
    fn chrome_options_parse() {
        let cfg = load_from(
            r#"{ "corner_radius_px": 0, "compact_tabs": true, "tab_close": "hover", "urlbar_border_px": 2 }"#,
        );
        assert_eq!(cfg.corner_radius_px, 0);
        assert!(cfg.compact_tabs);
        assert_eq!(cfg.tab_close, TabClose::Hover);
        assert_eq!(cfg.urlbar_border_px, 2);
    }
}
