mod cache;
mod color;
mod extract;
mod lab;
mod map;
mod recipe;
mod sample;
mod template;

use std::fs;
use std::path::Path;

use anyhow::{bail, Result};
use vile_protocol::{palettes_dir, sanitize_recipe_name};

pub use cache::{apply_sequences, export_palette, link_wal_compat, write_cache};
pub use color::Candidate;
pub use extract::{extract_all_methods, extract_report, ExtractReport};
pub use lab::run_lab;
pub use map::{make_palette, Palette};
pub use recipe::{
    AlphaMatte, Assignment, BgSource, ExtractorKind, MapParams, Recipe, Theme, RECIPE_SCHEMA,
};
pub use sample::sample_image;
pub use template::render;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorsBinCmd {
    Apply { recipe: String, image: String },
    Lab(LabOpts),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabOpts {
    pub intent: LabIntent,
    pub image: Option<String>,
    pub load: LoadSpec,
    pub recipe: Option<String>,
    pub out: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabIntent {
    Edit,
    PickApply,
    PickWall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSpec {
    Defaults,
    List,
    Name(String),
}

pub fn list_recipe_names() -> Result<Vec<String>> {
    let dir = palettes_dir();
    let mut names = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(err) => anyhow::bail!("reading {}: {err}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_json = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("json"));
        if !is_json {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let name = sanitize_recipe_name(stem);
        if !name.is_empty() {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn palette_from_image(recipe: &Recipe, image: &Path) -> Result<Palette> {
    let wallpaper = image.canonicalize().unwrap_or_else(|_| image.to_path_buf());
    let samples = sample::sample_image(image, &recipe.sample)?;
    if samples.is_empty() {
        bail!("no sampled pixels (check alpha cutoff / image)");
    }
    let ranked = extract::extract(&samples, recipe);
    map::make_palette(&ranked, &recipe.map, &wallpaper.to_string_lossy())
}

pub fn apply(recipe_name: &str, image: &Path) -> Result<Palette> {
    let name = sanitize_recipe_name(recipe_name);
    if name.is_empty() {
        bail!("invalid recipe name {recipe_name:?}");
    }
    if !image.is_file() {
        bail!("image not found: {}", image.display());
    }
    let path = palettes_dir().join(format!("{name}.json"));
    let recipe = Recipe::load(&path)?;
    let palette = palette_from_image(&recipe, image)?;
    export_palette(&palette)?;
    Ok(palette)
}

pub fn parse_helper_args(args: impl IntoIterator<Item = String>) -> Result<(String, String)> {
    match parse_cmd(args)? {
        ColorsBinCmd::Apply { recipe, image } => Ok((recipe, image)),
        ColorsBinCmd::Lab(_) => bail!("{}", helper_usage()),
    }
}

pub fn parse_cmd(args: impl IntoIterator<Item = String>) -> Result<ColorsBinCmd> {
    let args: Vec<String> = args.into_iter().collect();
    let mut recipe = None;
    let mut image = None;
    let mut lab = false;
    let mut pick = false;
    let mut pick_wall = false;
    let mut load: Option<Option<String>> = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            bail!("{}", helper_usage());
        }
        if arg == "--lab" {
            lab = true;
            i += 1;
            continue;
        }
        if arg == "--pick" {
            pick = true;
            i += 1;
            continue;
        }
        if arg == "--pick-wall" {
            pick_wall = true;
            i += 1;
            continue;
        }
        if arg == "--load" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    load = Some(Some(next.clone()));
                    i += 2;
                    continue;
                }
            }
            load = Some(None);
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--load=") {
            load = if rest.is_empty() {
                Some(None)
            } else {
                Some(Some(rest.to_string()))
            };
            i += 1;
            continue;
        }
        if arg == "--recipe" {
            recipe = Some(
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{}", helper_usage()))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--recipe=") {
            recipe = Some(rest.to_string());
            i += 1;
            continue;
        }
        if arg == "--out" {
            out = Some(
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{}", helper_usage()))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--out=") {
            out = Some(rest.to_string());
            i += 1;
            continue;
        }
        if arg == "--img" {
            image = Some(
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{}", helper_usage()))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--img=") {
            image = Some(rest.to_string());
            i += 1;
            continue;
        }
        bail!("{}", helper_usage());
    }
    if lab || pick || pick_wall {
        let intent = if pick_wall {
            LabIntent::PickWall
        } else if pick {
            LabIntent::PickApply
        } else {
            LabIntent::Edit
        };
        let load = match load {
            None => LoadSpec::Defaults,
            Some(None) => LoadSpec::List,
            Some(Some(name)) => LoadSpec::Name(name),
        };
        return Ok(ColorsBinCmd::Lab(LabOpts {
            intent,
            image,
            load,
            recipe,
            out,
        }));
    }
    match (recipe, image) {
        (Some(recipe), Some(image)) => Ok(ColorsBinCmd::Apply { recipe, image }),
        _ => bail!("{}", helper_usage()),
    }
}

pub fn helper_usage() -> &'static str {
    "usage: vilecolors --recipe <name> --img <path>\n       vilecolors --lab [--img <path>] [--load [name]]\n       vilecolors --pick [--img <path>] [--recipe <name>]\n       vilecolors --pick-wall"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::ExtractorKind;
    use image::{Rgba, RgbaImage};
    use std::fs;

    #[test]
    fn parses_helper_args() {
        let (r, i) = parse_helper_args([
            "--recipe".into(),
            "exported".into(),
            "--img".into(),
            "a.png".into(),
        ])
        .unwrap();
        assert_eq!(r, "exported");
        assert_eq!(i, "a.png");
        let (r, i) =
            parse_helper_args(["--img=/tmp/w.png".into(), "--recipe=cutty".into()]).unwrap();
        assert_eq!(r, "cutty");
        assert_eq!(i, "/tmp/w.png");
        assert!(parse_helper_args(["--recipe".into(), "x".into()]).is_err());
        match parse_cmd(["--lab".into(), "--img".into(), "a.png".into()]).unwrap() {
            ColorsBinCmd::Lab(opts) => {
                assert_eq!(opts.intent, LabIntent::Edit);
                assert_eq!(opts.image.as_deref(), Some("a.png"));
                assert_eq!(opts.load, LoadSpec::Defaults);
            }
            _ => panic!("expected lab"),
        }
        match parse_cmd(["--lab".into(), "--load".into()]).unwrap() {
            ColorsBinCmd::Lab(opts) => assert_eq!(opts.load, LoadSpec::List),
            _ => panic!("expected load list"),
        }
        match parse_cmd(["--lab".into(), "--load".into(), "cutty".into()]).unwrap() {
            ColorsBinCmd::Lab(opts) => assert_eq!(opts.load, LoadSpec::Name("cutty".into())),
            _ => panic!("expected load name"),
        }
        match parse_cmd(["--pick".into(), "--recipe".into(), "cutty".into()]).unwrap() {
            ColorsBinCmd::Lab(opts) => {
                assert_eq!(opts.intent, LabIntent::PickApply);
                assert_eq!(opts.recipe.as_deref(), Some("cutty"));
            }
            _ => panic!("expected pick"),
        }
        match parse_cmd(["--pick-wall".into()]).unwrap() {
            ColorsBinCmd::Lab(opts) => assert_eq!(opts.intent, LabIntent::PickWall),
            _ => panic!("expected pick-wall"),
        }
    }

    #[test]
    fn extracts_sixteen_hex_from_png() {
        let dir = std::env::temp_dir().join(format!("vilecolors-png-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.png");
        let mut img = RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if x < 16 {
                Rgba([220, 40, 40, 255])
            } else if y < 16 {
                Rgba([40, 80, 200, 255])
            } else {
                Rgba([40, 180, 90, 255])
            };
        }
        img.save(&path).unwrap();
        let mut recipe = Recipe::lab_defaults("t");
        recipe.sample.max_edge = 32;
        recipe.sample.sample_cap = 1024;
        for kind in [
            ExtractorKind::Cut,
            ExtractorKind::Hue,
            ExtractorKind::Kmeans,
        ] {
            recipe.extractor = kind;
            let pal = palette_from_image(&recipe, &path).unwrap();
            assert_eq!(pal.colors.len(), 16);
            for c in &pal.colors {
                assert_eq!(c.len(), 7, "{c}");
                assert!(c.starts_with('#'));
                assert!(c[1..].chars().all(|ch| ch.is_ascii_hexdigit()));
            }
            let report = extract_report(&sample_image(&path, &recipe.sample).unwrap(), &recipe);
            assert!(report.ranked.len() <= 12);
            assert!(report.all.len() >= report.ranked.len());
        }
        let _ = fs::remove_dir_all(dir);
    }
}
