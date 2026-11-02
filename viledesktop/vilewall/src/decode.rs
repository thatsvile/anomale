use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs;
use std::io::Cursor;
use std::path::Path;

pub fn load_wallpaper(path: &Path, target_edge: u32) -> Result<RgbaImage> {
    let target_edge = target_edge.max(1);
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut img = if looks_like_jpeg(&bytes) {
        load_jpeg(&bytes, target_edge)
            .with_context(|| format!("decoding jpeg {}", path.display()))?
    } else {
        image::load_from_memory(&bytes)
            .with_context(|| format!("decoding {}", path.display()))?
            .to_rgba8()
    };
    let long = img.width().max(img.height());
    if long > target_edge.saturating_mul(2) {
        let scale = target_edge as f64 / long as f64;
        let w = ((img.width() as f64 * scale).round() as u32).max(1);
        let h = ((img.height() as f64 * scale).round() as u32).max(1);
        img = image::imageops::resize(&img, w, h, FilterType::Nearest);
    }
    Ok(img)
}

fn looks_like_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

fn idct_out(len: u32, scale: u32) -> u32 {
    let len = len.max(1);
    let scale = scale.max(1);
    (len.saturating_mul(scale).saturating_sub(1) / 8).saturating_add(1)
}

/// Request the smallest JPEG IDCT (1/8, 1/4, 1/2, 1) whose long edge still
/// covers 3/4 of `target_edge`. Matches jpeg-decoder's output-size formula so
/// `Decoder::scale` actually picks that IDCT instead of falling back to full-res.
pub(crate) fn jpeg_scale_request(native_w: u32, native_h: u32, target_edge: u32) -> (u16, u16) {
    let clamp = |v: u32| v.max(1).min(u32::from(u16::MAX)) as u16;
    let min_long = target_edge.saturating_mul(3) / 4;
    let min_long = min_long.max(1);
    let native_w = native_w.max(1);
    let native_h = native_h.max(1);
    let mut chosen = 8u32;
    for scale in [1u32, 2, 4] {
        let out_long = idct_out(native_w, scale).max(idct_out(native_h, scale));
        if out_long >= min_long {
            chosen = scale;
            break;
        }
    }
    (
        clamp(idct_out(native_w, chosen)),
        clamp(idct_out(native_h, chosen)),
    )
}

fn load_jpeg(bytes: &[u8], target_edge: u32) -> Result<RgbaImage> {
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
    let (req_w, req_h) = jpeg_scale_request(native_w, native_h, target_edge);
    decoder
        .scale(req_w, req_h)
        .map_err(|err| anyhow::anyhow!("jpeg scale: {err}"))?;
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
    let mut buf = vec![255u8; n.saturating_mul(4)];
    match format {
        PixelFormat::L8 => {
            if pixels.len() < n {
                bail!("jpeg luma buffer short");
            }
            for (i, &v) in pixels.iter().take(n).enumerate() {
                let o = i * 4;
                buf[o] = v;
                buf[o + 1] = v;
                buf[o + 2] = v;
            }
        }
        PixelFormat::L16 => {
            if pixels.len() < n.saturating_mul(2) {
                bail!("jpeg luma16 buffer short");
            }
            for (i, chunk) in pixels.chunks_exact(2).take(n).enumerate() {
                let v = (u16::from_le_bytes([chunk[0], chunk[1]]) >> 8) as u8;
                let o = i * 4;
                buf[o] = v;
                buf[o + 1] = v;
                buf[o + 2] = v;
            }
        }
        PixelFormat::RGB24 => {
            if pixels.len() < n.saturating_mul(3) {
                bail!("jpeg rgb buffer short");
            }
            for (i, chunk) in pixels.chunks_exact(3).take(n).enumerate() {
                let o = i * 4;
                buf[o] = chunk[0];
                buf[o + 1] = chunk[1];
                buf[o + 2] = chunk[2];
            }
        }
        PixelFormat::CMYK32 => {
            if pixels.len() < n.saturating_mul(4) {
                bail!("jpeg cmyk buffer short");
            }
            for (i, chunk) in pixels.chunks_exact(4).take(n).enumerate() {
                let k = u16::from(chunk[3]);
                let o = i * 4;
                buf[o] = (u16::from(chunk[0]) * k / 255) as u8;
                buf[o + 1] = (u16::from(chunk[1]) * k / 255) as u8;
                buf[o + 2] = (u16::from(chunk[2]) * k / 255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, buf)
        .ok_or_else(|| anyhow::anyhow!("jpeg rgba packing failed"))
}

#[cfg(test)]
mod tests {
    use super::{idct_out, jpeg_scale_request};

    #[test]
    fn jpeg_scale_allows_half_for_4782_on_1440p() {
        let (w, h) = jpeg_scale_request(4782, 2690, 2560);
        assert_eq!((w, h), (2391, 1345));
        assert_eq!(idct_out(4782, 4), 2391);
        assert_eq!(idct_out(2690, 4), 1345);
    }

    #[test]
    fn jpeg_scale_keeps_full_when_native_is_small() {
        let (w, h) = jpeg_scale_request(800, 600, 2560);
        assert_eq!((w, h), (800, 600));
    }

    #[test]
    fn jpeg_scale_uses_quarter_for_huge_source() {
        let (w, h) = jpeg_scale_request(8000, 6000, 2560);
        assert_eq!((w, h), (idct_out(8000, 2) as u16, idct_out(6000, 2) as u16));
    }

    #[test]
    fn loads_sample_wallpaper_at_half_res() {
        let path = "/home/jor/anomale/vile/thestuff/wallpaper/851628.jpg";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let started = std::time::Instant::now();
        let img = super::load_wallpaper(std::path::Path::new(path), 2560).unwrap();
        let decode = started.elapsed();
        let started = std::time::Instant::now();
        let _ = crate::blit::render(
            &img,
            vile_protocol::WallpaperMode::Fill,
            2560,
            1440,
            crate::blit::OutputRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            },
            &[],
            1,
        );
        let blit = started.elapsed();
        eprintln!(
            "wallpaper decode {}x{} in {decode:.0?} blit {blit:.0?}",
            img.width(),
            img.height()
        );
        assert!(
            img.width() <= 2500 && img.height() <= 1400,
            "expected ~half-res, got {}x{}",
            img.width(),
            img.height()
        );
        assert!(
            decode + blit < std::time::Duration::from_millis(1500),
            "decode {decode:.0?} blit {blit:.0?} too slow"
        );
    }
}
