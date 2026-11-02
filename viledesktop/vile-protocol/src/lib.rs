pub mod cells;
pub mod config;
pub mod ipc;
pub mod messages;
pub mod paths;

pub use cells::{
    dirty_cell_ranges, display_width, fit_cells, flatten_cells, merge_ranges, pack_layout,
    pack_line, ranges_overlap, PackedLine,
};
pub use config::{
    parse_hex_color, resolve_color, AnimationConfig, BarConfig, BarEdges, Config, HwMetric,
    MenuConfig, MenuItem, MeterStyle, Modules, NotificationConfig, NotificationSide, OverlayMode,
    Position, ScreenshotConfig, TextInsets, TrayConfig, WallpaperConfig, WallpaperMode,
    WallpaperState,
};
pub use ipc::{read_json_line, read_json_lines, write_json_line};
pub use messages::{BarToDaemon, CellStyle, DaemonToBar, OverlayKind, PointerEvent};
pub use paths::{
    color_cache_dir, colors_dir, config_path, ensure_config_file, ensure_runtime_dir,
    expand_home_path, load_config, load_wallpaper_state, palettes_dir, sanitize_recipe_name,
    save_wallpaper_state, socket_path, templates_dir, vilefox_config_path, wal_compat_dir,
    wal_templates_dir, wallpaper_directory, wallpaper_state_path, write_wal_path,
};
