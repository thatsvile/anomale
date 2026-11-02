use anyhow::{bail, Context, Result};
use image::RgbaImage;
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs;
use std::io::Cursor;
use std::path::Path;

use crate::color::{color_point, Candidate, XorShift};
use crate::recipe::{AlphaMatte, SampleParams};

const SAMPLE_SEED: u32 = 0x0515_CC11;

pub fn sample_image(path: &Path, params: &SampleParams) -> Result<Vec<Candidate>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let rgba = if looks_like_jpeg(&bytes) {
        load_jpeg(&bytes, params.max_edge)
            .with_context(|| format!("decoding jpeg {}", path.display()))?
    } else {
        image::load_from_memory(&bytes)
            .with_context(|| format!("decoding {}", path.display()))?
            .to_rgba8()
    };
    if rgba.width() == 0 || rgba.height() == 0 {
        bail!("image has no pixels");
    }
    sample_rgba(&rgba, params)
}

fn looks_like_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// Request size that forbids 1/8 IDCT: at least a quarter of native, and never below `max_edge`.
fn jpeg_scale_request(native_w: u32, native_h: u32, max_edge: u32) -> (u16, u16) {
    let clamp_u16 = |v: u32| v.max(1).min(u32::from(u16::MAX)) as u16;
    (
        clamp_u16(max_edge.max(native_w.div_ceil(4))),
        clamp_u16(max_edge.max(native_h.div_ceil(4))),
    )
}

fn load_jpeg(bytes: &[u8], max_edge: u32) -> Result<RgbaImage> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder
        .read_info()
        .map_err(|err| anyhow::anyhow!("jpeg headers: {err}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("jpeg headers missing after read"))?;
    let native_w = u32::from(info.width);
    let native_h = u32::from(info.height);
    if native_w == 0 || native_h == 0 {
        bail!("image has no pixels");
    }
    if native_w.max(native_h) > max_edge {
        let (req_w, req_h) = jpeg_scale_request(native_w, native_h, max_edge);
        decoder
            .scale(req_w, req_h)
            .map_err(|err| anyhow::anyhow!("jpeg scale: {err}"))?;
    }
    let pixels = decoder
        .decode()
        .map_err(|err| anyhow::anyhow!("jpeg decode: {err}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("jpeg info missing after decode"))?;
    jpeg_pixels_to_rgba(
        u32::from(info.width),
        u32::from(info.height),
        info.pixel_format,
        &pixels,
    )
}

fn jpeg_pixels_to_rgba(
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: &[u8],
) -> Result<RgbaImage> {
    if width == 0 || height == 0 {
        bail!("image has no pixels");
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow::anyhow!("jpeg dimensions overflow"))?;
    let mut buf = Vec::with_capacity(n.saturating_mul(4));
    match format {
        PixelFormat::L8 => {
            if pixels.len() < n {
                bail!("jpeg luma buffer short");
            }
            for &v in pixels.iter().take(n) {
                buf.extend_from_slice(&[v, v, v, 255]);
            }
        }
        PixelFormat::L16 => {
            if pixels.len() < n.saturating_mul(2) {
                bail!("jpeg luma16 buffer short");
            }
            for chunk in pixels.chunks_exact(2).take(n) {
                let v = u16::from_le_bytes([chunk[0], chunk[1]]);
                let v = (v >> 8) as u8;
                buf.extend_from_slice(&[v, v, v, 255]);
            }
        }
        PixelFormat::RGB24 => {
            if pixels.len() < n.saturating_mul(3) {
                bail!("jpeg rgb buffer short");
            }
            for chunk in pixels.chunks_exact(3).take(n) {
                buf.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        PixelFormat::CMYK32 => {
            if pixels.len() < n.saturating_mul(4) {
                bail!("jpeg cmyk buffer short");
            }
            for chunk in pixels.chunks_exact(4).take(n) {
                // jpeg-decoder stores inverted CMYK (255 = no ink).
                let k = u16::from(chunk[3]);
                let r = (u16::from(chunk[0]) * k / 255) as u8;
                let g = (u16::from(chunk[1]) * k / 255) as u8;
                let b = (u16::from(chunk[2]) * k / 255) as u8;
                buf.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    RgbaImage::from_raw(width, height, buf)
        .ok_or_else(|| anyhow::anyhow!("jpeg rgba packing failed"))
}

pub fn sample_rgba(img: &RgbaImage, params: &SampleParams) -> Result<Vec<Candidate>> {
    let w = img.width();
    let h = img.height();
    let cap = params.sample_cap.max(1) as f64;
    let step = ((f64::from(w) * f64::from(h) / cap).sqrt()).max(1.0);
    let mut rng = XorShift::new(SAMPLE_SEED);
    let mut pts = Vec::new();
    let mut gy = 0.0;
    while gy < f64::from(h) {
        let mut gx = 0.0;
        while gx < f64::from(w) {
            let x = ((gx + rng.next_f64() * step).floor() as u32).min(w.saturating_sub(1));
            let y = ((gy + rng.next_f64() * step).floor() as u32).min(h.saturating_sub(1));
            let px = img.get_pixel(x, y).0;
            let alpha = px[3];
            if alpha <= params.alpha_threshold || alpha == 0 {
                gx += step;
                continue;
            }
            let mut rgb = [f64::from(px[0]), f64::from(px[1]), f64::from(px[2])];
            if alpha < 255 && params.alpha_matte != AlphaMatte::Raw {
                let a = f64::from(alpha) / 255.0;
                let matte = match params.alpha_matte {
                    AlphaMatte::Dark => 16.0,
                    AlphaMatte::Light => 245.0,
                    AlphaMatte::Raw => 0.0,
                };
                for ch in &mut rgb {
                    *ch = (*ch * a + matte * (1.0 - a)).round();
                }
            }
            pts.push(color_point(rgb));
            gx += step;
        }
        gy += step;
    }
    Ok(pts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Recipe;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
    use std::fs;

    #[test]
    fn samples_opaque_grid() {
        let mut img = RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = Rgba([200, 40, 40, 255]);
        }
        let params = Recipe::lab_defaults("t").sample;
        let pts = sample_rgba(&img, &params).unwrap();
        assert!(!pts.is_empty());
        assert!(pts.iter().all(|p| (p.rgb[0] - 200.0).abs() < 1.0));
    }

    #[test]
    fn sniffs_jpeg_magic() {
        assert!(looks_like_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!looks_like_jpeg(&[0x89, 0x50, 0x4E, 0x47]));
        assert!(!looks_like_jpeg(&[0xFF, 0xD8]));
    }

    #[test]
    fn jpeg_scale_request_never_allows_eighth() {
        assert_eq!(jpeg_scale_request(6000, 4000, 640), (1500, 1000));
        assert_eq!(jpeg_scale_request(1920, 1080, 640), (640, 640));
        assert_eq!(jpeg_scale_request(800, 600, 640), (640, 640));
        assert_eq!(jpeg_scale_request(160, 80, 20), (40, 20));
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vilecolors-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn samples_small_jpeg_without_scale() {
        let dir = temp_dir("jpg-small");
        let path = dir.join("small.jpg");
        let mut img = RgbImage::new(96, 64);
        for p in img.pixels_mut() {
            *p = Rgb([200, 40, 40]);
        }
        img.save_with_format(&path, ImageFormat::Jpeg).unwrap();
        let mut params = Recipe::lab_defaults("t").sample;
        params.max_edge = 64;
        params.sample_cap = 256;
        let pts = sample_image(&path, &params).unwrap();
        assert!(!pts.is_empty());
        assert!(pts.iter().any(|p| p.rgb[0] > p.rgb[1] + 40.0));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn samples_scaled_jpeg_at_quarter() {
        let dir = temp_dir("jpg-scale");
        let path = dir.join("split.jpg");
        let mut img = RgbImage::new(160, 80);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 80 {
                Rgb([220, 20, 20])
            } else {
                Rgb([20, 40, 200])
            };
        }
        img.save_with_format(&path, ImageFormat::Jpeg).unwrap();
        let mut params = Recipe::lab_defaults("t").sample;
        params.max_edge = 20;
        params.sample_cap = 128;
        let pts = sample_image(&path, &params).unwrap();
        assert!(!pts.is_empty());
        assert!(
            pts.iter().any(|p| p.rgb[0] > p.rgb[2] + 40.0),
            "expected a red sample"
        );
        assert!(
            pts.iter().any(|p| p.rgb[2] > p.rgb[0] + 40.0),
            "expected a blue sample"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn samples_png_larger_than_max_edge() {
        let dir = temp_dir("png-large");
        let path = dir.join("big.png");
        let mut img = RgbaImage::new(64, 32);
        for p in img.pixels_mut() {
            *p = Rgba([30, 180, 70, 255]);
        }
        img.save(&path).unwrap();
        let mut params = Recipe::lab_defaults("t").sample;
        params.max_edge = 16;
        params.sample_cap = 64;
        let pts = sample_image(&path, &params).unwrap();
        assert!(!pts.is_empty());
        assert!(pts.iter().all(|p| (p.rgb[1] - 180.0).abs() < 1.0));
        let _ = fs::remove_dir_all(dir);
    }
}
