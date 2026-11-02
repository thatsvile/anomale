use anyhow::{bail, Result};

use crate::color::{candidate, clamp, hue_distance, lch_hex, lerp, mix_hue, Candidate};
use crate::recipe::{Assignment, BgSource, MapParams, Theme};

#[derive(Debug, Clone)]
pub struct Palette {
    pub colors: [String; 16],
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub wallpaper: String,
    pub alpha: String,
    pub theme: Theme,
}

const ANSI_HUES: [f64; 6] = [29.0, 145.0, 95.0, 260.0, 326.0, 195.0];

pub fn make_palette(
    extracted: &[Candidate],
    config: &MapParams,
    wallpaper: &str,
) -> Result<Palette> {
    if extracted.is_empty() {
        bail!("extractor produced no colors");
    }
    let mut colors = vec![String::new(); 16];
    let darkest = extracted
        .iter()
        .min_by(|a, b| {
            a.lch[0]
                .partial_cmp(&b.lch[0])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();
    let vivid = extracted
        .iter()
        .max_by(|a, b| {
            a.lch[1]
                .partial_cmp(&b.lch[1])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();
    let base = match config.bg_source {
        BgSource::Darkest => darkest,
        BgSource::Vibrant => vivid,
        BgSource::Dominant => &extracted[0],
    };
    let bg_chroma = base.lch[1].min(config.bg_chroma);
    let neutral = extracted
        .iter()
        .min_by(|a, b| {
            a.lch[1]
                .partial_cmp(&b.lch[1])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();
    colors[0] = lch_hex(config.bg_l, bg_chroma, base.lch[2]);
    colors[7] = lch_hex(
        config.fg_l,
        neutral.lch[1].min(config.neutral_c),
        neutral.lch[2],
    );
    colors[8] = lch_hex(
        config.muted_l,
        base.lch[1].min(config.neutral_c),
        base.lch[2],
    );
    let fg15 = if config.theme == Theme::Dark {
        (config.fg_l + 0.035).min(0.99)
    } else {
        (config.fg_l - 0.035).max(0.01)
    };
    colors[15] = lch_hex(fg15, neutral.lch[1].min(config.neutral_c), neutral.lch[2]);
    let accent = choose_accents(extracted, config);
    let t = config.preserve_l / 100.0;
    for (i, source) in accent.iter().enumerate() {
        let h = if config.assignment == Assignment::Ansi {
            mix_hue(source.lch[2], ANSI_HUES[i], config.hue_snap / 100.0)
        } else {
            source.lch[2]
        };
        let c = source.lch[1].max(config.min_c) * config.chroma;
        let mut n = lerp(config.normal_l, source.lch[0], t);
        let mut b = lerp(config.bright_l, source.lch[0], t);
        if config.theme == Theme::Dark {
            b = b.max(n + config.bright_floor);
        } else {
            b = b.min(n - config.bright_floor);
        }
        n = clamp(n, 0.0, 1.0);
        b = clamp(b, 0.0, 1.0);
        colors[i + 1] = lch_hex(n, c, h);
        colors[i + 9] = lch_hex(b, c * config.bright_chroma, h);
    }
    let background = colors[0].clone();
    let foreground = colors[7].clone();
    Ok(Palette {
        colors: colors.try_into().expect("16 colors"),
        background: background.clone(),
        foreground: foreground.clone(),
        cursor: foreground,
        wallpaper: wallpaper.to_string(),
        alpha: "100".into(),
        theme: config.theme,
    })
}

fn choose_accents(extracted: &[Candidate], config: &MapParams) -> Vec<Candidate> {
    if extracted.is_empty() {
        return (0..6)
            .map(|_| candidate([130.0, 130.0, 130.0], 1, 1, 1.0))
            .collect();
    }
    let mut items: Vec<Candidate> = extracted.to_vec();
    if config.assignment == Assignment::HueOrder {
        items.sort_by(|a, b| {
            a.lch[2]
                .partial_cmp(&b.lch[2])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    if config.assignment == Assignment::Ansi {
        return ANSI_HUES
            .iter()
            .map(|h| {
                items
                    .iter()
                    .min_by(|a, b| {
                        let err = |c: &Candidate| {
                            hue_distance(c.lch[2], *h) + (0.055 - c.lch[1]).max(0.0) * 300.0
                        };
                        err(a)
                            .partial_cmp(&err(b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned()
                    .unwrap()
            })
            .collect();
    }
    (0..6)
        .map(|i| {
            let idx = ((i * items.len()) / 6).min(items.len() - 1);
            items[idx].clone()
        })
        .collect()
}
