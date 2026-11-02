use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use vile_protocol::sanitize_recipe_name;

pub const RECIPE_SCHEMA: &str = "vilecolor-recipe/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractorKind {
    Cut,
    Hue,
    Kmeans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMatte {
    Dark,
    Light,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Assignment {
    Ranked,
    HueOrder,
    Ansi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgSource {
    Dominant,
    Darkest,
    Vibrant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleParams {
    pub max_edge: u32,
    pub sample_cap: u32,
    pub alpha_threshold: u8,
    pub alpha_matte: AlphaMatte,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutParams {
    pub bits: u32,
    pub boxes: u32,
    pub sat_bias: f64,
    pub pop_exponent: f64,
    pub separation: f64,
    pub vivid: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueParams {
    pub h_bins: u32,
    pub s_bins: u32,
    pub v_bins: u32,
    pub vivid: f64,
    pub pop_exponent: f64,
    pub separation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmeansParams {
    pub k: u32,
    pub iterations: u32,
    pub vivid: f64,
    pub pop_exponent: f64,
    pub separation: f64,
    pub seed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapParams {
    pub assignment: Assignment,
    pub theme: Theme,
    pub bg_source: BgSource,
    pub hue_snap: f64,
    pub bg_l: f64,
    pub fg_l: f64,
    pub normal_l: f64,
    pub bright_l: f64,
    pub preserve_l: f64,
    pub chroma: f64,
    pub min_c: f64,
    pub bright_chroma: f64,
    pub neutral_c: f64,
    pub muted_l: f64,
    pub bg_chroma: f64,
    pub bright_floor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub schema: String,
    pub name: String,
    pub extractor: ExtractorKind,
    pub sample: SampleParams,
    pub cut: CutParams,
    pub hue: HueParams,
    pub kmeans: KmeansParams,
    pub map: MapParams,
}

impl Recipe {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading recipe {}", path.display()))?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let recipe: Self = serde_json::from_str(text).context("parsing vilecolor-recipe/v1")?;
        if recipe.schema != RECIPE_SCHEMA {
            bail!("need schema {RECIPE_SCHEMA}, got {}", recipe.schema);
        }
        Ok(recipe)
    }

    pub fn save(&self, dir: &Path) -> Result<PathBuf> {
        let name = sanitize_recipe_name(&self.name);
        if name.is_empty() {
            bail!("invalid recipe name");
        }
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(self).context("serializing recipe")?;
        fs::write(&path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    pub fn lab_defaults(name: &str) -> Self {
        Self {
            schema: RECIPE_SCHEMA.into(),
            name: sanitize_recipe_name(name),
            extractor: ExtractorKind::Cut,
            sample: SampleParams {
                max_edge: 640,
                sample_cap: 12000,
                alpha_threshold: 24,
                alpha_matte: AlphaMatte::Dark,
            },
            cut: CutParams {
                bits: 5,
                boxes: 24,
                sat_bias: 1.0,
                pop_exponent: 0.75,
                separation: 9.0,
                vivid: 0.8,
            },
            hue: HueParams {
                h_bins: 24,
                s_bins: 4,
                v_bins: 4,
                vivid: 1.5,
                pop_exponent: 0.65,
                separation: 10.0,
            },
            kmeans: KmeansParams {
                k: 14,
                iterations: 12,
                vivid: 1.25,
                pop_exponent: 0.7,
                separation: 8.0,
                seed: 17,
            },
            map: MapParams {
                assignment: Assignment::Ranked,
                theme: Theme::Dark,
                bg_source: BgSource::Darkest,
                hue_snap: 70.0,
                bg_l: 0.14,
                fg_l: 0.91,
                normal_l: 0.65,
                bright_l: 0.79,
                preserve_l: 20.0,
                chroma: 1.1,
                min_c: 0.035,
                bright_chroma: 1.05,
                neutral_c: 0.035,
                muted_l: 0.44,
                bg_chroma: 0.055,
                bright_floor: 0.08,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_example() {
        let json = r#"{
          "schema": "vilecolor-recipe/v1",
          "name": "exported",
          "extractor": "cut",
          "sample": { "max_edge": 640, "sample_cap": 12000, "alpha_threshold": 24, "alpha_matte": "dark" },
          "cut": { "bits": 5, "boxes": 24, "sat_bias": 1.0, "pop_exponent": 0.75, "separation": 9, "vivid": 0.8 },
          "hue": { "h_bins": 24, "s_bins": 4, "v_bins": 4, "vivid": 1.5, "pop_exponent": 0.65, "separation": 10 },
          "kmeans": { "k": 14, "iterations": 12, "vivid": 1.25, "pop_exponent": 0.7, "separation": 8, "seed": 17 },
          "map": {
            "assignment": "ranked", "theme": "dark", "bg_source": "darkest", "hue_snap": 70,
            "bg_l": 0.14, "fg_l": 0.91, "normal_l": 0.65, "bright_l": 0.79, "preserve_l": 20,
            "chroma": 1.1, "min_c": 0.035, "bright_chroma": 1.05, "neutral_c": 0.035,
            "muted_l": 0.44, "bg_chroma": 0.055, "bright_floor": 0.08
          }
        }"#;
        let recipe = Recipe::from_json(json).unwrap();
        assert_eq!(recipe.extractor, ExtractorKind::Cut);
        assert_eq!(recipe.sample.alpha_matte, AlphaMatte::Dark);
        assert_eq!(recipe.map.assignment, Assignment::Ranked);
        assert_eq!(recipe.cut.bits, 5);
    }

    #[test]
    fn rejects_wrong_schema() {
        let json = r#"{"schema":"nope","name":"x","extractor":"cut","sample":{"max_edge":1,"sample_cap":1,"alpha_threshold":0,"alpha_matte":"raw"},"cut":{"bits":5,"boxes":8,"sat_bias":1,"pop_exponent":1,"separation":1,"vivid":1},"hue":{"h_bins":8,"s_bins":2,"v_bins":2,"vivid":1,"pop_exponent":1,"separation":1},"kmeans":{"k":6,"iterations":2,"vivid":1,"pop_exponent":1,"separation":1,"seed":1},"map":{"assignment":"ansi","theme":"light","bg_source":"vibrant","hue_snap":0,"bg_l":0.5,"fg_l":0.5,"normal_l":0.5,"bright_l":0.5,"preserve_l":0,"chroma":1,"min_c":0,"bright_chroma":1,"neutral_c":0,"muted_l":0.5,"bg_chroma":0,"bright_floor":0}}"#;
        assert!(Recipe::from_json(json).is_err());
    }

    #[test]
    fn save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("vilecolor-recipe-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut recipe = Recipe::lab_defaults("round-trip");
        recipe.extractor = ExtractorKind::Hue;
        recipe.map.chroma = 1.25;
        let path = recipe.save(&dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "round-trip.json");
        let back = Recipe::load(&path).unwrap();
        assert_eq!(back.extractor, ExtractorKind::Hue);
        assert_eq!(back.map.chroma, 1.25);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_hue_order_assignment() {
        let mut recipe = Recipe::lab_defaults("t");
        recipe.map.assignment = Assignment::HueOrder;
        let json = serde_json::to_string(&recipe).unwrap();
        assert!(json.contains("\"hue-order\""));
        let back = Recipe::from_json(&json).unwrap();
        assert_eq!(back.map.assignment, Assignment::HueOrder);
    }
}
