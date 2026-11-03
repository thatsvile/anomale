use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::paths::color_cache_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_mode_timeout_ms")]
    pub mode_timeout_ms: u64,
    #[serde(default = "default_terminal")]
    pub terminal: String,
    #[serde(default)]
    pub bars: Vec<BarConfig>,
    #[serde(default)]
    pub wallpaper: WallpaperConfig,
    #[serde(default)]
    pub screenshot: ScreenshotConfig,
    #[serde(default)]
    pub animations: AnimationConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub tray: TrayConfig,
    #[serde(default)]
    pub menu: MenuConfig,
    /// Shell commands run after every successful colors apply (`export_palette`).
    /// Not run for wallpaper-only or `vile --refresh`. Each entry is `sh -c`.
    /// Used when the theme-specific list for the applied palette is empty.
    #[serde(default)]
    pub colors_apply_commands: Vec<String>,
    /// When non-empty, used instead of `colors_apply_commands` for dark Theme base.
    #[serde(default)]
    pub colors_apply_commands_dark: Vec<String>,
    /// When non-empty, used instead of `colors_apply_commands` for light Theme base.
    #[serde(default)]
    pub colors_apply_commands_light: Vec<String>,
}

impl Config {
    /// Theme-specific apply commands if non-empty, else `colors_apply_commands`.
    /// `theme` is `"dark"` or `"light"` (colorlab Theme base / `vile.theme`).
    pub fn colors_apply_commands_for(&self, theme: &str) -> &[String] {
        let specific = match theme {
            "light" => &self.colors_apply_commands_light,
            _ => &self.colors_apply_commands_dark,
        };
        if !specific.is_empty() {
            specific
        } else {
            &self.colors_apply_commands
        }
    }
}

fn default_mode_timeout_ms() -> u64 {
    2000
}

fn default_terminal() -> String {
    "foot".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode_timeout_ms: default_mode_timeout_ms(),
            terminal: default_terminal(),
            bars: vec![BarConfig::default()],
            wallpaper: WallpaperConfig::default(),
            screenshot: ScreenshotConfig::default(),
            animations: AnimationConfig::default(),
            notifications: NotificationConfig::default(),
            tray: TrayConfig::default(),
            menu: MenuConfig::default(),
            colors_apply_commands: Vec::new(),
            colors_apply_commands_dark: Vec::new(),
            colors_apply_commands_light: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperMode {
    #[default]
    Fill,
    #[serde(alias = "scale")]
    Fit,
    Stretch,
    Center,
    Tile,
    Span,
}

impl WallpaperMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fill" => Some(Self::Fill),
            "fit" | "scale" => Some(Self::Fit),
            "stretch" => Some(Self::Stretch),
            "center" => Some(Self::Center),
            "tile" => Some(Self::Tile),
            "span" => Some(Self::Span),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Fit => "fit",
            Self::Stretch => "stretch",
            Self::Center => "center",
            Self::Tile => "tile",
            Self::Span => "span",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperConfig {
    #[serde(default)]
    pub mode: WallpaperMode,
    #[serde(default = "default_monitors")]
    pub monitors: Vec<String>,
    /// Folder of images for the color-lab / `--colors` / `--wall` pickers. Empty means unset.
    #[serde(default)]
    pub directory: String,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            mode: WallpaperMode::Fill,
            monitors: default_monitors(),
            directory: String::new(),
        }
    }
}

impl WallpaperConfig {
    pub fn matches_output(&self, output_name: &str) -> bool {
        self.monitors
            .iter()
            .any(|monitor| monitor == "*" || monitor == output_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotConfig {
    #[serde(default)]
    pub save: bool,
    #[serde(default = "default_screenshot_directory")]
    pub directory: String,
    /// Seconds to keep the PNG on the clipboard before exiting. `0` holds until replaced.
    #[serde(default = "default_screenshot_clipboard_seconds")]
    pub clipboard_seconds: u64,
}

fn default_screenshot_directory() -> String {
    "~/Pictures".into()
}

fn default_screenshot_clipboard_seconds() -> u64 {
    20
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            save: false,
            directory: default_screenshot_directory(),
            clipboard_seconds: default_screenshot_clipboard_seconds(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_transition_frames() -> u8 {
    3
}

fn default_navigation_frames() -> u8 {
    6
}

fn default_anim_frame_ms() -> u64 {
    25
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_transition_frames", alias = "frames")]
    pub transition_frames: u8,
    #[serde(default = "default_navigation_frames")]
    pub navigation_frames: u8,
    #[serde(default = "default_anim_frame_ms")]
    pub frame_ms: u64,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transition_frames: default_transition_frames(),
            navigation_frames: default_navigation_frames(),
            frame_ms: default_anim_frame_ms(),
        }
    }
}

impl AnimationConfig {
    pub fn transition_frames(&self) -> u8 {
        self.transition_frames.clamp(1, 6)
    }

    pub fn navigation_frames(&self) -> u8 {
        self.navigation_frames.clamp(2, 12)
    }

    pub fn frame_ms(&self) -> u64 {
        self.frame_ms.clamp(16, 80)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NotificationSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub side: NotificationSide,
    #[serde(default = "default_toast_ms")]
    pub toast_ms: u64,
    #[serde(default = "default_notification_history")]
    pub history: usize,
}

fn default_toast_ms() -> u64 {
    2000
}

fn default_notification_history() -> usize {
    24
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            side: NotificationSide::Right,
            toast_ms: default_toast_ms(),
            history: default_notification_history(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MenuItem {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MenuConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_menu_items")]
    pub items: Vec<MenuItem>,
}

pub fn default_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem {
            label: "shutdown".into(),
            command: "systemctl poweroff".into(),
        },
        MenuItem {
            label: "log out".into(),
            command: "niri msg action quit --skip-confirmation".into(),
        },
        MenuItem {
            label: "reboot".into(),
            command: "systemctl reboot".into(),
        },
    ]
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            items: default_menu_items(),
        }
    }
}

impl NotificationConfig {
    pub fn history(&self) -> usize {
        self.history.clamp(1, 64)
    }

    pub fn toast_ms(&self, expire_timeout_ms: Option<i32>) -> u64 {
        if self.toast_ms > 0 {
            return self.toast_ms;
        }
        match expire_timeout_ms {
            Some(ms) if ms > 0 => ms as u64,
            _ => default_toast_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WallpaperState {
    pub image: String,
    pub mode: WallpaperMode,
}

impl Config {
    pub fn bar(&self, name: &str) -> Option<&BarConfig> {
        self.bars.iter().find(|bar| bar.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Top,
    Bottom,
}

impl Default for Position {
    fn default() -> Self {
        Self::Top
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarConfig {
    pub name: String,
    #[serde(default)]
    pub position: Position,
    #[serde(default = "default_monitors")]
    pub monitors: Vec<String>,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_margin_vertical")]
    pub margin_vertical: u32,
    #[serde(default = "default_margin_horizontal")]
    pub margin_horizontal: u32,
    /// Gap from the bound screen edge (top or bottom).
    #[serde(default)]
    pub padding_vertical: u32,
    /// Gap from the left and right screen edges.
    #[serde(default)]
    pub padding_horizontal: u32,
    /// Inner outline thickness in px. Color is `foreground`. `0` disables it.
    #[serde(default)]
    pub border: u32,
    /// Quarter-circle radius in px. `0` keeps square corners.
    #[serde(default)]
    pub corner_radius: u32,
    #[serde(default = "default_font")]
    pub font: String,
    /// Hex (`#rrggbb`) or a palette slot (`color4`, `background`, `foreground`).
    #[serde(default = "default_foreground")]
    pub foreground: String,
    /// Hex (`#rrggbb`) or a palette slot (`color0`, `background`).
    #[serde(default = "default_background")]
    pub background: String,
    #[serde(default)]
    pub modules: Modules,
    /// Full-bar volume overlay trigger: `never` / `scrolling` / `buttons` / `both`.
    /// The old `volume_meter` key is still accepted.
    #[serde(default, alias = "volume_meter")]
    pub volume_overlay: OverlayMode,
    /// Compact volume module: `numbers` (`VOL 40`) or `meter` (`VOL ░▒▓█`).
    #[serde(default)]
    pub volume_style: MeterStyle,
    /// Full-bar battery overlay trigger: `never` / `scrolling` / `buttons` / `both`.
    #[serde(default)]
    pub battery_overlay: OverlayMode,
    /// Compact battery module: `numbers` (`BAT 87+`) or `meter` (`BAT ░▒▓█`).
    #[serde(default)]
    pub battery_style: MeterStyle,
    /// Full-bar memory overlay trigger: `never` / `scrolling` / `buttons` / `both`.
    #[serde(default)]
    pub mem_overlay: OverlayMode,
    /// Compact memory module: `numbers` (`MEM 43`) or `meter` (`MEM ░▒▓█`).
    #[serde(default)]
    pub mem_style: MeterStyle,
    /// Full-bar CPU overlay trigger: `never` / `scrolling` / `buttons` / `both`.
    #[serde(default)]
    pub cpu_overlay: OverlayMode,
    /// Compact CPU module: `numbers` or `meter`.
    #[serde(default)]
    pub cpu_style: MeterStyle,
    /// Which CPU value the compact module shows: `load` or `temp`.
    #[serde(default)]
    pub cpu_metric: HwMetric,
    /// Full-bar GPU overlay trigger: `never` / `scrolling` / `buttons` / `both`.
    #[serde(default)]
    pub gpu_overlay: OverlayMode,
    /// Compact GPU module: `numbers` or `meter`.
    #[serde(default)]
    pub gpu_style: MeterStyle,
    /// Which GPU value the compact module shows: `load` or `temp`.
    #[serde(default)]
    pub gpu_metric: HwMetric,
    #[serde(default = "default_clock_format")]
    pub clock_format: String,
}

fn default_monitors() -> Vec<String> {
    vec!["*".into()]
}

fn default_height() -> u32 {
    28
}

fn default_margin_vertical() -> u32 {
    2
}

fn default_margin_horizontal() -> u32 {
    8
}

fn default_font() -> String {
    "monospace".into()
}

fn default_foreground() -> String {
    "#e0e0e0".into()
}

fn default_background() -> String {
    "#1a1a1a".into()
}

fn default_clock_format() -> String {
    "%H:%M:%S".into()
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            name: "main".into(),
            position: Position::Top,
            monitors: default_monitors(),
            height: default_height(),
            margin_vertical: default_margin_vertical(),
            margin_horizontal: default_margin_horizontal(),
            padding_vertical: 0,
            padding_horizontal: 0,
            border: 0,
            corner_radius: 0,
            font: default_font(),
            foreground: default_foreground(),
            background: default_background(),
            modules: Modules::default(),
            volume_overlay: OverlayMode::default(),
            volume_style: MeterStyle::default(),
            battery_overlay: OverlayMode::default(),
            battery_style: MeterStyle::default(),
            mem_overlay: OverlayMode::default(),
            mem_style: MeterStyle::default(),
            cpu_overlay: OverlayMode::default(),
            cpu_style: MeterStyle::default(),
            cpu_metric: HwMetric::default(),
            gpu_overlay: OverlayMode::default(),
            gpu_style: MeterStyle::default(),
            gpu_metric: HwMetric::default(),
            clock_format: default_clock_format(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterStyle {
    #[default]
    Numbers,
    Meter,
}

impl Serialize for MeterStyle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Numbers => "numbers",
            Self::Meter => "meter",
        })
    }
}

impl<'de> Deserialize<'de> for MeterStyle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_lowercase().as_str() {
            "meter" | "both" => Self::Meter,
            _ => Self::Numbers,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HwMetric {
    #[default]
    Load,
    Temp,
}

impl Serialize for HwMetric {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Load => "load",
            Self::Temp => "temp",
        })
    }
}

impl<'de> Deserialize<'de> for HwMetric {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_lowercase().as_str() {
            "temp" | "temperature" => Self::Temp,
            _ => Self::Load,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMode {
    Never,
    Scrolling,
    Buttons,
    #[default]
    Both,
}

impl OverlayMode {
    pub fn from_scroll(self) -> bool {
        matches!(self, Self::Scrolling | Self::Both)
    }

    pub fn from_buttons(self) -> bool {
        matches!(self, Self::Buttons | Self::Both)
    }
}

impl Serialize for OverlayMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Never => "never",
            Self::Scrolling => "scrolling",
            Self::Buttons => "buttons",
            Self::Both => "both",
        })
    }
}

impl<'de> Deserialize<'de> for OverlayMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_lowercase().as_str() {
            "never" => Self::Never,
            "scrolling" => Self::Scrolling,
            "buttons" => Self::Buttons,
            _ => Self::Both,
        })
    }
}

/// Which sides of the bar are inset from the screen and should get a border.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BarEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl BarEdges {
    pub fn round_tl(self) -> bool {
        self.top && self.left
    }

    pub fn round_tr(self) -> bool {
        self.top && self.right
    }

    pub fn round_bl(self) -> bool {
        self.bottom && self.left
    }

    pub fn round_br(self) -> bool {
        self.bottom && self.right
    }

    pub fn vertical_count(self) -> u32 {
        u32::from(self.top) + u32::from(self.bottom)
    }

    pub fn horizontal_count(self) -> u32 {
        u32::from(self.left) + u32::from(self.right)
    }

    pub fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

impl BarConfig {
    /// Layer-shell margins: `(top, right, bottom, left)` in surface-local px.
    /// Vertical padding is applied only on the bound edge.
    pub fn layer_margins(&self) -> (i32, i32, i32, i32) {
        let vertical = i32::try_from(self.padding_vertical).unwrap_or(i32::MAX);
        let horizontal = i32::try_from(self.padding_horizontal).unwrap_or(i32::MAX);
        match self.position {
            Position::Top => (vertical, horizontal, 0, horizontal),
            Position::Bottom => (0, horizontal, vertical, horizontal),
        }
    }

    /// Sides not against a screen edge. The inner edge (toward windows) is
    /// never flush, so it keeps a border whenever one is configured.
    pub fn drawn_edges(&self) -> BarEdges {
        let sides = self.padding_horizontal > 0;
        let bound = self.padding_vertical > 0;
        match self.position {
            Position::Top => BarEdges {
                top: bound,
                right: sides,
                bottom: true,
                left: sides,
            },
            Position::Bottom => BarEdges {
                top: true,
                right: sides,
                bottom: bound,
                left: sides,
            },
        }
    }

    /// Layer-shell height: configured bar height plus a border band per drawn vertical side.
    pub fn surface_height(&self) -> u32 {
        self.height.saturating_add(
            self.border
                .saturating_mul(self.drawn_edges().vertical_count()),
        )
    }

    pub fn exclusive_zone(&self) -> i32 {
        i32::try_from(self.surface_height()).unwrap_or(i32::MAX)
    }

    pub fn matches_output(&self, output_name: &str) -> bool {
        self.monitors
            .iter()
            .any(|monitor| monitor == "*" || monitor == output_name)
    }

    pub fn fg_argb(&self) -> u32 {
        resolve_color(&self.foreground).unwrap_or(0xFFE0E0E0)
    }

    pub fn bg_argb(&self) -> u32 {
        resolve_color(&self.background).unwrap_or(0xFF1A1A1A)
    }

    /// Clamp border and text margins so at least one pixel of inner text remains.
    /// Border thickness is reserved only on sides that are actually drawn.
    pub fn text_insets(&self, width: u32, height: u32) -> TextInsets {
        let edges = self.drawn_edges();
        let max_x = width.saturating_sub(1) / 2;
        let max_y = height.saturating_sub(1) / 2;
        let border = self.border.min(max_x).min(max_y);
        let left = if edges.left { border } else { 0 };
        let right = if edges.right { border } else { 0 };
        let top = if edges.top { border } else { 0 };
        let bottom = if edges.bottom { border } else { 0 };
        let inner_w = width.saturating_sub(left.saturating_add(right));
        let inner_h = height.saturating_sub(top.saturating_add(bottom)).max(1);
        let margin_y = if inner_h <= 1 {
            0
        } else {
            self.margin_vertical.min((inner_h - 1) / 2)
        };
        let margin_x = if inner_w <= 1 {
            0
        } else {
            self.margin_horizontal.min((inner_w - 1) / 2)
        };
        TextInsets {
            border,
            left,
            top,
            margin_x,
            margin_y,
            inner_width: inner_w.saturating_sub(margin_x.saturating_mul(2)),
            inner_height: inner_h.saturating_sub(margin_y.saturating_mul(2)).max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextInsets {
    pub border: u32,
    pub left: u32,
    pub top: u32,
    pub margin_x: u32,
    pub margin_y: u32,
    pub inner_width: u32,
    pub inner_height: u32,
}

impl TextInsets {
    pub fn origin_x(&self) -> u32 {
        self.left.saturating_add(self.margin_x)
    }

    pub fn origin_y(&self) -> u32 {
        self.top.saturating_add(self.margin_y)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modules {
    #[serde(default = "default_left")]
    pub left: Vec<String>,
    #[serde(default = "default_center")]
    pub center: Vec<String>,
    #[serde(default = "default_right")]
    pub right: Vec<String>,
}

fn default_left() -> Vec<String> {
    vec!["workspaces".into(), "clock".into()]
}

fn default_center() -> Vec<String> {
    Vec::new()
}

fn default_right() -> Vec<String> {
    vec![
        "cpu".into(),
        "gpu".into(),
        "mem".into(),
        "volume".into(),
        "battery".into(),
    ]
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            left: default_left(),
            center: default_center(),
            right: default_right(),
        }
    }
}

pub fn parse_hex_color(value: &str) -> Option<u32> {
    let value = value.trim().trim_start_matches('#');
    match value.len() {
        6 => u32::from_str_radix(value, 16)
            .ok()
            .map(|rgb| 0xFF000000 | rgb),
        8 => u32::from_str_radix(value, 16).ok(),
        _ => None,
    }
}

/// Resolve `#rrggbb` or a wal palette slot (`color4`, `background`, `foreground`, `cursor`).
pub fn resolve_color(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(color) = parse_hex_color(value) {
        return Some(color);
    }
    cached_palette().get(&value.to_ascii_lowercase()).copied()
}

#[derive(Deserialize)]
struct WalFile {
    #[serde(default)]
    special: WalSpecial,
    #[serde(default)]
    colors: HashMap<String, String>,
}

#[derive(Default, Deserialize)]
struct WalSpecial {
    background: Option<String>,
    foreground: Option<String>,
    cursor: Option<String>,
}

struct PaletteCache {
    mtime: Option<SystemTime>,
    colors: HashMap<String, u32>,
}

fn palette_lock() -> &'static Mutex<PaletteCache> {
    static CACHE: OnceLock<Mutex<PaletteCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(PaletteCache {
            mtime: None,
            colors: HashMap::new(),
        })
    })
}

fn cached_palette() -> HashMap<String, u32> {
    let path = color_cache_dir().join("colors.json");
    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok());
    let mut cache = palette_lock().lock().unwrap_or_else(|err| err.into_inner());
    if cache.mtime == mtime && !cache.colors.is_empty() {
        return cache.colors.clone();
    }
    let colors = load_wal_palette(&path);
    cache.mtime = mtime;
    cache.colors = colors.clone();
    colors
}

fn load_wal_palette(path: &std::path::Path) -> HashMap<String, u32> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    palette_from_wal_json(&text)
}

fn palette_from_wal_json(text: &str) -> HashMap<String, u32> {
    let Ok(file) = serde_json::from_str::<WalFile>(text) else {
        return HashMap::new();
    };
    let mut colors = HashMap::new();
    let mut insert = |name: &str, value: &str| {
        if let Some(argb) = parse_hex_color(value) {
            colors.insert(name.to_ascii_lowercase(), argb);
        }
    };
    if let Some(value) = &file.special.background {
        insert("background", value);
    }
    if let Some(value) = &file.special.foreground {
        insert("foreground", value);
    }
    if let Some(value) = &file.special.cursor {
        insert("cursor", value);
    }
    for (name, value) in &file.colors {
        insert(name, value);
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::parse_hex_color;

    #[test]
    fn wallpaper_defaults_when_omitted() {
        let cfg: super::Config = serde_json::from_str(r#"{"bars":[{"name":"main"}]}"#).unwrap();
        assert_eq!(cfg.wallpaper.mode, super::WallpaperMode::Fill);
        assert_eq!(cfg.wallpaper.monitors, vec!["*"]);
        assert_eq!(cfg.wallpaper.directory, "");
        assert_eq!(
            serde_json::from_str::<super::WallpaperMode>("\"scale\"").unwrap(),
            super::WallpaperMode::Fit
        );
        assert_eq!(
            super::WallpaperMode::parse("SCALE"),
            Some(super::WallpaperMode::Fit)
        );
        assert_eq!(
            super::WallpaperMode::parse("fill"),
            Some(super::WallpaperMode::Fill)
        );
        assert_eq!(super::WallpaperMode::parse("nope"), None);
        let shipped = super::Config::default();
        assert_eq!(shipped.wallpaper.mode, super::WallpaperMode::Fill);
        assert_eq!(shipped.wallpaper.monitors, vec!["*"]);
        assert_eq!(shipped.wallpaper.directory, "");
        assert_eq!(cfg.terminal, "foot");
        assert_eq!(shipped.terminal, "foot");
        assert!(!cfg.screenshot.save);
        assert_eq!(cfg.screenshot.directory, "~/Pictures");
        assert_eq!(cfg.screenshot.clipboard_seconds, 20);
        assert!(!shipped.screenshot.save);
        assert_eq!(shipped.screenshot.directory, "~/Pictures");
        assert_eq!(shipped.screenshot.clipboard_seconds, 20);
        assert!(cfg.animations.enabled);
        assert_eq!(cfg.animations.transition_frames(), 3);
        assert_eq!(cfg.animations.navigation_frames(), 6);
        assert_eq!(cfg.animations.frame_ms(), 25);
        assert!(cfg.notifications.enabled);
        assert_eq!(cfg.notifications.side, super::NotificationSide::Right);
        assert_eq!(cfg.notifications.toast_ms, 2000);
        assert_eq!(cfg.notifications.history(), 24);
        assert!(cfg.tray.enabled);
        assert!(cfg.menu.enabled);
        assert_eq!(cfg.menu.items, super::default_menu_items());
        assert!(cfg.colors_apply_commands.is_empty());
        assert!(shipped.animations.enabled);
        assert!(shipped.notifications.enabled);
        assert_eq!(shipped.notifications.side, super::NotificationSide::Right);
        assert!(shipped.tray.enabled);
        assert!(shipped.menu.enabled);
        assert_eq!(shipped.menu.items, super::default_menu_items());
        assert!(shipped.colors_apply_commands.is_empty());
    }

    #[test]
    fn colors_apply_commands_parse() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"colors_apply_commands":["niri msg action load-config-file","~/.local/bin/vile-refresh.sh"],"bars":[{"name":"main"}]}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.colors_apply_commands,
            vec![
                "niri msg action load-config-file".to_string(),
                "~/.local/bin/vile-refresh.sh".to_string(),
            ]
        );
        assert!(cfg.colors_apply_commands_dark.is_empty());
        assert!(cfg.colors_apply_commands_light.is_empty());
        assert_eq!(
            cfg.colors_apply_commands_for("dark"),
            cfg.colors_apply_commands.as_slice()
        );
        assert_eq!(
            cfg.colors_apply_commands_for("light"),
            cfg.colors_apply_commands.as_slice()
        );
    }

    #[test]
    fn colors_apply_commands_theme_specific() {
        let cfg: super::Config = serde_json::from_str(
            r#"{
              "colors_apply_commands":["fallback"],
              "colors_apply_commands_dark":["dark-hook"],
              "colors_apply_commands_light":["light-hook"],
              "bars":[{"name":"main"}]
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.colors_apply_commands_for("dark"), &["dark-hook".to_string()]);
        assert_eq!(cfg.colors_apply_commands_for("light"), &["light-hook".to_string()]);
        assert_eq!(cfg.colors_apply_commands_for("other"), &["dark-hook".to_string()]);
    }

    #[test]
    fn screenshot_config_parses_outside_bars() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"screenshot":{"save":true,"directory":"/tmp/shots","clipboard_seconds":8},"bars":[{"name":"main"}]}"#,
        )
        .unwrap();
        assert!(cfg.screenshot.save);
        assert_eq!(cfg.screenshot.directory, "/tmp/shots");
        assert_eq!(cfg.screenshot.clipboard_seconds, 8);
        let defaults: super::Config =
            serde_json::from_str(r#"{"screenshot":{"save":true},"bars":[{"name":"main"}]}"#)
                .unwrap();
        assert_eq!(defaults.screenshot.clipboard_seconds, 20);
    }

    #[test]
    fn animations_and_notifications_parse_outside_bars() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"animations":{"enabled":false,"transition_frames":8,"navigation_frames":1,"frame_ms":8},"notifications":{"enabled":false,"side":"left","toast_ms":0,"history":3},"bars":[{"name":"main"}]}"#,
        )
        .unwrap();
        assert!(!cfg.animations.enabled);
        assert_eq!(cfg.animations.transition_frames(), 6);
        assert_eq!(cfg.animations.navigation_frames(), 2);
        assert_eq!(cfg.animations.frame_ms(), 16);
        let alias: super::Config = serde_json::from_str(
            r#"{"animations":{"frames":4,"navigation_frames":8,"frame_ms":40},"bars":[{"name":"main"}]}"#,
        )
        .unwrap();
        assert_eq!(alias.animations.transition_frames(), 4);
        assert_eq!(alias.animations.navigation_frames(), 8);
        assert_eq!(alias.animations.frame_ms(), 40);
        assert!(!cfg.notifications.enabled);
        assert_eq!(cfg.notifications.side, super::NotificationSide::Left);
        assert_eq!(cfg.notifications.toast_ms(Some(1500)), 1500);
        assert_eq!(cfg.notifications.toast_ms(Some(0)), 2000);
        assert_eq!(cfg.notifications.history(), 3);
        assert!(cfg.tray.enabled);
        let tray: super::Config =
            serde_json::from_str(r#"{"tray":{"enabled":false},"bars":[{"name":"main"}]}"#).unwrap();
        assert!(!tray.tray.enabled);
        assert!(cfg.menu.enabled);
        assert_eq!(cfg.menu.items, super::default_menu_items());
        let menu: super::Config = serde_json::from_str(
            r#"{"menu":{"enabled":true,"items":[{"label":"lock","command":"loginctl lock-session"}]},"bars":[{"name":"main"}]}"#,
        )
        .unwrap();
        assert_eq!(menu.menu.items.len(), 1);
        assert_eq!(menu.menu.items[0].label, "lock");
        assert_eq!(menu.menu.items[0].command, "loginctl lock-session");
        let empty: super::Config =
            serde_json::from_str(r#"{"menu":{"items":[]},"bars":[{"name":"main"}]}"#).unwrap();
        assert!(empty.menu.enabled);
        assert!(empty.menu.items.is_empty());
    }

    #[test]
    fn parses_rgb_and_argb() {
        assert_eq!(parse_hex_color("#e0e0e0"), Some(0xFFE0E0E0));
        assert_eq!(parse_hex_color("1a1a1a"), Some(0xFF1A1A1A));
        assert_eq!(parse_hex_color("#80FF0000"), Some(0x80FF0000));
        assert_eq!(parse_hex_color("bad"), None);
    }

    #[test]
    fn palette_slots_from_wal_json() {
        let json = "{
            \"special\": { \"background\": \"#112233\", \"foreground\": \"#AABBCC\", \"cursor\": \"#AABBCC\" },
            \"colors\": { \"color0\": \"#112233\", \"color4\": \"#98933E\" }
        }";
        let map = super::palette_from_wal_json(json);
        assert_eq!(map.get("color4").copied(), Some(0xFF98933E));
        assert_eq!(map.get("background").copied(), Some(0xFF112233));
        assert_eq!(super::parse_hex_color("#98933E"), Some(0xFF98933E));
    }

    #[test]
    fn text_insets_leave_inner_box() {
        let mut bar = super::BarConfig::default();
        bar.height = 18;
        bar.margin_vertical = 2;
        bar.margin_horizontal = 8;
        let insets = bar.text_insets(1920, 18);
        assert_eq!(insets.border, 0);
        assert_eq!(insets.margin_x, 8);
        assert_eq!(insets.margin_y, 2);
        assert_eq!(insets.origin_x(), 8);
        assert_eq!(insets.origin_y(), 2);
        assert_eq!(insets.inner_width, 1904);
        assert_eq!(insets.inner_height, 14);
    }

    #[test]
    fn text_insets_clamp_oversize_margins() {
        let mut bar = super::BarConfig::default();
        bar.margin_vertical = 100;
        bar.margin_horizontal = 5000;
        let insets = bar.text_insets(100, 10);
        assert_eq!(insets.inner_height, 2);
        assert_eq!(insets.inner_width, 2);
    }

    #[test]
    fn text_insets_reserve_border() {
        let mut bar = super::BarConfig::default();
        bar.height = 18;
        bar.margin_vertical = 2;
        bar.margin_horizontal = 8;
        bar.border = 1;
        bar.padding_vertical = 8;
        bar.padding_horizontal = 12;
        let insets = bar.text_insets(1920, bar.surface_height());
        assert_eq!(insets.border, 1);
        assert_eq!(insets.left, 1);
        assert_eq!(insets.top, 1);
        assert_eq!(insets.margin_x, 8);
        assert_eq!(insets.margin_y, 2);
        assert_eq!(insets.origin_x(), 9);
        assert_eq!(insets.origin_y(), 3);
        assert_eq!(insets.inner_width, 1902);
        assert_eq!(insets.inner_height, 14);
        bar.padding_vertical = 0;
        bar.padding_horizontal = 0;
        bar.border = 100;
        let clamped = bar.text_insets(100, 10);
        assert_eq!(clamped.border, 4);
        assert_eq!(clamped.left, 0);
        assert_eq!(clamped.top, 0);
        assert_eq!(clamped.inner_height, 2);
    }

    #[test]
    fn volume_meter_mode_parses_and_defaults() {
        use super::{BarConfig, OverlayMode};
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"never\"").unwrap(),
            OverlayMode::Never
        );
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"scrolling\"").unwrap(),
            OverlayMode::Scrolling
        );
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"buttons\"").unwrap(),
            OverlayMode::Buttons
        );
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"both\"").unwrap(),
            OverlayMode::Both
        );
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"nope\"").unwrap(),
            OverlayMode::Both
        );
        let bar: BarConfig = serde_json::from_str(r#"{"name":"main"}"#).unwrap();
        assert_eq!(bar.volume_overlay, OverlayMode::Both);
        assert_eq!(bar.battery_overlay, OverlayMode::Both);
        let legacy: BarConfig =
            serde_json::from_str(r#"{"name":"main","volume_meter":"scrolling"}"#).unwrap();
        assert_eq!(legacy.volume_overlay, OverlayMode::Scrolling);
        let named: BarConfig = serde_json::from_str(
            r#"{"name":"main","volume_overlay":"never","battery_overlay":"buttons"}"#,
        )
        .unwrap();
        assert_eq!(named.volume_overlay, OverlayMode::Never);
        assert_eq!(named.battery_overlay, OverlayMode::Buttons);
        assert_eq!(bar.volume_style, super::MeterStyle::Numbers);
        assert_eq!(bar.battery_style, super::MeterStyle::Numbers);
        assert_eq!(bar.mem_style, super::MeterStyle::Numbers);
        assert_eq!(bar.mem_overlay, OverlayMode::Both);
        assert_eq!(bar.cpu_style, super::MeterStyle::Numbers);
        assert_eq!(bar.cpu_metric, super::HwMetric::Load);
        assert_eq!(bar.gpu_style, super::MeterStyle::Numbers);
        assert_eq!(bar.gpu_metric, super::HwMetric::Load);
        assert_eq!(
            serde_json::from_str::<super::HwMetric>("\"temp\"").unwrap(),
            super::HwMetric::Temp
        );
        assert_eq!(
            serde_json::from_str::<super::MeterStyle>("\"meter\"").unwrap(),
            super::MeterStyle::Meter
        );
        assert_eq!(
            serde_json::from_str::<super::MeterStyle>("\"both\"").unwrap(),
            super::MeterStyle::Meter
        );
        assert_eq!(
            serde_json::from_str::<super::MeterStyle>("\"numbers\"").unwrap(),
            super::MeterStyle::Numbers
        );
        assert_eq!(bar.clock_format, "%H:%M:%S");
        let twelve: BarConfig =
            serde_json::from_str(r#"{"name":"main","clock_format":"%I:%M:%S %p"}"#).unwrap();
        assert_eq!(twelve.clock_format, "%I:%M:%S %p");
        assert_eq!(bar.padding_vertical, 0);
        assert_eq!(bar.padding_horizontal, 0);
        assert_eq!(bar.border, 0);
        assert_eq!(bar.corner_radius, 0);
        assert!(OverlayMode::Both.from_scroll());
        assert!(OverlayMode::Both.from_buttons());
        assert!(OverlayMode::Scrolling.from_scroll());
        assert!(!OverlayMode::Scrolling.from_buttons());
        assert!(OverlayMode::Buttons.from_buttons());
        assert!(!OverlayMode::Buttons.from_scroll());
        assert!(!OverlayMode::Never.from_scroll());
        assert!(!OverlayMode::Never.from_buttons());
    }

    #[test]
    fn layer_margins_follow_bound_edge() {
        let mut bar = super::BarConfig::default();
        bar.padding_vertical = 10;
        bar.padding_horizontal = 16;
        bar.position = super::Position::Top;
        assert_eq!(bar.layer_margins(), (10, 16, 0, 16));
        bar.position = super::Position::Bottom;
        assert_eq!(bar.layer_margins(), (0, 16, 10, 16));
    }

    #[test]
    fn vertical_border_uses_extra_surface_height() {
        let mut bar = super::BarConfig::default();
        bar.height = 18;
        bar.margin_vertical = 2;
        bar.margin_horizontal = 8;
        bar.padding_vertical = 8;
        bar.padding_horizontal = 12;
        bar.border = 0;
        let without = bar.text_insets(1920, bar.surface_height());
        bar.border = 1;
        assert_eq!(bar.surface_height(), 20);
        assert_eq!(bar.exclusive_zone(), 20);
        let with = bar.text_insets(1920, bar.surface_height());
        assert_eq!(with.inner_height, without.inner_height);
        assert_eq!(with.origin_y(), without.origin_y() + 1);
        assert_eq!(with.inner_width, without.inner_width.saturating_sub(2));
        assert_eq!(with.origin_x(), without.origin_x() + 1);
    }

    #[test]
    fn flush_top_only_skips_top_border_and_rounds_bottom() {
        let mut bar = super::BarConfig::default();
        bar.position = super::Position::Top;
        bar.height = 18;
        bar.border = 1;
        bar.corner_radius = 8;
        bar.padding_vertical = 0;
        bar.padding_horizontal = 12;
        let edges = bar.drawn_edges();
        assert!(!edges.top);
        assert!(edges.bottom && edges.left && edges.right);
        assert!(!edges.round_tl());
        assert!(!edges.round_tr());
        assert!(edges.round_bl());
        assert!(edges.round_br());
        assert_eq!(bar.surface_height(), 19);
        let insets = bar.text_insets(1920, bar.surface_height());
        assert_eq!(insets.top, 0);
        assert_eq!(insets.left, 1);
        assert_eq!(insets.origin_y(), insets.margin_y);
    }

    #[test]
    fn flush_sides_keep_inner_border() {
        let mut bar = super::BarConfig::default();
        bar.position = super::Position::Top;
        bar.height = 18;
        bar.border = 2;
        bar.corner_radius = 8;
        bar.padding_vertical = 0;
        bar.padding_horizontal = 0;
        let edges = bar.drawn_edges();
        assert!(!edges.top && !edges.left && !edges.right);
        assert!(edges.bottom);
        assert!(!edges.round_tl() && !edges.round_tr() && !edges.round_bl() && !edges.round_br());
        assert_eq!(bar.surface_height(), 20);
        let insets = bar.text_insets(1920, bar.surface_height());
        assert_eq!(insets.left, 0);
        assert_eq!(insets.top, 0);
        assert_eq!(insets.origin_x(), insets.margin_x);
        assert_eq!(insets.origin_y(), insets.margin_y);
        bar.position = super::Position::Bottom;
        let edges = bar.drawn_edges();
        assert!(edges.top && !edges.bottom && !edges.left && !edges.right);
    }
}
