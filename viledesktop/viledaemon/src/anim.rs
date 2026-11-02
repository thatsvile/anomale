use vile_protocol::{display_width, fit_cells, flatten_cells, CellStyle};

use crate::layout::ComposedLine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideDir {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Out,
    In,
    Cross,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Full,
    Side { start: usize, width: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenLine {
    pub text: String,
    pub cells: Vec<CellStyle>,
    pub cols: usize,
}

impl FrozenLine {
    pub fn from_composed(line: &ComposedLine) -> Self {
        let cols = display_width(&line.text);
        Self {
            text: line.text.clone(),
            cells: line.cells.clone(),
            cols,
        }
    }
}

pub fn shift_cells(line: &FrozenLine, dx: i32) -> FrozenLine {
    let cols = line.cols.max(display_width(&line.text));
    let glyphs = flatten_cells(&fit_cells(&line.text, cols));
    let mut slots = vec![String::new(); cols];
    for (i, glyph) in glyphs.iter().enumerate() {
        if i >= cols {
            break;
        }
        let dest = i as i32 + dx;
        if dest >= 0 && (dest as usize) < cols {
            slots[dest as usize] = glyph.clone();
        }
    }
    for slot in slots.iter_mut() {
        if slot.is_empty() {
            *slot = " ".into();
        }
    }
    let text = slots.concat();
    let cells = shift_styles(&line.cells, dx, cols as u32);
    FrozenLine { text, cells, cols }
}

fn shift_styles(cells: &[CellStyle], dx: i32, cols: u32) -> Vec<CellStyle> {
    cells
        .iter()
        .filter_map(|cell| {
            let col = cell.col as i32 + dx;
            if col >= 0 && col < cols as i32 {
                Some(CellStyle {
                    col: col as u32,
                    fg: cell.fg,
                    bg: cell.bg,
                })
            } else {
                None
            }
        })
        .collect()
}

pub fn offset_for_frame(cols: usize, frame: u8, total: u8) -> usize {
    let total = total.max(1) as usize;
    let frame = (frame as usize + 1).min(total);
    (cols * frame) / total
}

/// Two-phase: Out slides `from` off in `dir`; In slides `to` in from the other side.
pub fn two_phase_frame(
    from: &FrozenLine,
    to: &FrozenLine,
    dir: SlideDir,
    phase: Phase,
    frame: u8,
    total: u8,
) -> FrozenLine {
    match phase {
        Phase::Out => {
            let dx = offset_for_frame(from.cols, frame, total) as i32;
            let dx = if dir == SlideDir::Left { -dx } else { dx };
            apply_region(from, from, &shift_cells(from, dx), dir)
        }
        Phase::In => {
            let gone = offset_for_frame(to.cols, frame, total);
            let remain = to.cols.saturating_sub(gone);
            let dx = if dir == SlideDir::Left {
                remain as i32
            } else {
                -(remain as i32)
            };
            apply_region(to, to, &shift_cells(to, dx), dir)
        }
        Phase::Cross => cross_frame(from, to, dir, frame, total),
    }
}

/// Cross-slide: `dir` is the direction the *outgoing* line travels.
/// Right-key (next) => outgoing goes Left, incoming comes from the right.
pub fn cross_frame(
    from: &FrozenLine,
    to: &FrozenLine,
    dir: SlideDir,
    frame: u8,
    total: u8,
) -> FrozenLine {
    let cols = from.cols.max(to.cols);
    let offset = offset_for_frame(cols, frame, total).min(cols);
    let glyphs_from = flatten_cells(&fit_cells(&from.text, cols));
    let glyphs_to = flatten_cells(&fit_cells(&to.text, cols));
    let mut slots = vec![String::from(" "); cols];
    for i in 0..cols {
        let (src, glyphs) = match dir {
            SlideDir::Left => {
                if i + offset < cols {
                    (i + offset, &glyphs_from)
                } else {
                    (i + offset - cols, &glyphs_to)
                }
            }
            SlideDir::Right => {
                if i >= offset {
                    (i - offset, &glyphs_from)
                } else {
                    (i + cols - offset, &glyphs_to)
                }
            }
        };
        if let Some(glyph) = glyphs.get(src) {
            if !glyph.is_empty() {
                slots[i] = glyph.clone();
            }
        }
    }
    let text = slots.concat();
    let cells = match dir {
        SlideDir::Left => {
            let mut out = shift_styles(&from.cells, -(offset as i32), cols as u32);
            out.extend(shift_styles(
                &to.cells,
                (cols.saturating_sub(offset)) as i32,
                cols as u32,
            ));
            out
        }
        SlideDir::Right => {
            let mut out = shift_styles(&from.cells, offset as i32, cols as u32);
            out.extend(shift_styles(
                &to.cells,
                -((cols.saturating_sub(offset)) as i32),
                cols as u32,
            ));
            out
        }
    };
    FrozenLine { text, cells, cols }
}

fn apply_region(
    base: &FrozenLine,
    _full: &FrozenLine,
    moving: &FrozenLine,
    _dir: SlideDir,
) -> FrozenLine {
    let _ = _full;
    FrozenLine {
        text: moving.text.clone(),
        cells: moving.cells.clone(),
        cols: base.cols.max(moving.cols),
    }
}

pub fn splice_region(
    base: &FrozenLine,
    piece: &FrozenLine,
    start: usize,
    width: usize,
) -> FrozenLine {
    let cols = base.cols.max(display_width(&base.text));
    let width = width.min(cols.saturating_sub(start));
    if width == 0 {
        return base.clone();
    }
    let mut slots = flatten_cells(&fit_cells(&base.text, cols));
    slots.resize(cols, " ".into());
    let piece_slots = flatten_cells(&fit_cells(&piece.text, width));
    for (i, glyph) in piece_slots.iter().enumerate() {
        if start + i < cols {
            slots[start + i] = if glyph.is_empty() {
                " ".into()
            } else {
                glyph.clone()
            };
        }
    }
    let text = slots.concat();
    let mut cells: Vec<CellStyle> = base
        .cells
        .iter()
        .filter(|cell| (cell.col as usize) < start || (cell.col as usize) >= start + width)
        .cloned()
        .collect();
    for cell in &piece.cells {
        let col = start as u32 + cell.col;
        if (cell.col as usize) < width && (col as usize) < cols {
            cells.push(CellStyle {
                col,
                fg: cell.fg,
                bg: cell.bg,
            });
        }
    }
    FrozenLine { text, cells, cols }
}

pub fn side_phase_frame(
    from: &FrozenLine,
    to: &FrozenLine,
    start: usize,
    width: usize,
    dir: SlideDir,
    phase: Phase,
    frame: u8,
    total: u8,
) -> FrozenLine {
    let width = width.max(1);
    let extract = |line: &FrozenLine| {
        let glyphs = flatten_cells(&fit_cells(&line.text, line.cols.max(start + width)));
        let mut slots = vec![String::from(" "); width];
        for i in 0..width {
            if let Some(glyph) = glyphs.get(start + i) {
                if !glyph.is_empty() {
                    slots[i] = glyph.clone();
                }
            }
        }
        let cells = line
            .cells
            .iter()
            .filter(|cell| (cell.col as usize) >= start && (cell.col as usize) < start + width)
            .map(|cell| CellStyle {
                col: cell.col - start as u32,
                fg: cell.fg,
                bg: cell.bg,
            })
            .collect();
        FrozenLine {
            text: slots.concat(),
            cells,
            cols: width,
        }
    };
    let from_side = extract(from);
    let to_side = extract(to);
    let moved = match phase {
        Phase::Cross => cross_frame(&from_side, &to_side, dir, frame, total),
        Phase::Out => two_phase_frame(&from_side, &to_side, dir, Phase::Out, frame, total),
        Phase::In => {
            let incoming = match dir {
                SlideDir::Left => SlideDir::Right,
                SlideDir::Right => SlideDir::Left,
            };
            two_phase_frame(&from_side, &to_side, incoming, Phase::In, frame, total)
        }
    };
    let base = match phase {
        Phase::Out => from,
        _ => to,
    };
    splice_region(base, &moved, start, width)
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub from: FrozenLine,
    pub to: FrozenLine,
    pub dir: SlideDir,
    pub phase: Phase,
    pub frame: u8,
    pub total: u8,
    pub region: Region,
    pub frame_ms: u64,
}

impl Transition {
    pub fn current(&self) -> FrozenLine {
        match self.region {
            Region::Full => two_phase_frame(
                &self.from, &self.to, self.dir, self.phase, self.frame, self.total,
            ),
            Region::Side { start, width } => side_phase_frame(
                &self.from, &self.to, start, width, self.dir, self.phase, self.frame, self.total,
            ),
        }
    }

    pub fn advance(&mut self) -> bool {
        self.frame = self.frame.saturating_add(1);
        if self.frame < self.total {
            return true;
        }
        match self.phase {
            Phase::Out => {
                self.phase = Phase::In;
                self.frame = 0;
                true
            }
            Phase::In | Phase::Cross => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vile_protocol::fit_cells;

    fn line(text: &str, cols: usize) -> FrozenLine {
        FrozenLine {
            text: fit_cells(text, cols),
            cells: Vec::new(),
            cols,
        }
    }

    #[test]
    fn shift_cells_moves_content_and_pads() {
        let src = line("ABCD", 4);
        let left = shift_cells(&src, -2);
        assert_eq!(display_width(&left.text), 4);
        assert!(left.text.starts_with("CD"), "{}", left.text);
        assert!(left.text.ends_with("  "), "{}", left.text);
        let right = shift_cells(&src, 2);
        assert!(right.text.starts_with("  "), "{}", right.text);
        assert!(right.text.ends_with("AB"), "{}", right.text);
    }

    #[test]
    fn two_phase_last_out_frame_is_blank() {
        let from = line("HELLO", 5);
        let to = line("WORLD", 5);
        let out = two_phase_frame(&from, &to, SlideDir::Left, Phase::Out, 1, 2);
        assert!(
            out.text.chars().all(|c| c == ' '),
            "expected blank, got {:?}",
            out.text
        );
        let inn = two_phase_frame(&from, &to, SlideDir::Left, Phase::In, 1, 2);
        assert!(inn.text.contains("WORLD"), "{}", inn.text);
    }

    #[test]
    fn cross_slide_starts_with_from_and_ends_with_to() {
        let from = line("AAAA", 4);
        let to = line("BBBB", 4);
        let mid = cross_frame(&from, &to, SlideDir::Left, 0, 2);
        assert_eq!(display_width(&mid.text), 4);
        let end = cross_frame(&from, &to, SlideDir::Left, 1, 2);
        assert!(end.text.contains("BBBB"), "{}", end.text);
    }

    #[test]
    fn side_splice_keeps_other_columns() {
        let from = line("LEFT----RIGHT", 13);
        let to = line("LEFT----TOAST", 13);
        let frame = side_phase_frame(&from, &to, 8, 5, SlideDir::Right, Phase::In, 1, 2);
        assert!(frame.text.starts_with("LEFT"), "{}", frame.text);
        assert!(frame.text.contains("TOAST"), "{}", frame.text);
    }
}
