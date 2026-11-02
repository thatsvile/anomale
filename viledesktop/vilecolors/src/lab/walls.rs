use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbaImage};
use jpeg_decoder::{Decoder, PixelFormat};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

const EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];
const THUMB_EDGE: u32 = 192;

pub fn scan_images(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, 0, 3, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: u32, max_depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < max_depth {
                dirs.push(path);
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if EXTS.contains(&ext.as_str()) {
            out.push(path);
        }
    }
    dirs.sort();
    for child in dirs {
        walk(&child, depth + 1, max_depth, out);
    }
}

pub fn page_start(sel: usize, cols: usize, rows: usize) -> usize {
    let size = page_size(cols, rows);
    (sel / size) * size
}

pub fn page_size(cols: usize, rows: usize) -> usize {
    cols.max(1).saturating_mul(rows.max(1))
}

pub fn page_count(n: usize, cols: usize, rows: usize) -> usize {
    let size = page_size(cols, rows);
    n.div_ceil(size).max(1)
}

pub struct ThumbCache {
    protocols: HashMap<PathBuf, StatefulProtocol>,
    failed: HashSet<PathBuf>,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            protocols: HashMap::new(),
            failed: HashSet::new(),
        }
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut StatefulProtocol> {
        self.protocols.get_mut(path)
    }

    pub fn needs_pump(&self, keep: &[PathBuf]) -> bool {
        keep.iter()
            .any(|path| !self.protocols.contains_key(path) && !self.failed.contains(path))
    }

    pub fn pump(&mut self, picker: &mut Picker, keep: &[PathBuf], budget: usize) -> bool {
        self.protocols.retain(|path, _| keep.contains(path));
        let mut decoded = 0;
        for path in keep {
            if decoded >= budget {
                break;
            }
            if self.protocols.contains_key(path) || self.failed.contains(path) {
                continue;
            }
            match load_thumb(path) {
                Some(thumb) => {
                    self.protocols
                        .insert(path.clone(), picker.new_resize_protocol(thumb));
                    decoded += 1;
                }
                None => {
                    self.failed.insert(path.clone());
                }
            }
        }
        decoded > 0
    }
}

fn load_thumb(path: &Path) -> Option<DynamicImage> {
    let bytes = fs::read(path).ok()?;
    let img = if looks_like_jpeg(&bytes) {
        load_jpeg_thumb(&bytes).or_else(|| image::load_from_memory(&bytes).ok())?
    } else {
        image::load_from_memory(&bytes).ok()?
    };
    if img.width() == 0 || img.height() == 0 {
        return None;
    }
    Some(img.thumbnail(THUMB_EDGE, THUMB_EDGE))
}

fn looks_like_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

fn load_jpeg_thumb(bytes: &[u8]) -> Option<DynamicImage> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    let native_w = u32::from(info.width);
    let native_h = u32::from(info.height);
    if native_w == 0 || native_h == 0 {
        return None;
    }
    if native_w.max(native_h) > THUMB_EDGE {
        let req = THUMB_EDGE.max(1).min(u32::from(u16::MAX)) as u16;
        decoder.scale(req, req).ok()?;
    }
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    jpeg_pixels_to_rgba(
        u32::from(info.width),
        u32::from(info.height),
        info.pixel_format,
        &pixels,
    )
    .ok()
    .map(DynamicImage::ImageRgba8)
}

fn jpeg_pixels_to_rgba(
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: &[u8],
) -> anyhow::Result<RgbaImage> {
    if width == 0 || height == 0 {
        anyhow::bail!("image has no pixels");
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow::anyhow!("jpeg dimensions overflow"))?;
    let mut buf = Vec::with_capacity(n.saturating_mul(4));
    match format {
        PixelFormat::L8 => {
            if pixels.len() < n {
                anyhow::bail!("jpeg luma buffer short");
            }
            for &v in pixels.iter().take(n) {
                buf.extend_from_slice(&[v, v, v, 255]);
            }
        }
        PixelFormat::L16 => {
            if pixels.len() < n.saturating_mul(2) {
                anyhow::bail!("jpeg luma16 buffer short");
            }
            for chunk in pixels.chunks_exact(2).take(n) {
                let v = u16::from_le_bytes([chunk[0], chunk[1]]);
                let v = (v >> 8) as u8;
                buf.extend_from_slice(&[v, v, v, 255]);
            }
        }
        PixelFormat::RGB24 => {
            if pixels.len() < n.saturating_mul(3) {
                anyhow::bail!("jpeg rgb buffer short");
            }
            for chunk in pixels.chunks_exact(3).take(n) {
                buf.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        PixelFormat::CMYK32 => {
            if pixels.len() < n.saturating_mul(4) {
                anyhow::bail!("jpeg cmyk buffer short");
            }
            for chunk in pixels.chunks_exact(4).take(n) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    #[test]
    fn scans_png_in_folder() {
        let dir = std::env::temp_dir().join(format!("vile-walls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        let img = RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]));
        img.save(&path).unwrap();
        std::fs::write(dir.join("notes.txt"), "nope").unwrap();
        let found = scan_images(&dir);
        assert_eq!(found, vec![path]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_start_is_aligned() {
        assert_eq!(page_start(0, 4, 3), 0);
        assert_eq!(page_start(11, 4, 3), 0);
        assert_eq!(page_start(12, 4, 3), 12);
        assert_eq!(page_start(25, 4, 3), 24);
        assert_eq!(page_count(48, 4, 3), 4);
        assert_eq!(page_count(1, 4, 3), 1);
        assert_eq!(page_count(0, 4, 3), 1);
    }

    #[test]
    fn jpeg_thumb_long_edge_fits() {
        let dir = std::env::temp_dir().join(format!(
            "vile-walls-jpg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jpg");
        let img = RgbImage::from_pixel(800, 600, Rgb([40, 80, 120]));
        img.save_with_format(&path, ImageFormat::Jpeg).unwrap();
        let thumb = load_thumb(&path).unwrap();
        assert!(thumb.width() > 0 && thumb.height() > 0);
        assert!(
            thumb.width().max(thumb.height()) <= THUMB_EDGE,
            "{}x{}",
            thumb.width(),
            thumb.height()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
