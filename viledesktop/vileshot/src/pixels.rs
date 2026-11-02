use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use image::{ImageEncoder, RgbaImage};
use wayland_client::protocol::wl_shm;

pub fn shm_to_rgba(
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
    y_invert: bool,
    data: &[u8],
) -> Result<RgbaImage> {
    if width == 0 || height == 0 {
        bail!("empty screenshot");
    }
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        let src_y = if y_invert { height - 1 - y } else { y };
        let src_row = src_y as usize * stride as usize;
        let dst_row = y as usize * width as usize * 4;
        for x in 0..width {
            let i = src_row + x as usize * 4;
            if i + 3 >= data.len() {
                bail!("screenshot buffer is truncated");
            }
            let (r, g, b, a) = match format {
                wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => {
                    let b = data[i];
                    let g = data[i + 1];
                    let r = data[i + 2];
                    let a = if format == wl_shm::Format::Argb8888 {
                        data[i + 3]
                    } else {
                        255
                    };
                    (r, g, b, a)
                }
                wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => {
                    let r = data[i];
                    let g = data[i + 1];
                    let b = data[i + 2];
                    let a = if format == wl_shm::Format::Abgr8888 {
                        data[i + 3]
                    } else {
                        255
                    };
                    (r, g, b, a)
                }
                other => bail!("unsupported screenshot format {other:?}"),
            };
            let o = dst_row + x as usize * 4;
            rgba[o] = r;
            rgba[o + 1] = g;
            rgba[o + 2] = b;
            rgba[o + 3] = a;
        }
    }
    RgbaImage::from_raw(width, height, rgba).context("building rgba image")
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encoding png")?;
    Ok(buf)
}

pub fn crop_rgba(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> RgbaImage {
    let max_x = img.width();
    let max_y = img.height();
    let left = x0.min(x1).min(max_x.saturating_sub(1));
    let top = y0.min(y1).min(max_y.saturating_sub(1));
    let right = x0.max(x1).min(max_x);
    let bottom = y0.max(y1).min(max_y);
    let width = right.saturating_sub(left).max(1);
    let height = bottom.saturating_sub(top).max(1);
    image::imageops::crop_imm(img, left, top, width, height).to_image()
}

pub fn rgba_to_bgra_scaled(img: &RgbaImage, dest_w: u32, dest_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; dest_w as usize * dest_h as usize * 4];
    if dest_w == 0 || dest_h == 0 {
        return out;
    }
    let src = img.as_raw();
    let src_w = img.width();
    let src_h = img.height();
    if src_w == dest_w && src_h == dest_h {
        for (dst, px) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            dst[0] = px[2];
            dst[1] = px[1];
            dst[2] = px[0];
            dst[3] = 255;
        }
        return out;
    }
    for y in 0..dest_h {
        let sy = (y as u64 * src_h as u64 / dest_h as u64) as u32;
        let sy = sy.min(src_h.saturating_sub(1));
        for x in 0..dest_w {
            let sx = (x as u64 * src_w as u64 / dest_w as u64) as u32;
            let sx = sx.min(src_w.saturating_sub(1));
            let i = (sy as usize * src_w as usize + sx as usize) * 4;
            let o = (y as usize * dest_w as usize + x as usize) * 4;
            out[o] = src[i + 2];
            out[o + 1] = src[i + 1];
            out[o + 2] = src[i];
            out[o + 3] = 255;
        }
    }
    out
}

pub fn dim_bgra(src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        px[0] /= 2;
        px[1] /= 2;
        px[2] /= 2;
    }
    out
}

pub fn blit_crop_overlay(
    canvas: &mut [u8],
    orig: &[u8],
    dim: &[u8],
    buf_w: u32,
    buf_h: u32,
    sel: Option<(u32, u32, u32, u32)>,
) {
    let needed = buf_w as usize * buf_h as usize * 4;
    if canvas.len() < needed || orig.len() < needed || dim.len() < needed {
        return;
    }
    canvas[..needed].copy_from_slice(&dim[..needed]);
    let Some((left, top, right, bottom)) = sel else {
        return;
    };
    let left = left.min(buf_w);
    let right = right.min(buf_w).max(left);
    let top = top.min(buf_h);
    let bottom = bottom.min(buf_h).max(top);
    if right > left && bottom > top {
        let row_bytes = (right - left) as usize * 4;
        for y in top..bottom {
            let start = (y as usize * buf_w as usize + left as usize) * 4;
            canvas[start..start + row_bytes].copy_from_slice(&orig[start..start + row_bytes]);
        }
    }
    let white = [255u8, 255, 255, 255];
    let set = |canvas: &mut [u8], x: u32, y: u32| {
        if x >= buf_w || y >= buf_h {
            return;
        }
        let i = (y as usize * buf_w as usize + x as usize) * 4;
        canvas[i..i + 4].copy_from_slice(&white);
    };
    if right > left && bottom > top {
        let border = 2u32;
        for t in 0..border {
            for x in left..right {
                set(canvas, x, top.saturating_add(t));
                set(canvas, x, bottom.saturating_sub(t + 1));
            }
            for y in top..bottom {
                set(canvas, left.saturating_add(t), y);
                set(canvas, right.saturating_sub(1 + t), y);
            }
        }
    }
}

pub fn map_point(logical: f64, logical_size: u32, capture_size: u32) -> u32 {
    if logical_size == 0 || capture_size == 0 {
        return 0;
    }
    let t = (logical / logical_size as f64) * capture_size as f64;
    t.round().clamp(0.0, capture_size as f64) as u32
}

pub fn expand_directory(raw: &str) -> PathBuf {
    let raw = raw.trim();
    let mut s = raw.to_string();
    if s.contains("$XDG_PICTURES_DIR") {
        let pictures = env::var("XDG_PICTURES_DIR")
            .unwrap_or_else(|_| home_dir().join("Pictures").to_string_lossy().into_owned());
        s = s.replace("$XDG_PICTURES_DIR", &pictures);
    }
    if s == "~" {
        return home_dir();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(s)
}

pub fn save_png(dir: &Path, png: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let now = chrono::Local::now();
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut path = dir.join(format!("vile-{stamp}.png"));
    if path.exists() {
        path = dir.join(format!("vile-{stamp}-{}.png", now.format("%3f")));
    }
    std::fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::{
        blit_crop_overlay, crop_rgba, dim_bgra, expand_directory, map_point, rgba_to_bgra_scaled,
        shm_to_rgba,
    };
    use image::Rgba;
    use std::path::PathBuf;
    use wayland_client::protocol::wl_shm;

    #[test]
    fn converts_xrgb_and_flips() {
        let mut data = vec![0u8; 8];
        data[0] = 1;
        data[1] = 2;
        data[2] = 3;
        data[4] = 4;
        data[5] = 5;
        data[6] = 6;
        let img = shm_to_rgba(1, 2, 4, wl_shm::Format::Xrgb8888, true, &data).unwrap();
        assert_eq!(img.get_pixel(0, 0), &Rgba([6, 5, 4, 255]));
        assert_eq!(img.get_pixel(0, 1), &Rgba([3, 2, 1, 255]));
    }

    #[test]
    fn crops_to_inclusive_min_max() {
        let mut img = image::RgbaImage::new(4, 4);
        img.put_pixel(2, 1, Rgba([9, 8, 7, 255]));
        let cropped = crop_rgba(&img, 3, 1, 2, 1);
        assert_eq!(cropped.width(), 1);
        assert_eq!(cropped.height(), 1);
        assert_eq!(cropped.get_pixel(0, 0), &Rgba([9, 8, 7, 255]));
    }

    #[test]
    fn maps_logical_point_onto_capture() {
        assert_eq!(map_point(100.0, 1920, 3840), 200);
        assert_eq!(map_point(0.0, 1920, 3840), 0);
        assert_eq!(map_point(1919.0, 1920, 3840), 3838);
        assert_eq!(map_point(1920.0, 1920, 3840), 3840);
    }

    #[test]
    fn overlay_blit_dims_outside_and_restores_selection() {
        let mut img = image::RgbaImage::new(4, 2);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([10, 20, 30, 255]);
        }
        let orig = rgba_to_bgra_scaled(&img, img.width(), img.height());
        let dim = dim_bgra(&orig);
        assert_eq!(&orig[..4], &[30, 20, 10, 255]);
        assert_eq!(&dim[..4], &[15, 10, 5, 255]);
        let mut canvas = vec![0u8; orig.len()];
        blit_crop_overlay(&mut canvas, &orig, &dim, 4, 2, Some((1, 0, 3, 2)));
        assert_eq!(&canvas[..4], &[15, 10, 5, 255]);
        assert_eq!(&canvas[4..8], &[255, 255, 255, 255]);
        assert_eq!(&canvas[8..12], &[255, 255, 255, 255]);
        assert_eq!(&canvas[12..16], &[15, 10, 5, 255]);
    }

    #[test]
    fn expands_home_and_pictures() {
        let home = expand_directory("~");
        assert_eq!(expand_directory("~/Pictures"), home.join("Pictures"));
        std::env::set_var("XDG_PICTURES_DIR", "/tmp/pics");
        assert_eq!(
            expand_directory("$XDG_PICTURES_DIR/shots"),
            PathBuf::from("/tmp/pics/shots")
        );
        std::env::remove_var("XDG_PICTURES_DIR");
    }
}
