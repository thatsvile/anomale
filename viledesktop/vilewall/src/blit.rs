use image::RgbaImage;
use vile_protocol::WallpaperMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
pub fn render(
    image: &RgbaImage,
    mode: WallpaperMode,
    dest_w: u32,
    dest_h: u32,
    this: OutputRect,
    all: &[OutputRect],
    scale: u32,
) -> Vec<u8> {
    let mut canvas = vec![0u8; dest_w as usize * dest_h as usize * 4];
    render_into(image, mode, &mut canvas, dest_w, dest_h, this, all, scale);
    canvas
}

pub fn render_into(
    image: &RgbaImage,
    mode: WallpaperMode,
    canvas: &mut [u8],
    dest_w: u32,
    dest_h: u32,
    this: OutputRect,
    all: &[OutputRect],
    scale: u32,
) {
    let needed = dest_w as usize * dest_h as usize * 4;
    if dest_w == 0 || dest_h == 0 || canvas.len() < needed {
        return;
    }
    canvas[..needed].fill(0);
    if image.width() == 0 || image.height() == 0 {
        return;
    }
    match mode {
        WallpaperMode::Span => blit_span(image, canvas, dest_w, dest_h, this, all, scale),
        WallpaperMode::Fill => blit_fill(image, canvas, dest_w, dest_h),
        WallpaperMode::Fit => blit_fit(image, canvas, dest_w, dest_h),
        WallpaperMode::Stretch => blit_stretch(image, canvas, dest_w, dest_h),
        WallpaperMode::Center => blit_center(image, canvas, dest_w, dest_h),
        WallpaperMode::Tile => blit_tile(image, canvas, dest_w, dest_h),
    }
}

fn blit_fill(image: &RgbaImage, canvas: &mut [u8], dest_w: u32, dest_h: u32) {
    let (sx, sy, sw, sh) = cover_crop(
        image.width() as f64,
        image.height() as f64,
        dest_w as f64,
        dest_h as f64,
    );
    sample_rect(
        image, canvas, dest_w, dest_h, 0, 0, dest_w, dest_h, sx, sy, sw, sh,
    );
}

fn blit_fit(image: &RgbaImage, canvas: &mut [u8], dest_w: u32, dest_h: u32) {
    let (dx, dy, dw, dh) = contain_rect(
        image.width() as f64,
        image.height() as f64,
        dest_w as f64,
        dest_h as f64,
    );
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    sample_rect(
        image,
        canvas,
        dest_w,
        dest_h,
        dx.round() as i32,
        dy.round() as i32,
        dw.round().max(1.0) as u32,
        dh.round().max(1.0) as u32,
        0.0,
        0.0,
        image.width() as f64,
        image.height() as f64,
    );
}

fn blit_stretch(image: &RgbaImage, canvas: &mut [u8], dest_w: u32, dest_h: u32) {
    sample_rect(
        image,
        canvas,
        dest_w,
        dest_h,
        0,
        0,
        dest_w,
        dest_h,
        0.0,
        0.0,
        image.width() as f64,
        image.height() as f64,
    );
}

fn blit_center(image: &RgbaImage, canvas: &mut [u8], dest_w: u32, dest_h: u32) {
    let dx = (dest_w as i32 - image.width() as i32) / 2;
    let dy = (dest_h as i32 - image.height() as i32) / 2;
    write_bgra(canvas, dest_w, dest_h, dx, dy, image);
}

fn blit_tile(image: &RgbaImage, canvas: &mut [u8], dest_w: u32, dest_h: u32) {
    let tw = image.width() as i32;
    let th = image.height() as i32;
    if tw <= 0 || th <= 0 {
        return;
    }
    let mut y = 0i32;
    while y < dest_h as i32 {
        let mut x = 0i32;
        while x < dest_w as i32 {
            write_bgra(canvas, dest_w, dest_h, x, y, image);
            x += tw;
        }
        y += th;
    }
}

fn blit_span(
    image: &RgbaImage,
    canvas: &mut [u8],
    dest_w: u32,
    dest_h: u32,
    this: OutputRect,
    all: &[OutputRect],
    _scale: u32,
) {
    let Some(bbox) = bounding_box(all) else {
        return;
    };
    let img_w = image.width() as f64;
    let img_h = image.height() as f64;
    let (sx, sy, sw, sh) = cover_crop(img_w, img_h, bbox.width as f64, bbox.height as f64);
    let mapped_scale_x = sw / bbox.width as f64;
    let mapped_scale_y = sh / bbox.height as f64;
    let src_x = sx + (this.x - bbox.x) as f64 * mapped_scale_x;
    let src_y = sy + (this.y - bbox.y) as f64 * mapped_scale_y;
    let src_w = this.width as f64 * mapped_scale_x;
    let src_h = this.height as f64 * mapped_scale_y;
    sample_rect(
        image, canvas, dest_w, dest_h, 0, 0, dest_w, dest_h, src_x, src_y, src_w, src_h,
    );
}

pub fn bounding_box(rects: &[OutputRect]) -> Option<OutputRect> {
    let first = rects.first()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.width as i32;
    let mut max_y = first.y + first.height as i32;
    for r in rects.iter().skip(1) {
        min_x = min_x.min(r.x);
        min_y = min_y.min(r.y);
        max_x = max_x.max(r.x + r.width as i32);
        max_y = max_y.max(r.y + r.height as i32);
    }
    Some(OutputRect {
        x: min_x,
        y: min_y,
        width: (max_x - min_x).max(0) as u32,
        height: (max_y - min_y).max(0) as u32,
    })
}

fn cover_crop(img_w: f64, img_h: f64, dest_w: f64, dest_h: f64) -> (f64, f64, f64, f64) {
    let scale = (dest_w / img_w).max(dest_h / img_h);
    let scaled_w = img_w * scale;
    let scaled_h = img_h * scale;
    let ox = (scaled_w - dest_w) / 2.0;
    let oy = (scaled_h - dest_h) / 2.0;
    (ox / scale, oy / scale, dest_w / scale, dest_h / scale)
}

fn contain_rect(img_w: f64, img_h: f64, dest_w: f64, dest_h: f64) -> (f64, f64, f64, f64) {
    let scale = (dest_w / img_w).min(dest_h / img_h);
    let dw = img_w * scale;
    let dh = img_h * scale;
    ((dest_w - dw) / 2.0, (dest_h - dh) / 2.0, dw, dh)
}

fn sample_rect(
    image: &RgbaImage,
    canvas: &mut [u8],
    dest_w: u32,
    dest_h: u32,
    dx: i32,
    dy: i32,
    dw: u32,
    dh: u32,
    sx: f64,
    sy: f64,
    sw: f64,
    sh: f64,
) {
    if dw == 0 || dh == 0 || sw <= 0.0 || sh <= 0.0 {
        return;
    }
    let img_w = image.width();
    let img_h = image.height();
    if img_w == 0 || img_h == 0 {
        return;
    }
    let raw = image.as_raw();
    let max_x = (img_w - 1) as f64;
    let max_y = (img_h - 1) as f64;
    let last_x = img_w - 1;
    let last_y = img_h - 1;
    let dest_w_i = dest_w as i32;
    let dest_h_i = dest_h as i32;
    let row_stride = img_w as usize * 4;
    for y in 0..dh {
        let py = dy + y as i32;
        if py < 0 || py >= dest_h_i {
            continue;
        }
        // Texel-center mapping + bilinear; avoids jagged edges from nearest.
        let fv = (sy + (y as f64 + 0.5) * sh / dh as f64 - 0.5).clamp(0.0, max_y);
        let iy0 = fv.floor() as u32;
        let iy1 = (iy0 + 1).min(last_y);
        let ty = fv - iy0 as f64;
        let row0 = iy0 as usize * row_stride;
        let row1 = iy1 as usize * row_stride;
        let di_row = (py as u32 * dest_w) as usize * 4;
        for x in 0..dw {
            let px = dx + x as i32;
            if px < 0 || px >= dest_w_i {
                continue;
            }
            let fu = (sx + (x as f64 + 0.5) * sw / dw as f64 - 0.5).clamp(0.0, max_x);
            let ix0 = fu.floor() as u32;
            let ix1 = if ix0 < last_x { ix0 + 1 } else { last_x };
            let tx = fu - ix0 as f64;
            let i00 = row0 + ix0 as usize * 4;
            let i10 = row0 + ix1 as usize * 4;
            let i01 = row1 + ix0 as usize * 4;
            let i11 = row1 + ix1 as usize * 4;
            let di = di_row + px as usize * 4;
            let w00 = (1.0 - tx) * (1.0 - ty);
            let w10 = tx * (1.0 - ty);
            let w01 = (1.0 - tx) * ty;
            let w11 = tx * ty;
            let r = raw[i00] as f64 * w00
                + raw[i10] as f64 * w10
                + raw[i01] as f64 * w01
                + raw[i11] as f64 * w11;
            let g = raw[i00 + 1] as f64 * w00
                + raw[i10 + 1] as f64 * w10
                + raw[i01 + 1] as f64 * w01
                + raw[i11 + 1] as f64 * w11;
            let b = raw[i00 + 2] as f64 * w00
                + raw[i10 + 2] as f64 * w10
                + raw[i01 + 2] as f64 * w01
                + raw[i11 + 2] as f64 * w11;
            canvas[di] = b.round().clamp(0.0, 255.0) as u8;
            canvas[di + 1] = g.round().clamp(0.0, 255.0) as u8;
            canvas[di + 2] = r.round().clamp(0.0, 255.0) as u8;
            canvas[di + 3] = 255;
        }
    }
}

fn write_bgra(canvas: &mut [u8], dest_w: u32, dest_h: u32, dx: i32, dy: i32, src: &RgbaImage) {
    if dest_w == 0 || dest_h == 0 {
        return;
    }
    let src_w = src.width() as i32;
    let src_h = src.height() as i32;
    let raw = src.as_raw();
    let dst_x0 = dx.max(0);
    let dst_y0 = dy.max(0);
    let dst_x1 = (dx + src_w).min(dest_w as i32);
    let dst_y1 = (dy + src_h).min(dest_h as i32);
    if dst_x1 <= dst_x0 || dst_y1 <= dst_y0 {
        return;
    }
    let src_x0 = (dst_x0 - dx) as u32;
    let src_y0 = (dst_y0 - dy) as u32;
    let cols = (dst_x1 - dst_x0) as usize;
    for y in 0..(dst_y1 - dst_y0) as u32 {
        let si0 = (((src_y0 + y) * src.width() + src_x0) * 4) as usize;
        let di0 = (((dst_y0 as u32 + y) * dest_w + dst_x0 as u32) * 4) as usize;
        for c in 0..cols {
            let si = si0 + c * 4;
            let di = di0 + c * 4;
            canvas[di] = raw[si + 2];
            canvas[di + 1] = raw[si + 1];
            canvas[di + 2] = raw[si];
            canvas[di + 3] = 255;
        }
    }
}

#[cfg(test)]
fn pixel_argb(canvas: &[u8], dest_w: u32, x: u32, y: u32) -> u32 {
    let i = ((y * dest_w + x) * 4) as usize;
    let b = canvas[i] as u32;
    let g = canvas[i + 1] as u32;
    let r = canvas[i + 2] as u32;
    let a = canvas[i + 3] as u32;
    (a << 24) | (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for p in img.pixels_mut() {
            *p = Rgba([r, g, b, 255]);
        }
        img
    }

    fn split_h(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < w / 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
        }
        img
    }

    #[test]
    fn fill_crops_wide_image() {
        let img = split_h(100, 50);
        let out = render(
            &img,
            WallpaperMode::Fill,
            50,
            50,
            OutputRect {
                x: 0,
                y: 0,
                width: 50,
                height: 50,
            },
            &[],
            1,
        );
        // Center of dest should be near the image midline (red/blue boundary around x=50).
        let left = pixel_argb(&out, 50, 5, 25);
        let right = pixel_argb(&out, 50, 45, 25);
        assert!(left & 0xFF0000 > 0x800000, "left {left:#010x}");
        assert!(right & 0x0000FF > 0x000080, "right {right:#010x}");
    }

    #[test]
    fn fit_letterboxes_wide_image() {
        let img = solid(100, 50, 0, 255, 0);
        let out = render(
            &img,
            WallpaperMode::Fit,
            50,
            50,
            OutputRect {
                x: 0,
                y: 0,
                width: 50,
                height: 50,
            },
            &[],
            1,
        );
        let top = pixel_argb(&out, 50, 25, 2);
        let mid = pixel_argb(&out, 50, 25, 25);
        assert_eq!(top & 0x00FFFFFF, 0);
        assert!(mid & 0x00FF00 > 0x008000, "mid {mid:#010x}");
    }

    #[test]
    fn center_keeps_native_pixels() {
        let img = solid(10, 10, 255, 0, 0);
        let out = render(
            &img,
            WallpaperMode::Center,
            30,
            30,
            OutputRect {
                x: 0,
                y: 0,
                width: 30,
                height: 30,
            },
            &[],
            1,
        );
        assert_eq!(pixel_argb(&out, 30, 0, 0) & 0x00FFFFFF, 0);
        assert_eq!(pixel_argb(&out, 30, 15, 15) & 0x00FFFFFF, 0xFF0000);
    }

    #[test]
    fn tile_repeats() {
        let img = solid(8, 8, 0, 0, 255);
        let out = render(
            &img,
            WallpaperMode::Tile,
            20,
            12,
            OutputRect {
                x: 0,
                y: 0,
                width: 20,
                height: 12,
            },
            &[],
            1,
        );
        assert_eq!(pixel_argb(&out, 20, 0, 0) & 0x00FFFFFF, 0x0000FF);
        assert_eq!(pixel_argb(&out, 20, 19, 11) & 0x00FFFFFF, 0x0000FF);
    }

    #[test]
    fn span_splits_across_two_outputs() {
        let img = split_h(200, 100);
        let left = OutputRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let right = OutputRect {
            x: 100,
            y: 0,
            width: 100,
            height: 100,
        };
        let all = [left, right];
        let a = render(&img, WallpaperMode::Span, 100, 100, left, &all, 1);
        let b = render(&img, WallpaperMode::Span, 100, 100, right, &all, 1);
        let left_px = pixel_argb(&a, 100, 20, 50);
        let right_px = pixel_argb(&b, 100, 80, 50);
        assert!(left_px & 0xFF0000 > 0x800000, "left {left_px:#010x}");
        assert!(right_px & 0x0000FF > 0x000080, "right {right_px:#010x}");
    }

    #[test]
    fn cover_crop_is_centered() {
        let (x, y, w, h) = cover_crop(100.0, 50.0, 50.0, 50.0);
        assert!((x - 25.0).abs() < 0.01, "{x}");
        assert!(y.abs() < 0.01, "{y}");
        assert!((w - 50.0).abs() < 0.01);
        assert!((h - 50.0).abs() < 0.01);
    }
}
