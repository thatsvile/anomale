use crate::recipe::{AlphaMatte, Assignment, BgSource, ExtractorKind, Recipe, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KnobPane {
    Sample,
    Extract,
    Map,
}

impl KnobPane {
    pub fn next(self) -> Self {
        match self {
            Self::Sample => Self::Extract,
            Self::Extract => Self::Map,
            Self::Map => Self::Sample,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Sample => Self::Map,
            Self::Extract => Self::Sample,
            Self::Map => Self::Extract,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sample => "Sample",
            Self::Extract => "Extract",
            Self::Map => "Map",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobId {
    MaxEdge,
    SampleCap,
    AlphaThreshold,
    AlphaMatte,
    CutBits,
    CutBoxes,
    CutSatBias,
    CutPop,
    CutSep,
    CutVivid,
    HueH,
    HueS,
    HueV,
    HueVivid,
    HuePop,
    HueSep,
    KmK,
    KmIter,
    KmVivid,
    KmPop,
    KmSep,
    KmSeed,
    MapAssign,
    MapTheme,
    MapBg,
    MapHueSnap,
    MapBgL,
    MapFgL,
    MapNormalL,
    MapBrightL,
    MapPreserveL,
    MapChroma,
    MapMinC,
    MapBrightChroma,
    MapNeutralC,
    MapMutedL,
    MapBgChroma,
    MapBrightFloor,
}

#[derive(Clone, Copy)]
pub enum KnobKind {
    Range {
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
    },
    Enum(&'static [&'static str]),
}

#[derive(Clone, Copy)]
pub struct Knob {
    pub id: KnobId,
    pub label: &'static str,
    pub kind: KnobKind,
}

const SAMPLE: &[Knob] = &[
    range(
        KnobId::MaxEdge,
        "Raster longest edge",
        160.0,
        1000.0,
        40.0,
        "px",
    ),
    range(
        KnobId::SampleCap,
        "Pixel sample cap",
        1000.0,
        24000.0,
        1000.0,
        "",
    ),
    range(KnobId::AlphaThreshold, "Alpha cutoff", 0.0, 254.0, 1.0, ""),
    enumer(
        KnobId::AlphaMatte,
        "Translucent pixels",
        &["dark", "light", "raw"],
    ),
];

const CUT: &[Knob] = &[
    range(KnobId::CutBits, "RGB bucket bits", 3.0, 6.0, 1.0, ""),
    range(KnobId::CutBoxes, "Median-cut boxes", 8.0, 48.0, 2.0, ""),
    range(KnobId::CutSatBias, "Saturation bias", 0.0, 4.0, 0.1, ""),
    range(KnobId::CutPop, "Frequency exponent", 0.2, 2.0, 0.1, ""),
    range(KnobId::CutSep, "Minimum distance", 0.0, 25.0, 1.0, "ΔE"),
    range(KnobId::CutVivid, "Vividness reward", 0.0, 4.0, 0.1, ""),
];

const HUE: &[Knob] = &[
    range(KnobId::HueH, "Hue sectors", 8.0, 48.0, 4.0, ""),
    range(KnobId::HueS, "Saturation bands", 2.0, 8.0, 1.0, ""),
    range(KnobId::HueV, "Value bands", 2.0, 8.0, 1.0, ""),
    range(KnobId::HueVivid, "Saturation reward", 0.0, 4.0, 0.1, ""),
    range(KnobId::HuePop, "Frequency exponent", 0.2, 2.0, 0.1, ""),
    range(KnobId::HueSep, "Minimum distance", 0.0, 25.0, 1.0, "ΔE"),
];

const KMEANS: &[Knob] = &[
    range(KnobId::KmK, "Cluster count", 6.0, 24.0, 1.0, ""),
    range(KnobId::KmIter, "Max iterations", 2.0, 24.0, 1.0, ""),
    range(KnobId::KmVivid, "Chroma reward", 0.0, 4.0, 0.1, ""),
    range(KnobId::KmPop, "Frequency exponent", 0.2, 2.0, 0.1, ""),
    range(KnobId::KmSep, "Minimum distance", 0.0, 25.0, 1.0, "ΔE"),
    range(KnobId::KmSeed, "Seed / initialization", 1.0, 99.0, 1.0, ""),
];

const MAP: &[Knob] = &[
    enumer(
        KnobId::MapAssign,
        "Six accent slots",
        &["ranked", "hue-order", "ansi"],
    ),
    enumer(KnobId::MapTheme, "Theme base", &["dark", "light"]),
    enumer(
        KnobId::MapBg,
        "Background hue source",
        &["dominant", "darkest", "vibrant"],
    ),
    range(
        KnobId::MapHueSnap,
        "ANSI hue target pull",
        0.0,
        100.0,
        5.0,
        "%",
    ),
    range(KnobId::MapBgL, "Background lightness", 0.02, 0.98, 0.01, ""),
    range(KnobId::MapFgL, "Foreground lightness", 0.02, 0.98, 0.01, ""),
    range(
        KnobId::MapNormalL,
        "Normal colors lightness",
        0.1,
        0.96,
        0.01,
        "",
    ),
    range(
        KnobId::MapBrightL,
        "Bright colors lightness",
        0.1,
        0.99,
        0.01,
        "",
    ),
    range(
        KnobId::MapPreserveL,
        "Preserve original lightness",
        0.0,
        100.0,
        5.0,
        "%",
    ),
    range(
        KnobId::MapChroma,
        "Accent chroma multiplier",
        0.0,
        2.5,
        0.05,
        "×",
    ),
    range(
        KnobId::MapMinC,
        "Accent minimum chroma",
        0.0,
        0.14,
        0.005,
        "",
    ),
    range(
        KnobId::MapBrightChroma,
        "Bright chroma multiplier",
        0.0,
        2.0,
        0.05,
        "×",
    ),
    range(
        KnobId::MapNeutralC,
        "Neutral chroma ceiling",
        0.0,
        0.14,
        0.005,
        "",
    ),
    range(KnobId::MapMutedL, "Color 8 lightness", 0.2, 0.8, 0.01, ""),
    range(
        KnobId::MapBgChroma,
        "Background chroma ceiling",
        0.0,
        0.14,
        0.005,
        "",
    ),
    range(
        KnobId::MapBrightFloor,
        "Bright min. L increase",
        0.0,
        0.25,
        0.01,
        "",
    ),
];

const fn range(
    id: KnobId,
    label: &'static str,
    min: f64,
    max: f64,
    step: f64,
    unit: &'static str,
) -> Knob {
    Knob {
        id,
        label,
        kind: KnobKind::Range {
            min,
            max,
            step,
            unit,
        },
    }
}

const fn enumer(id: KnobId, label: &'static str, options: &'static [&'static str]) -> Knob {
    Knob {
        id,
        label,
        kind: KnobKind::Enum(options),
    }
}

pub fn knobs_for(pane: KnobPane, extractor: ExtractorKind) -> &'static [Knob] {
    match pane {
        KnobPane::Sample => SAMPLE,
        KnobPane::Extract => match extractor {
            ExtractorKind::Cut => CUT,
            ExtractorKind::Hue => HUE,
            ExtractorKind::Kmeans => KMEANS,
        },
        KnobPane::Map => MAP,
    }
}

pub fn get_number(recipe: &Recipe, id: KnobId) -> f64 {
    match id {
        KnobId::MaxEdge => f64::from(recipe.sample.max_edge),
        KnobId::SampleCap => f64::from(recipe.sample.sample_cap),
        KnobId::AlphaThreshold => f64::from(recipe.sample.alpha_threshold),
        KnobId::CutBits => f64::from(recipe.cut.bits),
        KnobId::CutBoxes => f64::from(recipe.cut.boxes),
        KnobId::CutSatBias => recipe.cut.sat_bias,
        KnobId::CutPop => recipe.cut.pop_exponent,
        KnobId::CutSep => recipe.cut.separation,
        KnobId::CutVivid => recipe.cut.vivid,
        KnobId::HueH => f64::from(recipe.hue.h_bins),
        KnobId::HueS => f64::from(recipe.hue.s_bins),
        KnobId::HueV => f64::from(recipe.hue.v_bins),
        KnobId::HueVivid => recipe.hue.vivid,
        KnobId::HuePop => recipe.hue.pop_exponent,
        KnobId::HueSep => recipe.hue.separation,
        KnobId::KmK => f64::from(recipe.kmeans.k),
        KnobId::KmIter => f64::from(recipe.kmeans.iterations),
        KnobId::KmVivid => recipe.kmeans.vivid,
        KnobId::KmPop => recipe.kmeans.pop_exponent,
        KnobId::KmSep => recipe.kmeans.separation,
        KnobId::KmSeed => f64::from(recipe.kmeans.seed),
        KnobId::MapHueSnap => recipe.map.hue_snap,
        KnobId::MapBgL => recipe.map.bg_l,
        KnobId::MapFgL => recipe.map.fg_l,
        KnobId::MapNormalL => recipe.map.normal_l,
        KnobId::MapBrightL => recipe.map.bright_l,
        KnobId::MapPreserveL => recipe.map.preserve_l,
        KnobId::MapChroma => recipe.map.chroma,
        KnobId::MapMinC => recipe.map.min_c,
        KnobId::MapBrightChroma => recipe.map.bright_chroma,
        KnobId::MapNeutralC => recipe.map.neutral_c,
        KnobId::MapMutedL => recipe.map.muted_l,
        KnobId::MapBgChroma => recipe.map.bg_chroma,
        KnobId::MapBrightFloor => recipe.map.bright_floor,
        _ => 0.0,
    }
}

pub fn get_enum(recipe: &Recipe, id: KnobId) -> &'static str {
    match id {
        KnobId::AlphaMatte => match recipe.sample.alpha_matte {
            AlphaMatte::Dark => "dark",
            AlphaMatte::Light => "light",
            AlphaMatte::Raw => "raw",
        },
        KnobId::MapAssign => match recipe.map.assignment {
            Assignment::Ranked => "ranked",
            Assignment::HueOrder => "hue-order",
            Assignment::Ansi => "ansi",
        },
        KnobId::MapTheme => match recipe.map.theme {
            Theme::Dark => "dark",
            Theme::Light => "light",
        },
        KnobId::MapBg => match recipe.map.bg_source {
            BgSource::Dominant => "dominant",
            BgSource::Darkest => "darkest",
            BgSource::Vibrant => "vibrant",
        },
        _ => "",
    }
}

fn set_number(recipe: &mut Recipe, id: KnobId, value: f64) {
    let round_u = |v: f64| v.round().clamp(0.0, u32::MAX as f64) as u32;
    let round_u8 = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    match id {
        KnobId::MaxEdge => recipe.sample.max_edge = round_u(value),
        KnobId::SampleCap => recipe.sample.sample_cap = round_u(value),
        KnobId::AlphaThreshold => recipe.sample.alpha_threshold = round_u8(value),
        KnobId::CutBits => recipe.cut.bits = round_u(value),
        KnobId::CutBoxes => recipe.cut.boxes = round_u(value),
        KnobId::CutSatBias => recipe.cut.sat_bias = value,
        KnobId::CutPop => recipe.cut.pop_exponent = value,
        KnobId::CutSep => recipe.cut.separation = value,
        KnobId::CutVivid => recipe.cut.vivid = value,
        KnobId::HueH => recipe.hue.h_bins = round_u(value),
        KnobId::HueS => recipe.hue.s_bins = round_u(value),
        KnobId::HueV => recipe.hue.v_bins = round_u(value),
        KnobId::HueVivid => recipe.hue.vivid = value,
        KnobId::HuePop => recipe.hue.pop_exponent = value,
        KnobId::HueSep => recipe.hue.separation = value,
        KnobId::KmK => recipe.kmeans.k = round_u(value),
        KnobId::KmIter => recipe.kmeans.iterations = round_u(value),
        KnobId::KmVivid => recipe.kmeans.vivid = value,
        KnobId::KmPop => recipe.kmeans.pop_exponent = value,
        KnobId::KmSep => recipe.kmeans.separation = value,
        KnobId::KmSeed => recipe.kmeans.seed = round_u(value),
        KnobId::MapHueSnap => recipe.map.hue_snap = value,
        KnobId::MapBgL => recipe.map.bg_l = value,
        KnobId::MapFgL => recipe.map.fg_l = value,
        KnobId::MapNormalL => recipe.map.normal_l = value,
        KnobId::MapBrightL => recipe.map.bright_l = value,
        KnobId::MapPreserveL => recipe.map.preserve_l = value,
        KnobId::MapChroma => recipe.map.chroma = value,
        KnobId::MapMinC => recipe.map.min_c = value,
        KnobId::MapBrightChroma => recipe.map.bright_chroma = value,
        KnobId::MapNeutralC => recipe.map.neutral_c = value,
        KnobId::MapMutedL => recipe.map.muted_l = value,
        KnobId::MapBgChroma => recipe.map.bg_chroma = value,
        KnobId::MapBrightFloor => recipe.map.bright_floor = value,
        _ => {}
    }
}

fn set_enum(recipe: &mut Recipe, id: KnobId, value: &str) {
    match id {
        KnobId::AlphaMatte => {
            recipe.sample.alpha_matte = match value {
                "light" => AlphaMatte::Light,
                "raw" => AlphaMatte::Raw,
                _ => AlphaMatte::Dark,
            };
        }
        KnobId::MapAssign => {
            recipe.map.assignment = match value {
                "hue-order" => Assignment::HueOrder,
                "ansi" => Assignment::Ansi,
                _ => Assignment::Ranked,
            };
        }
        KnobId::MapTheme => {
            recipe.map.theme = match value {
                "light" => Theme::Light,
                _ => Theme::Dark,
            };
        }
        KnobId::MapBg => {
            recipe.map.bg_source = match value {
                "dominant" => BgSource::Dominant,
                "vibrant" => BgSource::Vibrant,
                _ => BgSource::Darkest,
            };
        }
        _ => {}
    }
}

fn apply_theme_presets(recipe: &mut Recipe) {
    match recipe.map.theme {
        Theme::Dark => {
            recipe.map.bg_l = 0.14;
            recipe.map.fg_l = 0.91;
            recipe.map.normal_l = 0.65;
            recipe.map.bright_l = 0.79;
            recipe.map.muted_l = 0.44;
        }
        Theme::Light => {
            recipe.map.bg_l = 0.95;
            recipe.map.fg_l = 0.17;
            recipe.map.normal_l = 0.48;
            recipe.map.bright_l = 0.33;
            recipe.map.muted_l = 0.68;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    Sample,
    Extract,
    Map,
}

pub fn dirty_for(id: KnobId) -> Dirty {
    match id {
        KnobId::MaxEdge | KnobId::SampleCap | KnobId::AlphaThreshold | KnobId::AlphaMatte => {
            Dirty::Sample
        }
        KnobId::MapAssign
        | KnobId::MapTheme
        | KnobId::MapBg
        | KnobId::MapHueSnap
        | KnobId::MapBgL
        | KnobId::MapFgL
        | KnobId::MapNormalL
        | KnobId::MapBrightL
        | KnobId::MapPreserveL
        | KnobId::MapChroma
        | KnobId::MapMinC
        | KnobId::MapBrightChroma
        | KnobId::MapNeutralC
        | KnobId::MapMutedL
        | KnobId::MapBgChroma
        | KnobId::MapBrightFloor => Dirty::Map,
        _ => Dirty::Extract,
    }
}

pub fn step(recipe: &mut Recipe, knob: Knob, dir: i32, coarse: bool) -> Dirty {
    match knob.kind {
        KnobKind::Range { min, max, step, .. } => {
            let mul = if coarse { 4.0 } else { 1.0 };
            let mut value = get_number(recipe, knob.id) + f64::from(dir) * step * mul;
            value = value.clamp(min, max);
            if step >= 1.0 {
                value = value.round();
            }
            set_number(recipe, knob.id, value);
        }
        KnobKind::Enum(opts) => {
            if opts.is_empty() {
                return dirty_for(knob.id);
            }
            let current = get_enum(recipe, knob.id);
            let idx = opts.iter().position(|o| *o == current).unwrap_or(0);
            let n = opts.len() as i32;
            let next = (idx as i32 + dir).rem_euclid(n) as usize;
            set_enum(recipe, knob.id, opts[next]);
            if knob.id == KnobId::MapTheme {
                apply_theme_presets(recipe);
            }
        }
    }
    dirty_for(knob.id)
}

pub fn format_value(recipe: &Recipe, knob: Knob) -> String {
    match knob.kind {
        KnobKind::Range { unit, step, .. } => {
            let v = get_number(recipe, knob.id);
            let num = if step >= 1.0 {
                format!("{}", v.round() as i64)
            } else if step >= 0.05 {
                format!("{v:.2}")
            } else {
                format!("{v:.3}")
            };
            if unit.is_empty() {
                num
            } else {
                format!("{num} {unit}")
            }
        }
        KnobKind::Enum(_) => get_enum(recipe, knob.id).to_string(),
    }
}

pub fn ratio(recipe: &Recipe, knob: Knob) -> f64 {
    match knob.kind {
        KnobKind::Range { min, max, .. } => {
            let span = max - min;
            if span <= 0.0 {
                0.0
            } else {
                ((get_number(recipe, knob.id) - min) / span).clamp(0.0, 1.0)
            }
        }
        KnobKind::Enum(opts) => {
            if opts.len() <= 1 {
                1.0
            } else {
                let current = get_enum(recipe, knob.id);
                let idx = opts.iter().position(|o| *o == current).unwrap_or(0);
                idx as f64 / (opts.len() - 1) as f64
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_clamp_to_range() {
        let mut recipe = Recipe::lab_defaults("t");
        let knob = CUT[0];
        assert_eq!(knob.id, KnobId::CutBits);
        recipe.cut.bits = 3;
        step(&mut recipe, knob, -1, false);
        assert_eq!(recipe.cut.bits, 3);
        recipe.cut.bits = 6;
        step(&mut recipe, knob, 1, false);
        assert_eq!(recipe.cut.bits, 6);
        recipe.cut.bits = 4;
        step(&mut recipe, knob, 1, false);
        assert_eq!(recipe.cut.bits, 5);
    }

    #[test]
    fn coarse_step_is_four_times() {
        let mut recipe = Recipe::lab_defaults("t");
        let knob = SAMPLE[0];
        recipe.sample.max_edge = 160;
        step(&mut recipe, knob, 1, true);
        assert_eq!(recipe.sample.max_edge, 320);
    }

    #[test]
    fn enum_cycles() {
        let mut recipe = Recipe::lab_defaults("t");
        let knob = SAMPLE[3];
        assert_eq!(get_enum(&recipe, KnobId::AlphaMatte), "dark");
        step(&mut recipe, knob, 1, false);
        assert_eq!(recipe.sample.alpha_matte, AlphaMatte::Light);
        step(&mut recipe, knob, 1, false);
        assert_eq!(recipe.sample.alpha_matte, AlphaMatte::Raw);
        step(&mut recipe, knob, 1, false);
        assert_eq!(recipe.sample.alpha_matte, AlphaMatte::Dark);
    }
}
