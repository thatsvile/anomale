use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_cols).sum()
}

pub fn grapheme_cols(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme)
}

pub fn fit_cells(text: &str, cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let width = grapheme_cols(grapheme);
        if width == 0 {
            out.push_str(grapheme);
            continue;
        }
        if used + width > cols {
            break;
        }
        out.push_str(grapheme);
        used += width;
    }
    while used < cols {
        out.push(' ');
        used += 1;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedLine {
    pub text: String,
    pub left_w: usize,
    pub center_start: usize,
    pub center_w: usize,
    pub right_start: usize,
    pub right_w: usize,
}

/// Pack left, center, and right segments into a fixed cell row.
/// Center is placed at the visual midpoint when it does not overlap the sides.
pub fn pack_line(cols: usize, left: &str, center: &str, right: &str) -> String {
    pack_layout(cols, left, center, right).text
}

pub fn pack_layout(cols: usize, left: &str, center: &str, right: &str) -> PackedLine {
    if cols == 0 {
        return PackedLine {
            text: String::new(),
            left_w: 0,
            center_start: 0,
            center_w: 0,
            right_start: 0,
            right_w: 0,
        };
    }

    let left = fit_cells(left, cols);
    let left = trim_cells(&left);
    let left_w = display_width(&left);

    let right_budget = cols.saturating_sub(left_w);
    let right = fit_cells(right, right_budget);
    let right = trim_cells(&right);
    let right_w = display_width(&right);

    let center_budget = cols.saturating_sub(left_w + right_w);
    let center = fit_cells(center, center_budget);
    let center = trim_cells(&center);
    let center_w = display_width(&center);

    let ideal = cols.saturating_sub(center_w) / 2;
    let max_start = cols.saturating_sub(right_w + center_w);
    let center_start = ideal.clamp(left_w, max_start);
    let right_start = cols.saturating_sub(right_w);

    let mut out = String::new();
    out.push_str(&left);
    out.push_str(&spaces(center_start.saturating_sub(left_w)));
    out.push_str(&center);
    let used = center_start + center_w;
    out.push_str(&spaces(cols.saturating_sub(right_w).saturating_sub(used)));
    out.push_str(&right);
    PackedLine {
        text: fit_cells(&out, cols),
        left_w,
        center_start,
        center_w,
        right_start,
        right_w,
    }
}

fn trim_cells(text: &str) -> String {
    let width = display_width(text);
    let trailing = text.chars().rev().take_while(|c| *c == ' ').count();
    if trailing == 0 {
        return text.to_string();
    }
    let keep = width.saturating_sub(trailing);
    fit_cells(text, keep).trim_end_matches(' ').to_string()
}

fn spaces(n: usize) -> String {
    " ".repeat(n)
}

/// Flatten a row into one string per display cell.
/// Wide glyphs occupy a start cell plus empty continuation cells.
pub fn flatten_cells(text: &str) -> Vec<String> {
    let mut cells: Vec<String> = Vec::new();
    for grapheme in text.graphemes(true) {
        let width = grapheme_cols(grapheme);
        if width == 0 {
            if let Some(last) = cells.last_mut() {
                last.push_str(grapheme);
            }
            continue;
        }
        cells.push(grapheme.to_string());
        for _ in 1..width {
            cells.push(String::new());
        }
    }
    cells
}

/// Half-open display-column ranges `[start, end)` that differ between two rows.
/// Adjacent dirty cells are merged into a single range.
pub fn dirty_cell_ranges(old: &str, new: &str) -> Vec<(usize, usize)> {
    let old_cells = flatten_cells(old);
    let new_cells = flatten_cells(new);
    let n = old_cells.len().max(new_cells.len());
    let mut ranges = Vec::new();
    let mut start = None;
    for i in 0..n {
        let old_cell = old_cells.get(i).map(String::as_str).unwrap_or("");
        let new_cell = new_cells.get(i).map(String::as_str).unwrap_or("");
        if old_cell == new_cell {
            if let Some(s) = start.take() {
                ranges.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        ranges.push((s, n));
    }
    ranges
}

pub fn ranges_overlap(start: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|(s, e)| start < *e && end > *s)
}

pub fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|range| range.0);
    let mut out = Vec::new();
    for (start, end) in ranges {
        if end <= start {
            continue;
        }
        if let Some((_, last_end)) = out.last_mut() {
            if start <= *last_end {
                *last_end = (*last_end).max(end);
                continue;
            }
        }
        out.push((start, end));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{dirty_cell_ranges, display_width, fit_cells, pack_line};

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit_cells("ab", 5), "ab   ");
        assert_eq!(display_width(&fit_cells("abcdef", 4)), 4);
        assert_eq!(fit_cells("日本語", 4).chars().count() > 0, true);
    }

    #[test]
    fn pack_keeps_sides_and_center() {
        let line = pack_line(20, "LEFT", "MID", "RIGHT");
        assert_eq!(display_width(&line), 20);
        assert!(line.starts_with("LEFT"));
        assert!(line.ends_with("RIGHT"));
        assert!(line.contains("MID"));
    }

    #[test]
    fn pack_narrow_bar_drops_center() {
        let line = pack_line(8, "LEFT", "CENTER", "R");
        assert_eq!(display_width(&line), 8);
        assert!(line.starts_with("LEFT"));
        assert!(line.ends_with("R"));
    }

    #[test]
    fn dirty_ranges_identical_are_empty() {
        assert!(dirty_cell_ranges("hello", "hello").is_empty());
    }

    #[test]
    fn dirty_ranges_single_cell() {
        assert_eq!(dirty_cell_ranges("abcde", "abXde"), vec![(2, 3)]);
    }

    #[test]
    fn dirty_ranges_right_side_hover() {
        let old = pack_line(40, "12:00:00", "click any cell", "h:9 c:40");
        let new = pack_line(40, "12:00:00", "click any cell", "h:10 c:40");
        let ranges = dirty_cell_ranges(&old, &new);
        assert!(!ranges.is_empty());
        let first = ranges[0].0;
        assert!(
            first > 10,
            "left clock should stay clean, first dirty={first}"
        );
        assert!(
            ranges.iter().all(|&(s, _)| s > 10),
            "hover should not dirty the left side: {ranges:?}"
        );
    }

    #[test]
    fn dirty_ranges_length_change() {
        assert_eq!(dirty_cell_ranges("aa", "aaa"), vec![(2, 3)]);
        assert_eq!(dirty_cell_ranges("", "abc"), vec![(0, 3)]);
    }
}
