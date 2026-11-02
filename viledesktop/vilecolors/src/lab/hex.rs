use ratatui::style::Color;

pub fn parse(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ))
}

pub fn rgb(hex: &str) -> Color {
    match parse(hex) {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => Color::Reset,
    }
}

pub fn contrast_fg(hex: &str) -> Color {
    let Some((r, g, b)) = parse(hex) else {
        return Color::White;
    };
    let lin = |c: u8| {
        let x = f64::from(c) / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let y = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    if y > 0.4 {
        Color::Indexed(0)
    } else {
        Color::Indexed(7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(parse("#AABBCC"), Some((0xAA, 0xBB, 0xCC)));
        assert_eq!(parse("112233"), Some((0x11, 0x22, 0x33)));
        assert!(parse("#fff").is_none());
    }
}
