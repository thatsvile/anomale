use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use vile_protocol::{load_config, palettes_dir, sanitize_recipe_name, wallpaper_directory};

use crate::extract::{extract_all_methods, ExtractReport};
use crate::map::{make_palette, Palette};
use crate::recipe::{ExtractorKind, Recipe};
use crate::sample::sample_image;
use crate::{Candidate, LabIntent, LabOpts, LoadSpec};

use super::knobs::{self, Dirty, KnobPane};
use super::recipes::{self, RecipeRow};
use super::walls::{self, ThumbCache};

pub const DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Walls,
    Recipes,
    Editor,
}

pub enum Overlay {
    None,
    SaveName { buf: String, then_apply: bool },
    ConfirmDelete { name: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PickResult {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recipe: String,
    pub image: String,
}

pub struct App {
    pub intent: LabIntent,
    pub load: LoadSpec,
    pub named_recipe: Option<String>,
    pub screen: Screen,
    pub overlay: Overlay,
    pub image: Option<PathBuf>,
    pub recipe: Recipe,
    pub dirty_recipe: bool,
    pub samples: Vec<Candidate>,
    pub reports: [ExtractReport; 3],
    pub palettes: [Option<Palette>; 3],
    pub pane: KnobPane,
    pub knob_index: usize,
    pub walls: Vec<PathBuf>,
    pub wall_dir: Option<PathBuf>,
    pub wall_sel: usize,
    pub wall_cols: usize,
    pub wall_rows: usize,
    pub thumbs: ThumbCache,
    pub recipe_rows: Vec<RecipeRow>,
    pub recipe_sel: usize,
    pub picker: Picker,
    pub preview: Option<StatefulProtocol>,
    pub debounce: Option<(Instant, Dirty)>,
    pub status: String,
    pub quit: bool,
    pub pick: Option<PickResult>,
}

impl App {
    pub fn new(opts: LabOpts, picker: Picker) -> Result<Self> {
        let named = match &opts.load {
            LoadSpec::Name(name) => Some(sanitize_recipe_name(name)),
            _ => opts.recipe.as_deref().map(sanitize_recipe_name),
        };
        let named = named.filter(|s| !s.is_empty());
        let loaded_named = named.is_some();
        let recipe = if let Some(name) = &named {
            recipes::load_named(name).unwrap_or_else(|_| Recipe::lab_defaults(name))
        } else {
            Recipe::lab_defaults("untitled")
        };
        let image = opts.image.map(PathBuf::from);
        let mut app = Self {
            intent: opts.intent,
            load: opts.load,
            named_recipe: named,
            screen: Screen::Editor,
            overlay: Overlay::None,
            image,
            recipe,
            dirty_recipe: !loaded_named,
            samples: Vec::new(),
            reports: [
                ExtractReport::default(),
                ExtractReport::default(),
                ExtractReport::default(),
            ],
            palettes: [None, None, None],
            pane: KnobPane::Sample,
            knob_index: 0,
            walls: Vec::new(),
            wall_dir: None,
            wall_sel: 0,
            wall_cols: 4,
            wall_rows: 3,
            thumbs: ThumbCache::new(),
            recipe_rows: Vec::new(),
            recipe_sel: 0,
            picker,
            preview: None,
            debounce: None,
            status: String::new(),
            quit: false,
            pick: None,
        };
        if app.image.is_none() {
            app.open_walls()?;
        } else {
            app.reload_preview();
            app.after_image()?;
        }
        Ok(app)
    }

    fn wants_recipe_list(&self) -> bool {
        matches!(self.load, LoadSpec::List)
            || (self.intent == LabIntent::PickApply && self.named_recipe.is_none())
    }

    fn after_image(&mut self) -> Result<()> {
        match self.intent {
            LabIntent::PickWall => {
                self.finish_pick(None);
                Ok(())
            }
            LabIntent::PickApply if self.named_recipe.is_some() => {
                self.finish_pick(self.named_recipe.clone());
                Ok(())
            }
            _ if self.wants_recipe_list() => self.open_recipes(),
            _ => {
                self.open_editor();
                Ok(())
            }
        }
    }

    pub fn open_walls(&mut self) -> Result<()> {
        let cfg = load_config()?;
        let dir = wallpaper_directory(&cfg.wallpaper.directory).ok_or_else(|| {
            anyhow::anyhow!("set wallpaper.directory in ~/.config/vile/config.json or pass --img")
        })?;
        if !dir.is_dir() {
            bail!("wallpaper directory is not a folder: {}", dir.display());
        }
        self.walls = walls::scan_images(&dir);
        if self.walls.is_empty() {
            bail!("no images in {}", dir.display());
        }
        if let Some(current) = &self.image {
            if let Some(i) = self.walls.iter().position(|p| p == current) {
                self.wall_sel = i;
            }
        }
        self.wall_sel = self.wall_sel.min(self.walls.len().saturating_sub(1));
        self.wall_dir = Some(dir);
        self.screen = Screen::Walls;
        self.refresh_wall_status();
        Ok(())
    }

    pub fn refresh_wall_status(&mut self) {
        let pages = walls::page_count(self.walls.len(), self.wall_cols, self.wall_rows);
        let page = self.wall_sel / walls::page_size(self.wall_cols, self.wall_rows) + 1;
        let dir = self
            .wall_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.status = format!("page {page}/{pages} · {} images · {dir}", self.walls.len());
    }

    pub fn wall_keep_paths(&self) -> Vec<PathBuf> {
        let size = walls::page_size(self.wall_cols, self.wall_rows);
        let start = walls::page_start(self.wall_sel, self.wall_cols, self.wall_rows);
        let prev = start.saturating_sub(size);
        let end = (start + size).min(self.walls.len());
        self.walls.get(prev..end).unwrap_or(&[]).to_vec()
    }

    pub fn pump_thumbs(&mut self) {
        if self.screen != Screen::Walls {
            return;
        }
        let keep = self.wall_keep_paths();
        self.thumbs.pump(&mut self.picker, &keep, 1);
    }

    pub fn open_recipes(&mut self) -> Result<()> {
        let image = self
            .image
            .clone()
            .ok_or_else(|| anyhow::anyhow!("pick an image first"))?;
        self.status = "extracting recipe previews…".into();
        self.recipe_rows = recipes::load_rows(&image);
        if self.recipe_rows.is_empty() {
            self.status = format!("no recipes in {}", palettes_dir().display());
        } else {
            self.status = format!("{} recipes", self.recipe_rows.len());
        }
        if let Some(name) = &self.named_recipe {
            if let Some(i) = self.recipe_rows.iter().position(|r| &r.name == name) {
                self.recipe_sel = i;
            }
        }
        self.recipe_sel = self
            .recipe_sel
            .min(self.recipe_rows.len().saturating_sub(1));
        self.screen = Screen::Recipes;
        Ok(())
    }

    pub fn open_editor(&mut self) {
        self.screen = Screen::Editor;
        self.resample_all();
        self.status = format!(
            "{} · {}",
            self.recipe.name,
            self.image
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("no image")
        );
    }

    fn reload_preview(&mut self) {
        self.preview = None;
        let Some(path) = &self.image else {
            return;
        };
        match image::open(path) {
            Ok(img) => {
                let img = img.resize(960, 540, image::imageops::FilterType::Triangle);
                self.preview = Some(self.picker.new_resize_protocol(img));
            }
            Err(err) => self.status = format!("image: {err}"),
        }
    }

    fn resample_all(&mut self) {
        let Some(path) = &self.image else {
            self.samples.clear();
            return;
        };
        match sample_image(path, &self.recipe.sample) {
            Ok(samples) => {
                self.samples = samples;
                self.reextract_all();
            }
            Err(err) => self.status = format!("sample: {err}"),
        }
    }

    fn reextract_all(&mut self) {
        self.reports = extract_all_methods(&self.samples, &self.recipe);
        self.remap_all();
    }

    fn remap_all(&mut self) {
        let wall = self
            .image
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        for i in 0..3 {
            self.palettes[i] = make_palette(&self.reports[i].ranked, &self.recipe.map, &wall).ok();
        }
    }

    pub fn active_idx(&self) -> usize {
        match self.recipe.extractor {
            ExtractorKind::Cut => 0,
            ExtractorKind::Hue => 1,
            ExtractorKind::Kmeans => 2,
        }
    }

    pub fn flush_debounce(&mut self) {
        let Some((at, kind)) = self.debounce else {
            return;
        };
        if at.elapsed() < DEBOUNCE {
            return;
        }
        self.run_dirty(kind);
    }

    fn flush_now(&mut self) {
        if let Some((_, kind)) = self.debounce.take() {
            self.run_dirty(kind);
        }
    }

    fn run_dirty(&mut self, kind: Dirty) {
        self.debounce = None;
        match kind {
            Dirty::Sample => self.resample_all(),
            Dirty::Extract => self.reextract_all(),
            Dirty::Map => self.remap_all(),
        }
    }

    pub fn poll_timeout(&self) -> Duration {
        if let Some((at, _)) = self.debounce {
            return DEBOUNCE.saturating_sub(at.elapsed());
        }
        if self.screen == Screen::Walls && self.thumbs.needs_pump(&self.wall_keep_paths()) {
            return Duration::from_millis(30);
        }
        Duration::from_millis(250)
    }

    fn touch(&mut self, kind: Dirty) {
        self.dirty_recipe = true;
        let merged = match (self.debounce.map(|(_, k)| k), kind) {
            (Some(Dirty::Sample), _) | (_, Dirty::Sample) => Dirty::Sample,
            (Some(Dirty::Extract), _) | (_, Dirty::Extract) => Dirty::Extract,
            _ => Dirty::Map,
        };
        self.debounce = Some((Instant::now(), merged));
    }

    fn finish_pick(&mut self, recipe: Option<String>) {
        let Some(image) = self.image.clone() else {
            self.status = "no image selected".into();
            return;
        };
        self.pick = Some(PickResult {
            recipe: recipe.unwrap_or_default(),
            image: image.to_string_lossy().into_owned(),
        });
        self.quit = true;
    }

    fn save_recipe(&mut self, name: &str) -> Result<String> {
        self.flush_now();
        let name = sanitize_recipe_name(name);
        if name.is_empty() {
            bail!("invalid recipe name");
        }
        self.recipe.name = name.clone();
        let path = self
            .recipe
            .save(&palettes_dir())
            .with_context(|| "saving recipe")?;
        self.dirty_recipe = false;
        self.status = format!("saved {}", path.display());
        Ok(name)
    }

    fn request_apply(&mut self) {
        self.apply_now();
    }

    fn apply_now(&mut self) {
        self.flush_now();
        let name = self.recipe.name.clone();
        let Some(image) = self.image.clone() else {
            self.status = "no image selected".into();
            return;
        };
        match apply_live(&self.recipe, &image) {
            Ok(()) => self.status = format!("applied {name}"),
            Err(err) => self.status = format!("apply: {err}"),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return;
        }
        match &self.overlay {
            Overlay::SaveName { .. } => {
                self.on_save_key(key);
                return;
            }
            Overlay::ConfirmDelete { .. } => {
                self.on_confirm_delete_key(key);
                return;
            }
            Overlay::None => {}
        }
        match self.screen {
            Screen::Walls => self.on_walls_key(key),
            Screen::Recipes => self.on_recipes_key(key),
            Screen::Editor => self.on_editor_key(key),
        }
    }

    fn on_save_key(&mut self, key: KeyEvent) {
        let Overlay::SaveName { buf, then_apply } = &mut self.overlay else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Enter => {
                let name = buf.clone();
                let then_apply = *then_apply;
                match self.save_recipe(&name) {
                    Ok(_) => {
                        self.overlay = Overlay::None;
                        if then_apply {
                            self.apply_now();
                        }
                    }
                    Err(err) => self.status = err.to_string(),
                }
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && buf.len() < 64 =>
            {
                buf.push(ch);
            }
            _ => {}
        }
    }

    fn on_confirm_delete_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let Overlay::ConfirmDelete { name } = &self.overlay else {
                    return;
                };
                let name = name.clone();
                self.overlay = Overlay::None;
                self.finish_delete(&name);
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    fn request_delete(&mut self) {
        let Some(row) = self.recipe_rows.get(self.recipe_sel) else {
            return;
        };
        self.overlay = Overlay::ConfirmDelete {
            name: row.name.clone(),
        };
    }

    fn finish_delete(&mut self, name: &str) {
        match recipes::delete_named(name) {
            Ok(()) => {
                let name = sanitize_recipe_name(name);
                self.recipe_rows.retain(|row| row.name != name);
                if self.named_recipe.as_deref() == Some(name.as_str()) {
                    self.named_recipe = None;
                }
                if self.recipe_rows.is_empty() {
                    self.recipe_sel = 0;
                    self.status = format!("deleted {name} · no recipes left");
                } else {
                    self.recipe_sel = self.recipe_sel.min(self.recipe_rows.len() - 1);
                    self.status = format!("deleted {name}");
                }
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn open_colorlab_from_picker(&mut self) {
        self.intent = LabIntent::Edit;
        if self.recipe_rows.is_empty() {
            self.recipe = Recipe::lab_defaults("untitled");
            self.named_recipe = None;
            self.dirty_recipe = true;
            self.open_editor();
            return;
        }
        let Some(row) = self.recipe_rows.get(self.recipe_sel) else {
            self.recipe = Recipe::lab_defaults("untitled");
            self.named_recipe = None;
            self.dirty_recipe = true;
            self.open_editor();
            return;
        };
        let name = row.name.clone();
        match recipes::load_named(&name) {
            Ok(recipe) => {
                self.recipe = recipe;
                self.named_recipe = Some(name);
                self.dirty_recipe = false;
                self.open_editor();
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn on_walls_key(&mut self, key: KeyEvent) {
        if self.quit_key(key) {
            return;
        }
        let cols = self.wall_cols.max(1);
        let n = self.walls.len();
        if n == 0 {
            return;
        }
        match key.code {
            KeyCode::Left if !shift(&key) => {
                self.wall_sel = self.wall_sel.saturating_sub(1);
                self.refresh_wall_status();
            }
            KeyCode::Right if !shift(&key) => {
                self.wall_sel = (self.wall_sel + 1).min(n - 1);
                self.refresh_wall_status();
            }
            KeyCode::Up if !shift(&key) => {
                self.wall_sel = self.wall_sel.saturating_sub(cols);
                self.refresh_wall_status();
            }
            KeyCode::Down if !shift(&key) => {
                self.wall_sel = (self.wall_sel + cols).min(n - 1);
                self.refresh_wall_status();
            }
            KeyCode::PageUp => {
                let size = walls::page_size(cols, self.wall_rows);
                self.wall_sel = self.wall_sel.saturating_sub(size);
                self.refresh_wall_status();
            }
            KeyCode::PageDown => {
                let size = walls::page_size(cols, self.wall_rows);
                self.wall_sel = (self.wall_sel + size).min(n - 1);
                self.refresh_wall_status();
            }
            KeyCode::Enter => {
                self.image = Some(self.walls[self.wall_sel].clone());
                self.reload_preview();
                if let Err(err) = self.after_image() {
                    self.status = err.to_string();
                }
            }
            _ => {}
        }
    }

    fn on_recipes_key(&mut self, key: KeyEvent) {
        if self.quit_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('o') => {
                if let Err(err) = self.open_walls() {
                    self.status = err.to_string();
                }
            }
            KeyCode::Up if !shift(&key) => {
                self.recipe_sel = self.recipe_sel.saturating_sub(1);
            }
            KeyCode::Down if !shift(&key) => {
                if !self.recipe_rows.is_empty() {
                    self.recipe_sel = (self.recipe_sel + 1).min(self.recipe_rows.len() - 1);
                }
            }
            KeyCode::Enter => {
                let Some(row) = self.recipe_rows.get(self.recipe_sel) else {
                    return;
                };
                let name = row.name.clone();
                match recipes::load_named(&name) {
                    Ok(recipe) => {
                        self.recipe = recipe;
                        self.named_recipe = Some(name.clone());
                        self.dirty_recipe = false;
                        if self.intent == LabIntent::PickApply {
                            self.finish_pick(Some(name));
                        } else {
                            self.open_editor();
                        }
                    }
                    Err(err) => self.status = err.to_string(),
                }
            }
            KeyCode::Char('d') => self.request_delete(),
            KeyCode::Char('e') if self.intent == LabIntent::PickApply => {
                self.open_colorlab_from_picker();
            }
            _ => {}
        }
    }

    fn on_editor_key(&mut self, key: KeyEvent) {
        if self.quit_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('o') => {
                if let Err(err) = self.open_walls() {
                    self.status = err.to_string();
                }
            }
            KeyCode::Char('l') => {
                if let Err(err) = self.open_recipes() {
                    self.status = err.to_string();
                }
            }
            KeyCode::Char('s') => {
                self.overlay = Overlay::SaveName {
                    buf: self.recipe.name.clone(),
                    then_apply: false,
                };
            }
            KeyCode::Char('a') => self.request_apply(),
            KeyCode::Char('r') => {
                let name = self.recipe.name.clone();
                self.recipe = Recipe::lab_defaults(&name);
                self.knob_index = 0;
                self.touch(Dirty::Sample);
            }
            KeyCode::Char('1') => self.set_extractor(ExtractorKind::Cut),
            KeyCode::Char('2') => self.set_extractor(ExtractorKind::Hue),
            KeyCode::Char('3') => self.set_extractor(ExtractorKind::Kmeans),
            KeyCode::Up if shift(&key) => self.step_focused(1, true),
            KeyCode::Down if shift(&key) => self.step_focused(-1, true),
            KeyCode::Left if shift(&key) => self.step_focused(-1, false),
            KeyCode::Right if shift(&key) => self.step_focused(1, false),
            KeyCode::Up => self.move_knob(0, -1),
            KeyCode::Down => self.move_knob(0, 1),
            KeyCode::Left => self.move_knob(-1, 0),
            KeyCode::Right => self.move_knob(1, 0),
            _ => {}
        }
    }

    fn quit_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.quit = true;
                true
            }
            _ => false,
        }
    }

    fn set_extractor(&mut self, kind: ExtractorKind) {
        self.recipe.extractor = kind;
        let n = knobs::knobs_for(self.pane, kind).len();
        if n > 0 {
            self.knob_index = self.knob_index.min(n - 1);
        } else {
            self.knob_index = 0;
        }
        self.dirty_recipe = true;
    }

    pub fn knobs(&self) -> &'static [knobs::Knob] {
        knobs::knobs_for(self.pane, self.recipe.extractor)
    }

    fn move_knob(&mut self, dx: i32, dy: i32) {
        let n = self.knobs().len();
        if n == 0 {
            return;
        }
        let mut idx = self.knob_index as i32;
        if dy != 0 {
            let next = idx + dy * 2;
            if next >= 0 && next < n as i32 {
                idx = next;
            } else if dy > 0 && idx + 1 < n as i32 && idx % 2 == 0 {
                idx += 1;
            }
        } else if dx < 0 {
            if idx % 2 == 1 {
                idx -= 1;
            } else {
                self.pane = self.pane.prev();
                let n = self.knobs().len();
                idx = n.saturating_sub(1) as i32;
            }
        } else if dx > 0 {
            if idx % 2 == 0 && idx + 1 < n as i32 {
                idx += 1;
            } else {
                self.pane = self.pane.next();
                idx = 0;
            }
        }
        let n = self.knobs().len();
        self.knob_index = (idx as usize).min(n.saturating_sub(1));
    }

    fn step_focused(&mut self, dir: i32, coarse: bool) {
        let Some(knob) = self.knobs().get(self.knob_index).copied() else {
            return;
        };
        let kind = knobs::step(&mut self.recipe, knob, dir, coarse);
        self.touch(kind);
    }
}

fn shift(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::SHIFT)
}

fn sibling_bin(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
            exe.parent().map(|dir| dir.join(name))
        })
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn apply_live(recipe: &Recipe, image: &Path) -> Result<()> {
    export_live(recipe, image)?;
    let bin = sibling_bin("vile");
    let _ = Command::new(&bin)
        .args(["--wall", "--img"])
        .arg(image)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn export_live(recipe: &Recipe, image: &Path) -> Result<()> {
    let palette = crate::palette_from_image(recipe, image)?;
    crate::export_palette(&palette)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Theme;
    use image::{Rgba, RgbaImage};
    use std::sync::Mutex;
    use vile_protocol::color_cache_dir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct RestoreEnv {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn restore(key: &'static str) -> RestoreEnv {
        RestoreEnv {
            key,
            old: std::env::var_os(key),
        }
    }

    #[test]
    fn apply_live_uses_knobs_not_disk_recipe_with_same_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "vile-lab-apply-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("config");
        let cache = root.join("cache");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let _config = restore("XDG_CONFIG_HOME");
        let _cache = restore("XDG_CACHE_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_CACHE_HOME", &cache);

        let mut on_disk = Recipe::lab_defaults("untitled");
        on_disk.map.theme = Theme::Light;
        on_disk.map.bg_l = 0.95;
        on_disk.save(&palettes_dir()).unwrap();

        let png = root.join("tiny.png");
        let mut img = RgbaImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = Rgba([32, 24, 20, 255]);
        }
        img.save(&png).unwrap();

        let live = Recipe::lab_defaults("untitled");
        assert_eq!(live.map.theme, Theme::Dark);
        export_live(&live, &png).unwrap();

        let json = std::fs::read_to_string(color_cache_dir().join("colors.json")).unwrap();
        assert!(json.contains("\"theme\": \"dark\""), "{json}");
        assert!(!json.contains("\"theme\": \"light\""));

        let _ = std::fs::remove_dir_all(root);
    }
}
