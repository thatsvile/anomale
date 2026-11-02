use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vile_protocol::{ranges_overlap, BarEdges, CellStyle};

pub struct FontEngine {
    font_system: FontSystem,
    swash: SwashCache,
}

impl FontEngine {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
        }
    }

    pub fn cell_width(&mut self, font: &str, font_size: f32, line_height: f32) -> f32 {
        let attrs = attrs(font);
        let font_size = font_size.max(1.0);
        let line_height = line_height.max(1.0);
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(font_size, line_height));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(Some(4096.0), Some(line_height));
        buffer.set_text("M", &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        for run in buffer.layout_runs() {
            if let Some(glyph) = run.glyphs.first() {
                if glyph.w > 0.0 {
                    return glyph.w;
                }
            }
        }
        font_size * 0.6
    }

    pub fn paint(
        &mut self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        font: &str,
        font_size: f32,
        cell_w: f32,
        origin_x: f32,
        origin_y: f32,
        fg: u32,
        bg: u32,
        cells: &[CellStyle],
        dirty: &[(usize, usize)],
        full: bool,
        border_px: u32,
        radius_px: u32,
        edges: BarEdges,
    ) {
        if dirty.is_empty() {
            return;
        }
        let font_size = font_size.max(1.0);
        let clip_y0 = origin_y.round().clamp(0.0, height as f32) as i32;
        let clip_y1 = (origin_y + font_size).round().clamp(0.0, height as f32) as i32;
        if full {
            fill_argb(canvas, bg);
            for cell in cells {
                fill_columns(
                    canvas,
                    width,
                    height,
                    cell_w,
                    origin_x,
                    clip_y0,
                    clip_y1,
                    cell.col as usize,
                    cell.col as usize + 1,
                    cell.bg,
                );
            }
        } else {
            for &(start, end) in dirty {
                for col in start..end {
                    fill_columns(
                        canvas,
                        width,
                        height,
                        cell_w,
                        origin_x,
                        clip_y0,
                        clip_y1,
                        col,
                        col + 1,
                        cell_bg(cells, col as u32, bg),
                    );
                }
            }
        }

        let attrs = attrs(font);
        let mut col = 0usize;
        for grapheme in text.graphemes(true) {
            let gw = UnicodeWidthStr::width(grapheme);
            if gw == 0 {
                continue;
            }
            let end = col + gw;
            if full || ranges_overlap(col, end, dirty) {
                let x = origin_x + col as f32 * cell_w;
                let color = argb_to_color(cell_fg(cells, col as u32, fg));
                self.draw_grapheme(
                    canvas, width, height, grapheme, &attrs, font_size, cell_w, x, origin_y,
                    clip_y0, clip_y1, color,
                );
            }
            col = end;
        }
        finish_chrome(canvas, width, height, border_px, radius_px, edges, fg);
    }

    fn draw_grapheme(
        &mut self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        grapheme: &str,
        attrs: &Attrs<'_>,
        font_size: f32,
        cell_w: f32,
        x: f32,
        y: f32,
        clip_y0: i32,
        clip_y1: i32,
        fg: Color,
    ) {
        let box_w = (cell_w * UnicodeWidthStr::width(grapheme) as f32).max(1.0);
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(font_size, font_size));
        buffer.set_wrap(Wrap::None);
        buffer.set_monospace_width(Some(cell_w));
        buffer.set_size(Some(box_w), Some(font_size));
        buffer.set_text(grapheme, attrs, Shaping::Advanced, None);
        let ox = x.round() as i32;
        let oy = y.round() as i32;
        buffer.draw(
            &mut self.font_system,
            &mut self.swash,
            fg,
            |gx, gy, w, h, color| {
                blit_rect(
                    canvas,
                    width,
                    height,
                    gx + ox,
                    gy + oy,
                    w,
                    h,
                    clip_y0,
                    clip_y1,
                    color,
                );
            },
        );
    }
}

pub fn cell_damage_rect(
    start: usize,
    end: usize,
    cell_w: f32,
    origin_x: f32,
    buf_w: u32,
    buf_h: u32,
) -> (i32, i32, i32, i32) {
    let mut x0 = origin_x + start as f32 * cell_w;
    let mut x1 = origin_x + end as f32 * cell_w;
    x0 = (x0 - 1.0).max(origin_x).clamp(0.0, buf_w as f32);
    x1 = (x1 + 1.0).clamp(0.0, buf_w as f32);
    let w = (x1.round() as i32 - x0.round() as i32).max(0);
    (x0.round() as i32, 0, w, buf_h as i32)
}

fn cell_fg(cells: &[CellStyle], col: u32, fallback: u32) -> u32 {
    cells
        .iter()
        .rev()
        .find(|cell| cell.col == col)
        .map(|cell| cell.fg)
        .unwrap_or(fallback)
}

fn cell_bg(cells: &[CellStyle], col: u32, fallback: u32) -> u32 {
    cells
        .iter()
        .rev()
        .find(|cell| cell.col == col)
        .map(|cell| cell.bg)
        .unwrap_or(fallback)
}

fn attrs(font: &str) -> Attrs<'_> {
    if font.eq_ignore_ascii_case("monospace") {
        Attrs::new().family(Family::Monospace)
    } else {
        Attrs::new().family(Family::Name(font))
    }
}

fn argb_to_color(argb: u32) -> Color {
    Color::rgba(
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
        ((argb >> 24) & 0xFF) as u8,
    )
}

fn finish_chrome(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    thickness: u32,
    radius: u32,
    edges: BarEdges,
    argb: u32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let t = if thickness == 0 {
        0
    } else {
        thickness.min(width / 2).min(height / 2)
    };
    let r = radius.min(width / 2).min(height / 2);
    stroke_border(canvas, width, height, t, r, edges, argb);
    if r > 0 {
        if edges.round_tl() {
            punch_corner(canvas, width, height, Corner::Tl, r, t, argb);
        }
        if edges.round_tr() {
            punch_corner(canvas, width, height, Corner::Tr, r, t, argb);
        }
        if edges.round_bl() {
            punch_corner(canvas, width, height, Corner::Bl, r, t, argb);
        }
        if edges.round_br() {
            punch_corner(canvas, width, height, Corner::Br, r, t, argb);
        }
    }
}

fn stroke_border(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    t: u32,
    radius: u32,
    edges: BarEdges,
    argb: u32,
) {
    if t == 0 || !edges.any() {
        return;
    }
    let r = if radius > 0 { radius } else { 0 };
    let tl = if edges.round_tl() { r } else { 0 };
    let tr = if edges.round_tr() { r } else { 0 };
    let bl = if edges.round_bl() { r } else { 0 };
    let br = if edges.round_br() { r } else { 0 };
    if edges.top {
        let x0 = tl;
        let x1 = width.saturating_sub(tr);
        if x1 > x0 {
            fill_rect(canvas, width, height, x0, 0, x1 - x0, t, argb);
        }
    }
    if edges.bottom {
        let x0 = bl;
        let x1 = width.saturating_sub(br);
        if x1 > x0 {
            fill_rect(
                canvas,
                width,
                height,
                x0,
                height.saturating_sub(t),
                x1 - x0,
                t,
                argb,
            );
        }
    }
    if edges.left {
        let y0 = tl;
        let y1 = height.saturating_sub(bl);
        if y1 > y0 {
            fill_rect(canvas, width, height, 0, y0, t, y1 - y0, argb);
        }
    }
    if edges.right {
        let y0 = tr;
        let y1 = height.saturating_sub(br);
        if y1 > y0 {
            fill_rect(
                canvas,
                width,
                height,
                width.saturating_sub(t),
                y0,
                t,
                y1 - y0,
                argb,
            );
        }
    }
}

#[derive(Clone, Copy)]
enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

fn punch_corner(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    corner: Corner,
    radius: u32,
    thickness: u32,
    argb: u32,
) {
    let r = radius.min(width / 2).min(height / 2);
    if r == 0 {
        return;
    }
    let r2 = (r as i32) * (r as i32);
    let inner = r.saturating_sub(thickness);
    let inner2 = (inner as i32) * (inner as i32);
    let (x0, y0) = match corner {
        Corner::Tl => (0, 0),
        Corner::Tr => (width.saturating_sub(r), 0),
        Corner::Bl => (0, height.saturating_sub(r)),
        Corner::Br => (width.saturating_sub(r), height.saturating_sub(r)),
    };
    for y in y0..y0 + r {
        for x in x0..x0 + r {
            let dx = match corner {
                Corner::Tl | Corner::Bl => r as i32 - x as i32,
                Corner::Tr | Corner::Br => x as i32 + r as i32 + 1 - width as i32,
            };
            let dy = match corner {
                Corner::Tl | Corner::Tr => r as i32 - y as i32,
                Corner::Bl | Corner::Br => y as i32 + r as i32 + 1 - height as i32,
            };
            if dx <= 0 || dy <= 0 {
                continue;
            }
            let d2 = dx * dx + dy * dy;
            if d2 > r2 {
                set_pixel(canvas, width, height, x, y, 0);
            } else if thickness > 0 && d2 > inner2 {
                set_pixel(canvas, width, height, x, y, argb);
            }
        }
    }
}

#[cfg(test)]
fn pixel_alpha(canvas: &[u8], width: u32, x: u32, y: u32) -> u8 {
    let i = ((y * width + x) * 4) as usize;
    canvas.get(i + 3).copied().unwrap_or(0)
}

fn set_pixel(canvas: &mut [u8], width: u32, height: u32, x: u32, y: u32, argb: u32) {
    if x >= width || y >= height {
        return;
    }
    let i = ((y * width + x) * 4) as usize;
    if i + 3 >= canvas.len() {
        return;
    }
    canvas[i] = (argb & 0xFF) as u8;
    canvas[i + 1] = ((argb >> 8) & 0xFF) as u8;
    canvas[i + 2] = ((argb >> 16) & 0xFF) as u8;
    canvas[i + 3] = ((argb >> 24) & 0xFF) as u8;
}

fn fill_argb(canvas: &mut [u8], argb: u32) {
    let a = ((argb >> 24) & 0xFF) as u8;
    let r = ((argb >> 16) & 0xFF) as u8;
    let g = ((argb >> 8) & 0xFF) as u8;
    let b = (argb & 0xFF) as u8;
    for chunk in canvas.chunks_exact_mut(4) {
        chunk[0] = b;
        chunk[1] = g;
        chunk[2] = r;
        chunk[3] = a;
    }
}

fn fill_columns(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    cell_w: f32,
    origin_x: f32,
    clip_y0: i32,
    clip_y1: i32,
    start: usize,
    end: usize,
    argb: u32,
) {
    let mut x0 = origin_x + start as f32 * cell_w;
    let mut x1 = origin_x + end as f32 * cell_w;
    x0 = (x0 - 1.0).max(origin_x).clamp(0.0, width as f32);
    x1 = (x1 + 1.0).clamp(0.0, width as f32);
    let x0 = x0.round() as u32;
    let x1 = x1.round() as u32;
    if x1 <= x0 {
        return;
    }
    let y0 = clip_y0.clamp(0, height as i32) as u32;
    let y1 = clip_y1.clamp(0, height as i32) as u32;
    if y1 <= y0 {
        return;
    }
    fill_rect(canvas, width, height, x0, y0, x1 - x0, y1 - y0, argb);
}

fn fill_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    argb: u32,
) {
    if w == 0 || h == 0 {
        return;
    }
    let a = ((argb >> 24) & 0xFF) as u8;
    let r = ((argb >> 16) & 0xFF) as u8;
    let g = ((argb >> 8) & 0xFF) as u8;
    let b = (argb & 0xFF) as u8;
    let x1 = (x + w).min(width);
    let y1 = (y + h).min(height);
    for py in y..y1 {
        let row = (py * width) as usize * 4;
        for px in x..x1 {
            let i = row + px as usize * 4;
            canvas[i] = b;
            canvas[i + 1] = g;
            canvas[i + 2] = r;
            canvas[i + 3] = a;
        }
    }
}

fn blit_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    clip_y0: i32,
    clip_y1: i32,
    color: Color,
) {
    let src_a = color.a() as u32;
    if src_a == 0 || w == 0 || h == 0 {
        return;
    }
    let src_r = color.r() as u32;
    let src_g = color.g() as u32;
    let src_b = color.b() as u32;
    let y0 = clip_y0.max(0);
    let y1 = clip_y1.min(height as i32);
    for row in 0..h as i32 {
        let py = y + row;
        if py < y0 || py >= y1 {
            continue;
        }
        for col in 0..w as i32 {
            let px = x + col;
            if px < 0 || px >= width as i32 {
                continue;
            }
            let i = ((py as u32 * width + px as u32) * 4) as usize;
            if src_a == 255 {
                canvas[i] = src_b as u8;
                canvas[i + 1] = src_g as u8;
                canvas[i + 2] = src_r as u8;
                canvas[i + 3] = 255;
            } else {
                let inv = 255 - src_a;
                canvas[i] = ((src_b * src_a + canvas[i] as u32 * inv) / 255) as u8;
                canvas[i + 1] = ((src_g * src_a + canvas[i + 1] as u32 * inv) / 255) as u8;
                canvas[i + 2] = ((src_r * src_a + canvas[i + 2] as u32 * inv) / 255) as u8;
                canvas[i + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{finish_chrome, pixel_alpha};
    use vile_protocol::BarEdges;

    fn canvas(w: u32, h: u32, argb: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                super::set_pixel(&mut out, w, h, x, y, argb);
            }
        }
        out
    }

    #[test]
    fn punch_makes_outer_pixel_transparent() {
        let mut buf = canvas(16, 16, 0xFF1A1A1A);
        let edges = BarEdges {
            top: true,
            right: true,
            bottom: true,
            left: true,
        };
        finish_chrome(&mut buf, 16, 16, 1, 6, edges, 0xFFE0E0E0);
        assert_eq!(pixel_alpha(&buf, 16, 0, 0), 0);
        assert_eq!(pixel_alpha(&buf, 16, 15, 0), 0);
        assert_eq!(pixel_alpha(&buf, 16, 0, 15), 0);
        assert_eq!(pixel_alpha(&buf, 16, 15, 15), 0);
        assert_eq!(pixel_alpha(&buf, 16, 8, 8), 255);
    }

    #[test]
    fn flush_sides_skip_border_and_keep_square() {
        let mut buf = canvas(16, 16, 0xFF1A1A1A);
        let edges = BarEdges {
            top: false,
            right: true,
            bottom: true,
            left: true,
        };
        finish_chrome(&mut buf, 16, 16, 1, 6, edges, 0xFFE0E0E0);
        assert_eq!(pixel_alpha(&buf, 16, 0, 0), 255);
        assert_eq!(pixel_alpha(&buf, 16, 15, 0), 255);
        assert_eq!(pixel_alpha(&buf, 16, 0, 15), 0);
        assert_eq!(pixel_alpha(&buf, 16, 15, 15), 0);
    }

    #[test]
    fn invert_fill_stays_in_text_box() {
        let mut buf = vec![0u8; (8 * 16 * 4) as usize];
        super::fill_columns(&mut buf, 8, 16, 8.0, 0.0, 4, 12, 0, 1, 0xFFFFFFFF);
        assert_eq!(pixel_alpha(&buf, 8, 0, 0), 0);
        assert_eq!(pixel_alpha(&buf, 8, 0, 3), 0);
        assert_eq!(pixel_alpha(&buf, 8, 0, 4), 255);
        assert_eq!(pixel_alpha(&buf, 8, 0, 11), 255);
        assert_eq!(pixel_alpha(&buf, 8, 0, 12), 0);

        let mut buf = vec![0u8; (16 * 16 * 4) as usize];
        super::fill_columns(&mut buf, 16, 16, 4.0, 8.0, 0, 16, 0, 1, 0xFFFFFFFF);
        assert_eq!(pixel_alpha(&buf, 16, 0, 8), 0);
        assert_eq!(pixel_alpha(&buf, 16, 7, 8), 0);
        assert_eq!(pixel_alpha(&buf, 16, 8, 8), 255);
    }
}
