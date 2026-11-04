use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::color::rgb_hsl;
use crate::map::Palette;
use vile_protocol::{templates_dir, wal_templates_dir};

#[derive(Clone)]
struct Color {
    hex: String,
}

impl Color {
    fn from_hex(hex: &str) -> Self {
        Self {
            hex: hex.to_string(),
        }
    }

    fn rgb(&self) -> [u8; 3] {
        let h = self.hex.trim_start_matches('#');
        if h.len() < 6 {
            return [0, 0, 0];
        }
        let parse = |i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
        [parse(0), parse(2), parse(4)]
    }

    fn hsl_parts(&self) -> (i32, i32, i32) {
        let [r, g, b] = self.rgb();
        let [h, s, l] = rgb_hsl(f64::from(r), f64::from(g), f64::from(b));
        (
            h.round().clamp(0.0, 360.0) as i32 % 360,
            (s * 100.0).round().clamp(0.0, 100.0) as i32,
            (l * 100.0).round().clamp(0.0, 100.0) as i32,
        )
    }

    fn apply(&self, mods: &[&str], alpha: &str) -> Result<String> {
        if mods.is_empty() {
            return Ok(self.hex.clone());
        }
        let [r, g, b] = self.rgb();
        let a = alpha.parse::<f64>().unwrap_or(100.0) / 100.0;
        match mods {
            ["strip"] => Ok(self.hex.trim_start_matches('#').to_string()),
            ["rgb"] => Ok(format!("{r},{g},{b}")),
            ["rgba"] => Ok(format!("{r},{g},{b},{a}")),
            ["xrgba"] => {
                let aa = (a * 255.0).round().clamp(0.0, 255.0) as u8;
                Ok(format!("{r:02x}/{g:02x}/{b:02x}/{aa:02x}"))
            }
            ["red"] => Ok(r.to_string()),
            ["green"] => Ok(g.to_string()),
            ["blue"] => Ok(b.to_string()),
            ["hsl"] => {
                let (h, s, l) = self.hsl_parts();
                Ok(format!("{h}, {s}%, {l}%"))
            }
            ["hsl", "h"] => Ok(self.hsl_parts().0.to_string()),
            ["hsl", "s"] => Ok(self.hsl_parts().1.to_string()),
            ["hsl", "l"] => Ok(self.hsl_parts().2.to_string()),
            _ => bail!("unknown template modifier .{}", mods.join(".")),
        }
    }
}

pub fn render(template: &str, palette: &Palette) -> Result<String> {
    let mut colors = BTreeMap::new();
    colors.insert("background", Color::from_hex(&palette.background));
    colors.insert("foreground", Color::from_hex(&palette.foreground));
    colors.insert("cursor", Color::from_hex(&palette.cursor));
    let numbered: Vec<Color> = palette.colors.iter().map(|h| Color::from_hex(h)).collect();
    subst(template, palette, &colors, &numbered)
}

fn subst(
    template: &str,
    palette: &Palette,
    named: &BTreeMap<&str, Color>,
    numbered: &[Color],
) -> Result<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '{' {
            out.push('{');
            i += 2;
            continue;
        }
        if chars[i] == '}' && i + 1 < chars.len() && chars[i + 1] == '}' {
            out.push('}');
            i += 2;
            continue;
        }
        if chars[i] == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '}') {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if inner.contains('{') {
                    out.push('{');
                    i += 1;
                    continue;
                }
                out.push_str(&replace_var(&inner, palette, named, numbered)?);
                i += end + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    Ok(out)
}

/// Vesktop clientTheme HSL from palette `background`, adjusted for Theme base.
/// Hue/sat follow the background; L is forced into a dark (near-black) or light
/// band so Discord does not flip to the wrong overall mode.
fn vesktop_hsl(background: &Color, theme: crate::recipe::Theme) -> (i32, i32, i32) {
    let (h, s, _) = background.hsl_parts();
    match theme {
        // Dark L ~16 keeps clientTheme near-black without crushing chrome to pure black.
        crate::recipe::Theme::Dark => (h, s.clamp(12, 36), 16),
        crate::recipe::Theme::Light => (h, s.clamp(10, 28), 72),
    }
}

fn replace_var(
    inner: &str,
    palette: &Palette,
    named: &BTreeMap<&str, Color>,
    numbered: &[Color],
) -> Result<String> {
    let mut parts = inner.split('.');
    let name = parts.next().unwrap_or("");
    let mods: Vec<&str> = parts.collect();
    if name == "wallpaper" {
        return Ok(palette.wallpaper.clone());
    }
    if name == "alpha" {
        return Ok(palette.alpha.clone());
    }
    if name == "theme" {
        return Ok(match palette.theme {
            crate::recipe::Theme::Dark => "dark".into(),
            crate::recipe::Theme::Light => "light".into(),
        });
    }
    if name == "vesktop" {
        let background = named
            .get("background")
            .cloned()
            .unwrap_or_else(|| Color::from_hex("#000000"));
        let (h, s, l) = vesktop_hsl(&background, palette.theme);
        return Ok(match mods.as_slice() {
            [] | ["hsl"] => format!("{h}, {s}%, {l}%"),
            ["h"] | ["hsl", "h"] => h.to_string(),
            ["s"] | ["hsl", "s"] => s.to_string(),
            ["l"] | ["hsl", "l"] => l.to_string(),
            _ => format!("{{{inner}}}"),
        });
    }
    let color = if let Some(rest) = name.strip_prefix("color") {
        let Some(idx) = rest.parse().ok().filter(|i: &usize| *i < 16) else {
            return Ok(format!("{{{inner}}}"));
        };
        &numbered[idx]
    } else {
        match named.get(name) {
            Some(c) => c,
            None => return Ok(format!("{{{inner}}}")),
        }
    };
    match color.apply(&mods, &palette.alpha) {
        Ok(value) => Ok(value),
        Err(_) => Ok(format!("{{{inner}}}")),
    }
}

pub fn builtin_templates() -> Vec<(&'static str, String)> {
    vec![
        ("wal", "{wallpaper}\n".into()),
        ("colors", colors_list()),
        ("colors.sh", colors_sh()),
        ("colors.json", colors_json()),
        ("colors.Xresources", colors_xresources()),
        ("colors.css", colors_css()),
        ("colors.scss", colors_scss()),
        ("sequences", sequences_template()),
        ("foot", foot_template()),
        ("niri-colors.kdl", niri_colors_template()),
    ]
}

fn colors_list() -> String {
    (0..16).map(|i| format!("{{color{i}}}\n")).collect()
}

fn colors_sh() -> String {
    let mut s = String::from("# Generated by vilecolors\nwallpaper='{wallpaper}'\nbackground='{background}'\nforeground='{foreground}'\ncursor='{cursor}'\n");
    for i in 0..16 {
        s.push_str(&format!("color{i}='{{color{i}}}'\n"));
    }
    s
}

fn colors_json() -> String {
    let mut s = String::from("{\n    \"wallpaper\": \"{wallpaper}\",\n    \"alpha\": \"{alpha}\",\n    \"special\": {\n        \"background\": \"{background}\",\n        \"foreground\": \"{foreground}\",\n        \"cursor\": \"{cursor}\"\n    },\n    \"colors\": {\n");
    for i in 0..16 {
        let comma = if i == 15 { "" } else { "," };
        s.push_str(&format!("        \"color{i}\": \"{{color{i}}}\"{comma}\n"));
    }
    s.push_str("    },\n    \"vile\": {\n        \"theme\": \"{theme}\"\n    }\n}\n");
    s
}

fn colors_xresources() -> String {
    let mut s = String::from("! Generated by vilecolors\n*.foreground: {foreground}\n*.background: {background}\n*.cursorColor: {cursor}\n");
    for i in 0..16 {
        s.push_str(&format!("*.color{i}: {{color{i}}}\n"));
    }
    s
}

fn colors_css() -> String {
    let mut s = String::from(":root {{\n    --wallpaper: url(\"{wallpaper}\");\n    --background: {background};\n    --foreground: {foreground};\n    --cursor: {cursor};\n");
    for i in 0..16 {
        s.push_str(&format!("    --color{i}: {{color{i}}};\n"));
    }
    s.push_str("}}\n");
    s
}

fn colors_scss() -> String {
    let mut s = String::from("$wallpaper: \"{wallpaper}\";\n$background: {background};\n$foreground: {foreground};\n$cursor: {cursor};\n");
    for i in 0..16 {
        s.push_str(&format!("$color{i}: {{color{i}}};\n"));
    }
    s
}

fn foot_template() -> String {
    let mut s = String::from(
        "# Generated by vilecolors\n[colors-dark]\nbackground={background.strip}\nforeground={foreground.strip}\n",
    );
    for i in 0..8 {
        s.push_str(&format!("regular{i}={{color{i}.strip}}\n"));
    }
    for i in 0..8 {
        s.push_str(&format!("bright{i}={{color{}.strip}}\n", i + 8));
    }
    s
}

fn niri_colors_template() -> String {
    String::from(
        r#"// Generated by vilecolors
// color4 = active border (same slot as vile bar foreground)
// color8 = inactive border
// color1 = urgent
// color0 / background = workspace + overview backdrop
layout {
    background-color "{background}"

    border {
        active-color "{color4}"
        inactive-color "{color8}"
        urgent-color "{color1}"
    }

    insert-hint {
        color "{color4}80"
    }

    shadow {
        color "{background}70"
    }
}

overview {
    backdrop-color "{color0}"
}
"#,
    )
}

fn sequences_template() -> String {
    let mut t = String::new();
    for i in 0..16 {
        t.push_str(&format!("\x1b]4;{i};{{color{i}}}\x1b\\"));
    }
    t.push_str("\x1b]10;{foreground}\x1b\\");
    t.push_str("\x1b]11;{background}\x1b\\");
    t.push_str("\x1b]12;{cursor}\x1b\\");
    t.push_str("\x1b]13;{foreground}\x1b\\");
    t.push_str("\x1b]17;{background}\x1b\\");
    t.push_str("\x1b]19;{foreground}\x1b\\");
    t.push_str("\x1b]708;{background}\x1b\\");
    t
}

fn skip_name(name: &str) -> bool {
    name == ".DS_Store" || name.ends_with(".swp") || name.starts_with('.')
}

pub fn collect_templates() -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for (name, body) in builtin_templates() {
        map.insert(name.to_string(), body);
    }
    load_dir(&wal_templates_dir(), &mut map)?;
    load_dir(&templates_dir(), &mut map)?;
    Ok(map)
}

fn load_dir(dir: &Path, map: &mut BTreeMap<String, String>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip_name(&name) {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            map.insert(name, text);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> Palette {
        let mut colors = std::array::from_fn(|i| format!("#{i:02X}{i:02X}{i:02X}"));
        colors[0] = "#112233".into();
        Palette {
            background: "#112233".into(),
            foreground: "#AABBCC".into(),
            cursor: "#AABBCC".into(),
            colors,
            wallpaper: "/tmp/wall.png".into(),
            alpha: "100".into(),
            theme: crate::recipe::Theme::Light,
        }
    }

    #[test]
    fn substitutes_hex_strip_and_escapes() {
        let out = render(
            "bg {background} strip {color0.strip} brace {{ok}} rgb {foreground.rgb}",
            &pal(),
        )
        .unwrap();
        assert_eq!(out, "bg #112233 strip 112233 brace {ok} rgb 170,187,204");
    }

    #[test]
    fn substitutes_hsl_for_clienttheme() {
        // #00FF00 → h=120, s=100%, l=50%
        let mut p = pal();
        p.colors[4] = "#00FF00".into();
        let out = render(
            ":root {{\n    --theme-h: {color4.hsl.h};\n    --theme-s: {color4.hsl.s}%;\n    --theme-l: {color4.hsl.l}%;\n}}\n{color4.hsl}",
            &p,
        )
        .unwrap();
        assert_eq!(
            out,
            ":root {\n    --theme-h: 120;\n    --theme-s: 100%;\n    --theme-l: 50%;\n}\n120, 100%, 50%"
        );
    }

    #[test]
    fn vesktop_hsl_dark_is_near_black() {
        let mut p = pal();
        p.theme = crate::recipe::Theme::Dark;
        // Bright green background → hue kept, S clamped, L forced low.
        p.background = "#00FF00".into();
        let out = render(
            "--theme-h: {vesktop.h}; --theme-s: {vesktop.s}%; --theme-l: {vesktop.l}%;",
            &p,
        )
        .unwrap();
        assert_eq!(out, "--theme-h: 120; --theme-s: 36%; --theme-l: 16%;");
    }

    #[test]
    fn vesktop_hsl_light_is_bright() {
        let mut p = pal();
        p.theme = crate::recipe::Theme::Light;
        p.background = "#00FF00".into();
        let out = render(
            "--theme-h: {vesktop.h}; --theme-s: {vesktop.s}%; --theme-l: {vesktop.l}%;",
            &p,
        )
        .unwrap();
        assert_eq!(out, "--theme-h: 120; --theme-s: 28%; --theme-l: 72%;");
    }

    #[test]
    fn wallpaper_and_alpha() {
        let out = render("{wallpaper} {alpha}", &pal()).unwrap();
        assert_eq!(out, "/tmp/wall.png 100");
    }

    #[test]
    fn colors_json_includes_vile_theme() {
        let out = render(&colors_json(), &pal()).unwrap();
        assert!(out.contains("\"theme\": \"light\""));
        assert!(out.contains("\"color0\": \"#112233\""));
    }

    #[test]
    fn foot_template_uses_stripped_hex() {
        let out = render(&foot_template(), &pal()).unwrap();
        assert!(out.contains("[colors-dark]"));
        assert!(out.contains("background=112233"));
        assert!(out.contains("foreground=AABBCC"));
        assert!(out.contains("regular0=112233"));
        assert!(out.contains("bright7=0F0F0F"));
        assert!(!out.contains("=#"));
    }

    #[test]
    fn niri_template_uses_quoted_hex() {
        let out = render(&niri_colors_template(), &pal()).unwrap();
        assert!(out.contains("background-color \"#112233\""));
        assert!(out.contains("active-color \"#040404\""));
        assert!(out.contains("inactive-color \"#080808\""));
        assert!(out.contains("urgent-color \"#010101\""));
        assert!(out.contains("backdrop-color \"#112233\""));
        assert!(out.contains("color \"#04040480\""));
    }
}
