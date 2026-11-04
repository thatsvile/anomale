#[derive(Debug, Clone)]
pub struct Candidate {
    pub rgb: [f64; 3],
    pub lab: [f64; 3],
    pub lch: [f64; 3],
    pub hsv: [f64; 3],
    pub hex: String,
    pub count: usize,
    pub share: f64,
    pub score: f64,
}

pub struct XorShift(u32);

impl XorShift {
    pub fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9e37_79b9 } else { seed })
    }

    pub fn next_f64(&mut self) -> f64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        f64::from(s) / 4_294_967_296.0
    }
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.min(hi).max(lo)
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn srgb_linear(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_srgb(x: f64) -> f64 {
    if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

pub fn rgb_lab(r: f64, g: f64, b: f64) -> [f64; 3] {
    let r = srgb_linear(r);
    let g = srgb_linear(g);
    let b = srgb_linear(b);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

pub fn lab_linear_rgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_,
    ]
}

pub fn lab_to_rgb(lab: [f64; 3]) -> [f64; 3] {
    lab_linear_rgb(lab[0], lab[1], lab[2]).map(|v| linear_srgb(v).clamp(0.0, 1.0) * 255.0)
}

pub fn rgb_hex(rgb: [f64; 3]) -> String {
    let mut hex = String::from("#");
    for ch in rgb {
        let v = clamp(ch.round(), 0.0, 255.0) as u8;
        hex.push_str(&format!("{v:02X}"));
    }
    hex
}

pub fn lab_to_lch(lab: [f64; 3]) -> [f64; 3] {
    let c = lab[1].hypot(lab[2]);
    let h = ((lab[2].atan2(lab[1]).to_degrees() % 360.0) + 360.0) % 360.0;
    [lab[0], c, h]
}

pub fn lch_to_lab(l: f64, c: f64, h: f64) -> [f64; 3] {
    let rad = h.to_radians();
    [l, c * rad.cos(), c * rad.sin()]
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.iter()
        .all(|x| x.is_finite() && *x >= -0.0000001 && *x <= 1.0000001)
}

pub fn lch_hex(l: f64, c: f64, h: f64) -> String {
    let l = clamp(l, 0.0, 1.0);
    let mut c = c.max(0.0);
    let mut lab = lch_to_lab(l, c, h);
    if !in_gamut(lab_linear_rgb(lab[0], lab[1], lab[2])) {
        let mut lo = 0.0;
        let mut hi = c;
        for _ in 0..18 {
            let mid = (lo + hi) / 2.0;
            let trial = lch_to_lab(l, mid, h);
            if in_gamut(lab_linear_rgb(trial[0], trial[1], trial[2])) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        c = lo;
        lab = lch_to_lab(l, c, h);
    }
    rgb_hex(lab_to_rgb(lab))
}

pub fn rgb_hsv(r: f64, g: f64, b: f64) -> [f64; 3] {
    let r = r / 255.0;
    let g = g / 255.0;
    let b = b / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = 0.0;
    if d > 0.0 {
        if max == r {
            h = ((g - b) / d) % 6.0;
        } else if max == g {
            h = (b - r) / d + 2.0;
        } else {
            h = (r - g) / d + 4.0;
        }
        h = (h * 60.0 + 360.0) % 360.0;
    }
    let s = if max == 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

/// Standard HSL: H in degrees `[0, 360)`, S/L in `[0, 1]`.
pub fn rgb_hsl(r: f64, g: f64, b: f64) -> [f64; 3] {
    let r = r / 255.0;
    let g = g / 255.0;
    let b = b / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let mut h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h * 60.0 + 360.0) % 360.0;
    [h, s, l]
}

pub fn color_point(rgb: [f64; 3]) -> Candidate {
    let lab = rgb_lab(rgb[0], rgb[1], rgb[2]);
    Candidate {
        rgb,
        lab,
        lch: lab_to_lch(lab),
        hsv: rgb_hsv(rgb[0], rgb[1], rgb[2]),
        hex: rgb_hex(rgb),
        count: 0,
        share: 0.0,
        score: 0.0,
    }
}

pub fn candidate(rgb: [f64; 3], count: usize, total: usize, score: f64) -> Candidate {
    let mut c = color_point(rgb);
    c.count = count;
    c.share = if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    };
    c.score = score;
    c
}

pub fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let x = a[0] - b[0];
    let y = a[1] - b[1];
    let z = a[2] - b[2];
    x * x + y * y + z * z
}

pub fn de(a: [f64; 3], b: [f64; 3]) -> f64 {
    dist2(a, b).sqrt() * 100.0
}

pub fn bucket_score(share: f64, exponent: f64, chroma_reward: f64) -> f64 {
    share.powf(exponent) * (1.0 + chroma_reward)
}

pub fn rank_and_select(candidates: &[Candidate], n: usize, separation: f64) -> Vec<Candidate> {
    let mut by_score: Vec<Candidate> = candidates.to_vec();
    by_score.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.count.cmp(&a.count))
            .then(a.hex.cmp(&b.hex))
    });
    let mut selected = Vec::new();
    for c in &by_score {
        if selected
            .iter()
            .all(|v: &Candidate| de(v.lab, c.lab) >= separation)
        {
            selected.push(c.clone());
            if selected.len() >= n {
                break;
            }
        }
    }
    if selected.len() < n {
        for c in &by_score {
            if selected.iter().all(|v| v.hex != c.hex) {
                selected.push(c.clone());
                if selected.len() >= n {
                    break;
                }
            }
        }
    }
    selected
}

pub fn hue_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

pub fn mix_hue(a: f64, b: f64, t: f64) -> f64 {
    let delta = ((b - a + 540.0) % 360.0) - 180.0;
    (a + t * delta + 360.0) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xorshift_unit_interval() {
        let mut rng = XorShift::new(0x0515_CC11);
        for _ in 0..32 {
            let v = rng.next_f64();
            assert!(v >= 0.0 && v < 1.0);
        }
    }

    #[test]
    fn srgb_roundtrip_mid_gray() {
        let lab = rgb_lab(128.0, 128.0, 128.0);
        let rgb = lab_to_rgb(lab);
        assert!((rgb[0] - 128.0).abs() < 1.5);
        assert!((rgb[1] - 128.0).abs() < 1.5);
        assert!((rgb[2] - 128.0).abs() < 1.5);
    }

    #[test]
    fn hsv_primary_red() {
        let hsv = rgb_hsv(255.0, 0.0, 0.0);
        assert!(hsv[0].abs() < 1e-9 || (hsv[0] - 360.0).abs() < 1e-9);
        assert!((hsv[1] - 1.0).abs() < 1e-9);
        assert!((hsv[2] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hsl_primary_red() {
        let hsl = rgb_hsl(255.0, 0.0, 0.0);
        assert!(hsl[0].abs() < 1e-9 || (hsl[0] - 360.0).abs() < 1e-9);
        assert!((hsl[1] - 1.0).abs() < 1e-9);
        assert!((hsl[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn hsl_gray_and_green() {
        let gray = rgb_hsl(128.0, 128.0, 128.0);
        assert!((gray[0]).abs() < 1e-9);
        assert!((gray[1]).abs() < 1e-9);
        assert!((gray[2] - 128.0 / 255.0).abs() < 1e-9);

        let green = rgb_hsl(0.0, 255.0, 0.0);
        assert!((green[0] - 120.0).abs() < 1e-9);
        assert!((green[1] - 1.0).abs() < 1e-9);
        assert!((green[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn hex_is_uppercase() {
        assert_eq!(rgb_hex([15.0, 16.0, 255.0]), "#0F10FF");
    }

    #[test]
    fn lch_hex_stays_hash_rrggbb() {
        let hex = lch_hex(0.65, 0.12, 40.0);
        assert!(hex.starts_with('#'));
        assert_eq!(hex.len(), 7);
        assert!(hex[1..].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
