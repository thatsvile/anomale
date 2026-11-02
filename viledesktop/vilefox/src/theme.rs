use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
}

pub fn theme_kind(json: &Value) -> Option<ThemeKind> {
    let c0 = json
        .get("colors")
        .and_then(|c| c.get("color0"))
        .and_then(Value::as_str)?;
    Some(match json
        .get("vile")
        .and_then(|v| v.get("theme"))
        .and_then(Value::as_str)
    {
        Some("light") => ThemeKind::Light,
        Some("dark") => ThemeKind::Dark,
        _ => {
            if is_light_hex(c0) {
                ThemeKind::Light
            } else {
                ThemeKind::Dark
            }
        }
    })
}

fn is_light_hex(hex: &str) -> bool {
    let h = hex.trim().trim_start_matches('#');
    if h.len() < 6 {
        return false;
    }
    let parse = |i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32;
    let r = parse(0) / 255.0;
    let g = parse(2) / 255.0;
    let b = parse(4) / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vile_theme_wins() {
        let json = json!({
            "vile": { "theme": "light" },
            "colors": { "color0": "#111111" }
        });
        assert_eq!(theme_kind(&json), Some(ThemeKind::Light));
    }
}
