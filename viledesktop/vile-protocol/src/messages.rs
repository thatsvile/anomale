use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BarToDaemon {
    Hello {
        bar: String,
        output: String,
        width_px: u32,
        height_px: u32,
        scale: u32,
        cols: u32,
    },
    Configure {
        width_px: u32,
        height_px: u32,
        scale: u32,
        cols: u32,
    },
    Pointer {
        col: u32,
        event: PointerEvent,
    },
    Volume {
        delta: i32,
    },
    Mute,
    Colors {
        recipe: String,
        image: String,
        #[serde(default)]
        apply_wall: bool,
        #[serde(default)]
        wall_mode: Option<crate::config::WallpaperMode>,
    },
    Wallpaper {
        image: String,
        #[serde(default)]
        mode: Option<crate::config::WallpaperMode>,
    },
    WallHello,
    AppsToggle,
    NotificationsToggle,
    TrayToggle,
    MenuToggle,
    Overlay {
        kind: OverlayKind,
    },
    Key {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    /// Bar lost keyboard focus while it had an exclusive grab (click outside).
    KeyboardLost,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    Battery,
    Mem,
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerEvent {
    Enter,
    Motion,
    Leave,
    Button { code: u32, pressed: bool },
    Axis { steps: i32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellStyle {
    pub col: u32,
    pub fg: u32,
    pub bg: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonToBar {
    Frame {
        text: String,
        fg: u32,
        bg: u32,
        #[serde(default)]
        cells: Vec<CellStyle>,
    },
    Ok,
    Error {
        message: String,
    },
    SetWallpaper {
        image: String,
        mode: crate::config::WallpaperMode,
    },
    Keyboard {
        exclusive: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::BarToDaemon;

    #[test]
    fn tray_toggle_roundtrip() {
        let json = serde_json::to_string(&BarToDaemon::TrayToggle).unwrap();
        assert!(json.contains("tray_toggle"));
        let parsed: BarToDaemon = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, BarToDaemon::TrayToggle));
    }

    #[test]
    fn menu_toggle_roundtrip() {
        let json = serde_json::to_string(&BarToDaemon::MenuToggle).unwrap();
        assert!(json.contains("menu_toggle"));
        let parsed: BarToDaemon = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, BarToDaemon::MenuToggle));
    }

    #[test]
    fn keyboard_lost_roundtrip() {
        let json = serde_json::to_string(&BarToDaemon::KeyboardLost).unwrap();
        assert!(json.contains("keyboard_lost"));
        let parsed: BarToDaemon = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, BarToDaemon::KeyboardLost));
    }

    #[test]
    fn overlay_roundtrip() {
        use super::OverlayKind;
        let json = serde_json::to_string(&BarToDaemon::Overlay {
            kind: OverlayKind::Battery,
        })
        .unwrap();
        assert!(json.contains("overlay"));
        assert!(json.contains("battery"));
        let parsed: BarToDaemon = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            BarToDaemon::Overlay {
                kind: OverlayKind::Battery
            }
        ));
    }
}
