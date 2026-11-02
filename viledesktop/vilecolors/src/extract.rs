use std::collections::BTreeMap;

use crate::color::{bucket_score, candidate, clamp, dist2, rank_and_select, Candidate, XorShift};
use crate::recipe::{CutParams, ExtractorKind, HueParams, KmeansParams, Recipe};

#[derive(Clone, Default)]
struct Bin {
    count: usize,
    r_sum: f64,
    g_sum: f64,
    b_sum: f64,
    s_sum: f64,
    qr: i32,
    qg: i32,
    qb: i32,
}

impl Bin {
    fn add(&mut self, p: &Candidate) {
        self.count += 1;
        self.r_sum += p.rgb[0];
        self.g_sum += p.rgb[1];
        self.b_sum += p.rgb[2];
        self.s_sum += p.hsv[1];
    }

    fn to_candidate(&self, total: usize, score: f64) -> Candidate {
        let d = self.count.max(1) as f64;
        candidate(
            [self.r_sum / d, self.g_sum / d, self.b_sum / d],
            self.count,
            total,
            score,
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtractReport {
    pub ranked: Vec<Candidate>,
    pub all: Vec<Candidate>,
}

impl ExtractReport {
    fn from_all(all: Vec<Candidate>, separation: f64) -> Self {
        let ranked = rank_and_select(&all, 12, separation);
        Self { ranked, all }
    }
}

pub fn extract(pixels: &[Candidate], recipe: &Recipe) -> Vec<Candidate> {
    extract_report(pixels, recipe).ranked
}

pub fn extract_report(pixels: &[Candidate], recipe: &Recipe) -> ExtractReport {
    match recipe.extractor {
        ExtractorKind::Cut => median_cut(pixels, &recipe.cut),
        ExtractorKind::Hue => hue_histogram(pixels, &recipe.hue),
        ExtractorKind::Kmeans => kmeans(pixels, &recipe.kmeans),
    }
}

pub fn extract_all_methods(pixels: &[Candidate], recipe: &Recipe) -> [ExtractReport; 3] {
    [
        median_cut(pixels, &recipe.cut),
        hue_histogram(pixels, &recipe.hue),
        kmeans(pixels, &recipe.kmeans),
    ]
}

fn median_cut(pixels: &[Candidate], p: &CutParams) -> ExtractReport {
    let bits = p.bits.clamp(1, 8);
    let shift = 8 - bits;
    let mut levels: BTreeMap<u32, Bin> = BTreeMap::new();
    for px in pixels {
        let r = (px.rgb[0].floor() as u32) >> shift;
        let g = (px.rgb[1].floor() as u32) >> shift;
        let b = (px.rgb[2].floor() as u32) >> shift;
        let key = (r << (bits * 2)) | (g << bits) | b;
        let bin = levels.entry(key).or_insert(Bin {
            qr: r as i32,
            qg: g as i32,
            qb: b as i32,
            ..Bin::default()
        });
        bin.add(px);
    }
    if levels.is_empty() {
        return ExtractReport::default();
    }
    let arr: Vec<Bin> = levels.into_values().collect();
    let mut boxes = vec![describe(&arr, p.sat_bias)];
    let target = p.boxes.max(1) as usize;
    for _ in 1..target {
        boxes.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let split_index = boxes
            .iter()
            .position(|x| x.items.len() > 1 && x.priority > 0.0);
        let Some(split_index) = split_index else {
            break;
        };
        let box_ = boxes.remove(split_index);
        let axis = box_.axis;
        let mut list = box_.items;
        list.sort_by(|a, b| match axis {
            0 => a.qr.cmp(&b.qr),
            1 => a.qg.cmp(&b.qg),
            _ => a.qb.cmp(&b.qb),
        });
        let mut acc = 0.0;
        let mut cut = 1usize;
        for i in 0..list.len().saturating_sub(1) {
            let b = &list[i];
            acc += b.count as f64 * (1.0 + p.sat_bias * b.s_sum / b.count.max(1) as f64);
            cut = i + 1;
            if acc >= box_.mass / 2.0 {
                break;
            }
        }
        boxes.push(describe(&list[..cut], p.sat_bias));
        boxes.push(describe(&list[cut..], p.sat_bias));
    }
    let total = pixels.len();
    let all: Vec<Candidate> = boxes
        .iter()
        .map(|bx| {
            let mut merged = Bin::default();
            for b in &bx.items {
                merged.count += b.count;
                merged.r_sum += b.r_sum;
                merged.g_sum += b.g_sum;
                merged.b_sum += b.b_sum;
                merged.s_sum += b.s_sum;
            }
            let mut c = merged.to_candidate(total, 0.0);
            c.score = bucket_score(
                c.share,
                p.pop_exponent,
                p.vivid * clamp(c.lch[1] / 0.16, 0.0, 1.0),
            );
            c
        })
        .collect();
    ExtractReport::from_all(all, p.separation)
}

struct BoxDesc {
    items: Vec<Bin>,
    mass: f64,
    priority: f64,
    axis: usize,
}

fn describe(items: &[Bin], sat_bias: f64) -> BoxDesc {
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    let mut mass = 0.0;
    for b in items {
        let xyz = [b.qr, b.qg, b.qb];
        for i in 0..3 {
            min[i] = min[i].min(xyz[i]);
            max[i] = max[i].max(xyz[i]);
        }
        mass += b.count as f64 * (1.0 + sat_bias * b.s_sum / b.count.max(1) as f64);
    }
    let range = [
        (max[0] - min[0]) as f64,
        (max[1] - min[1]) as f64,
        (max[2] - min[2]) as f64,
    ];
    let mut axis = 0;
    let mut best = range[0];
    for (i, r) in range.iter().enumerate().skip(1) {
        if *r > best {
            best = *r;
            axis = i;
        }
    }
    BoxDesc {
        items: items.to_vec(),
        mass,
        priority: mass * best,
        axis,
    }
}

fn hue_histogram(pixels: &[Candidate], p: &HueParams) -> ExtractReport {
    let h_bins = p.h_bins.max(1);
    let s_bins = p.s_bins.max(1);
    let v_bins = p.v_bins.max(1);
    let mut hist: BTreeMap<u32, Bin> = BTreeMap::new();
    for px in pixels {
        let h = ((px.hsv[0] / 360.0 * f64::from(h_bins)).floor() as u32).min(h_bins - 1);
        let s = ((px.hsv[1] * f64::from(s_bins)).floor() as u32).min(s_bins - 1);
        let v = ((px.hsv[2] * f64::from(v_bins)).floor() as u32).min(v_bins - 1);
        let key = (h * s_bins + s) * v_bins + v;
        hist.entry(key).or_default().add(px);
    }
    let total = pixels.len();
    let all: Vec<Candidate> = hist
        .into_values()
        .map(|bin| {
            let mut c = bin.to_candidate(total, 0.0);
            let mean_sat = bin.s_sum / bin.count.max(1) as f64;
            c.score = bucket_score(c.share, p.pop_exponent, p.vivid * mean_sat);
            c
        })
        .collect();
    ExtractReport::from_all(all, p.separation)
}

fn kmeans(pixels: &[Candidate], p: &KmeansParams) -> ExtractReport {
    let n = pixels.len();
    if n == 0 {
        return ExtractReport::default();
    }
    let k = (p.k as usize).min(n).max(1);
    let mut rng = XorShift::new(p.seed);
    let mut centers = vec![pixels[(rng.next_f64() * n as f64).floor() as usize].lab];
    let mut nearest = vec![f64::INFINITY; n];
    for _ in 1..k {
        let mut sum = 0.0;
        for i in 0..n {
            nearest[i] = nearest[i].min(dist2(pixels[i].lab, centers[centers.len() - 1]));
            sum += nearest[i];
        }
        let mut t = rng.next_f64() * sum;
        let mut idx = (rng.next_f64() * n as f64).floor() as usize;
        if sum > 1e-15 {
            for i in 0..n {
                t -= nearest[i];
                if t <= 0.0 {
                    idx = i;
                    break;
                }
            }
        }
        idx = idx.min(n - 1);
        centers.push(pixels[idx].lab);
    }
    let mut labels = vec![-1i32; n];
    for _ in 0..p.iterations {
        let mut sums = vec![[0.0; 4]; k];
        let mut changed = 0;
        for i in 0..n {
            let mut best = f64::INFINITY;
            let mut bi = 0usize;
            for (j, center) in centers.iter().enumerate() {
                let d = dist2(pixels[i].lab, *center);
                if d < best {
                    best = d;
                    bi = j;
                }
            }
            if labels[i] != bi as i32 {
                labels[i] = bi as i32;
                changed += 1;
            }
            let s = &mut sums[bi];
            let lab = pixels[i].lab;
            s[0] += lab[0];
            s[1] += lab[1];
            s[2] += lab[2];
            s[3] += 1.0;
        }
        for (j, v) in sums.iter().enumerate() {
            if v[3] > 0.0 {
                centers[j] = [v[0] / v[3], v[1] / v[3], v[2] / v[3]];
            } else {
                let idx = (rng.next_f64() * n as f64).floor() as usize;
                centers[j] = pixels[idx.min(n - 1)].lab;
            }
        }
        if changed == 0 {
            break;
        }
    }
    let mut groups = vec![Bin::default(); k];
    for px in pixels {
        let mut dmin = f64::INFINITY;
        let mut best = 0usize;
        for (j, center) in centers.iter().enumerate() {
            let d = dist2(px.lab, *center);
            if d < dmin {
                dmin = d;
                best = j;
            }
        }
        groups[best].add(px);
    }
    let all: Vec<Candidate> = groups
        .into_iter()
        .filter(|g| g.count > 0)
        .map(|g| {
            let mut c = g.to_candidate(n, 0.0);
            c.score = bucket_score(
                c.share,
                p.pop_exponent,
                p.vivid * clamp(c.lch[1] / 0.16, 0.0, 1.0),
            );
            c
        })
        .collect();
    ExtractReport::from_all(all, p.separation)
}
