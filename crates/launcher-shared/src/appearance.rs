//! The launcher's look: its UI font and the accent colour every other colour of the design is
//! derived from, as CSS custom properties the window sets on `<html>`.

use serde::{Deserialize, Serialize};

/// The UI font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFont {
    /// Inter, with Exo 2 headings.
    #[default]
    Standard,
    /// Monocraft everywhere.
    Minecraft,
    Exo,
    Mono,
    /// The system's own UI font.
    System,
}

impl UiFont {
    pub const ALL: [UiFont; 5] =
        [UiFont::Standard, UiFont::Minecraft, UiFont::Exo, UiFont::Mono, UiFont::System];

    pub fn label_key(self) -> &'static str {
        match self {
            UiFont::Standard => "appearance_font_standard",
            UiFont::Minecraft => "appearance_font_minecraft",
            UiFont::Exo => "appearance_font_exo",
            UiFont::Mono => "appearance_font_mono",
            UiFont::System => "appearance_font_system",
        }
    }
}

pub const DEFAULT_ACCENT: &str = "#17c3a2";

/// A ready accent colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPreset {
    pub id: &'static str,
    pub label_key: &'static str,
    pub hex: &'static str,
}

const fn preset(id: &'static str, label_key: &'static str, hex: &'static str) -> AccentPreset {
    AccentPreset { id, label_key, hex }
}

pub const ACCENT_PRESETS: [AccentPreset; 8] = [
    preset("default", "appearance_accent_default", DEFAULT_ACCENT),
    preset("grass", "appearance_accent_grass", "#5bc236"),
    preset("sky", "appearance_accent_sky", "#3d9cf5"),
    preset("ice", "appearance_accent_ice", "#34cfe6"),
    preset("amethyst", "appearance_accent_amethyst", "#9b6cf6"),
    preset("sakura", "appearance_accent_sakura", "#f06aa8"),
    preset("ruby", "appearance_accent_ruby", "#f0506e"),
    preset("amber", "appearance_accent_amber", "#f2a93b"),
];

/// `raw` as a stored accent: trimmed, lowercase `#rrggbb` (`#rgb` expanded); `None` for anything else.
pub fn normalize_accent(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix('#')?.to_ascii_lowercase();
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{hex}")),
        3 => Some(format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>())),
        _ => None,
    }
}

/// The `--f-ui`, `--f-head` and `--f-mono` stacks of `font`.
pub fn font_stacks(font: UiFont) -> (&'static str, &'static str, &'static str) {
    const INTER: &str = "\"Inter\", \"Segoe UI\", system-ui, sans-serif";
    const EXO: &str = "\"Exo 2\", \"Inter\", sans-serif";
    const MONO: &str = "\"JetBrains Mono\", Consolas, monospace";
    const SYSTEM: &str = "\"Segoe UI\", system-ui, -apple-system, \"Ubuntu\", sans-serif";
    match font {
        UiFont::Standard => (INTER, EXO, MONO),
        UiFont::Minecraft => (
            "\"Monocraft\", \"Inter\", sans-serif",
            "\"Monocraft\", \"Inter\", sans-serif",
            "\"Monocraft\", \"JetBrains Mono\", monospace",
        ),
        UiFont::Exo => (EXO, EXO, MONO),
        UiFont::Mono => (MONO, MONO, MONO),
        UiFont::System => (SYSTEM, SYSTEM, MONO),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Hsl {
    h: f64,
    s: f64,
    l: f64,
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = normalize_accent(hex)?;
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((channel(1)?, channel(3)?, channel(5)?))
}

fn to_hsl((r, g, b): (u8, u8, u8)) -> Hsl {
    let (r, g, b) = (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return Hsl { h: 0.0, s: 0.0, l };
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsl { h, s: s.min(1.0), l }
}

fn to_rgb(c: Hsl) -> (u8, u8, u8) {
    let (s, l) = (c.s.clamp(0.0, 1.0), c.l.clamp(0.0, 1.0));
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = c.h.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = l - chroma / 2.0;
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn triplet((r, g, b): (u8, u8, u8)) -> String {
    format!("{r}, {g}, {b}")
}

/// A neutral of the design: name, today's colour, alpha (`None`: opaque).
type Neutral = (&'static str, (u8, u8, u8), Option<&'static str>);

/// Today's neutrals (the default accent's).
const NEUTRALS: [Neutral; 12] = [
    ("--s-app", (11, 20, 18), None),
    ("--s-shell", (10, 19, 17), Some("0.38")),
    ("--s-row", (15, 28, 25), Some("0.84")),
    ("--s-sec", (13, 25, 22), Some("0.86")),
    ("--s-input", (7, 14, 12), Some("0.72")),
    ("--s-pop", (12, 22, 20), Some("0.97")),
    ("--text", (230, 246, 242), None),
    ("--text-2", (190, 215, 210), None),
    ("--text-3", (141, 167, 162), None),
    ("--text-4", (89, 112, 108), None),
    ("--text-code", (207, 233, 227), None),
    ("--s-knob", (185, 207, 202), None),
];

/// Neutrals the CSS reads as r, g, b (`rgba(var(--x-rgb), a)`): name and today's colour.
const NEUTRAL_TRIPLETS: [(&str, (u8, u8, u8)); 2] =
    [("--text-3-rgb", (141, 167, 162)), ("--s-scrim-rgb", (3, 8, 7))];

/// The edges, in the pale accent: name and alpha.
const LINES: [(&str, &str); 4] =
    [("--line", "0.09"), ("--line-2", "0.16"), ("--line-3", "0.32"), ("--line-ctl", "0.26")];

/// Below this saturation an accent counts as grey: the neutrals and the background lose colour
/// in proportion.
const GREY: f64 = 0.25;

/// How far a colour reads on the darkest surface, `--s-app` (WCAG contrast).
fn contrast_on_app(rgb: (u8, u8, u8)) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = |(r, g, b): (u8, u8, u8)| 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
    let (a, b) = (luminance(rgb), luminance((11, 20, 18)));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The accent as the design uses it: a colour too dark to read on the dark surfaces (and to carry
/// the primary button's dark ink) is lifted until it does; any other stays as chosen.
fn readable(rgb: (u8, u8, u8)) -> ((u8, u8, u8), Hsl) {
    let mut colour = to_hsl(rgb);
    let mut lifted = rgb;
    while contrast_on_app(lifted) < 3.2 && colour.l < 0.9 {
        colour.l = (colour.l + 0.02).min(0.9);
        lifted = to_rgb(colour);
    }
    (lifted, colour)
}

/// Every colour variable of the design, derived from `accent` (a bad one is the default). The
/// default accent gives today's `tokens.css`.
pub fn palette(accent: &str) -> Vec<(&'static str, String)> {
    let base = to_hsl(parse_hex(DEFAULT_ACCENT).unwrap_or((23, 195, 162)));
    let (rgb, a) = readable(parse_hex(accent).unwrap_or((23, 195, 162)));
    // The pale, light and dark tints keep the default's relation to its accent.
    let hi = to_rgb(Hsl { h: a.h, s: a.s * 0.914, l: (a.l + 0.135).min(a.l.max(0.85)) });
    let lo = to_rgb(Hsl { h: a.h, s: (a.s * 1.023).min(1.0), l: (a.l - 0.1215).max(a.l.min(0.12)) });
    let light = to_rgb(Hsl { h: a.h + 1.9, s: a.s * 0.824, l: 0.686 });
    // The primary button's gradient, and its top when hovered.
    let top = to_rgb(Hsl { h: a.h, s: (a.s * 0.92).min(1.0), l: (a.l + 0.059).min(a.l.max(0.9)) });
    let bottom = to_rgb(Hsl { h: a.h, s: (a.s * 1.025).min(1.0), l: (a.l - 0.0375).max(0.0) });
    let top_hover = to_rgb(Hsl { h: a.h, s: (a.s * 0.96).min(1.0), l: (a.l + 0.12).min(a.l.max(0.9)) });
    // A grey's hue does not matter: its share of colour (`colour`) is zero.
    let shift = a.h - base.h;
    let colour = if a.s < GREY { a.s / GREY } else { 1.0 };

    let mut vars = vec![
        ("--primary", hex(rgb)),
        ("--primary-hi", hex(hi)),
        ("--primary-lo", hex(lo)),
        ("--primary-light", hex(light)),
        ("--accent-rgb", triplet(rgb)),
        ("--accent-hi-rgb", triplet(hi)),
        ("--accent-light-rgb", triplet(light)),
        ("--ring", format!("0 0 0 3px rgba({}, 0.18)", triplet(rgb))),
        ("--primary-top", hex(top)),
        ("--primary-bottom", hex(bottom)),
        ("--primary-top-hover", hex(top_hover)),
        // The hue and the share of colour the design's dark tints take: `hsl(var(--tint-h) …)`.
        ("--tint-h", format!("{:.1}", (base.h + shift).rem_euclid(360.0))),
        ("--tint-s", if colour >= 1.0 { "1".to_string() } else { format!("{colour:.2}") }),
    ];
    // A neutral of today takes the accent's hue and its share of colour, and keeps its light.
    let tint = |today| {
        let n = to_hsl(today);
        to_rgb(Hsl { h: n.h + shift, s: n.s * colour, l: n.l })
    };
    for (name, today, alpha) in NEUTRALS {
        let tinted = tint(today);
        vars.push((
            name,
            match alpha {
                None => hex(tinted),
                Some(alpha) => format!("rgba({}, {alpha})", triplet(tinted)),
            },
        ));
    }
    for (name, today) in NEUTRAL_TRIPLETS {
        vars.push((name, triplet(tint(today))));
    }
    for (name, alpha) in LINES {
        vars.push((name, format!("rgba({}, {alpha})", triplet(light))));
    }
    // The picture keeps its own colours for the default; any other accent greys it and lays the
    // accent over it in the `color` blend mode (hue and saturation from the accent, light from
    // the picture), which keeps the hue exact where `hue-rotate` drifts.
    if shift.abs() < 1.0 && colour >= 1.0 {
        vars.push(("--bg-filter", "none".to_string()));
        vars.push(("--bg-tint", "transparent".to_string()));
    } else {
        let saturation = (a.s * 85.0).min(75.0) * colour;
        vars.push(("--bg-filter", "grayscale(1)".to_string()));
        vars.push(("--bg-tint", format!("hsl({:.0} {saturation:.0}% 50%)", a.h)));
    }
    vars
}

/// The page's background (`--s-app`) under `accent`, as r, g, b: the window shows it before the
/// page is drawn.
pub fn window_background(accent: &str) -> (u8, u8, u8) {
    palette(accent)
        .into_iter()
        .find(|(name, _)| *name == "--s-app")
        .and_then(|(_, hex)| parse_hex(&hex))
        .unwrap_or((11, 20, 18))
}

/// The palette of `accent` and the stacks of `font`: every property the window sets.
pub fn appearance_vars(font: UiFont, accent: &str) -> Vec<(&'static str, String)> {
    let (ui, head, mono) = font_stacks(font);
    let mut vars = palette(accent);
    vars.extend([("--f-ui", ui.to_string()), ("--f-head", head.to_string()), ("--f-mono", mono.to_string())]);
    vars
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn map(vars: Vec<(&'static str, String)>) -> HashMap<&'static str, String> {
        vars.into_iter().collect()
    }

    fn rgb_of(value: &str) -> (u8, u8, u8) {
        if let Some(rgb) = parse_hex(value) {
            return rgb;
        }
        let inner = value.trim_start_matches("rgba(").trim_end_matches(')');
        let parts: Vec<u8> = inner.split(',').take(3).map(|p| p.trim().parse().unwrap()).collect();
        (parts[0], parts[1], parts[2])
    }

    /// The hue of an `hsl(H S% L%)` value.
    fn hsl_hue(value: &str) -> f64 {
        value.trim_start_matches("hsl(").split(' ').next().unwrap().parse().unwrap()
    }

    fn close(value: &str, want: &str) {
        let (a, b) = (rgb_of(value), rgb_of(want));
        let near = |x: u8, y: u8| x.abs_diff(y) <= 2;
        assert!(near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2), "{value} is not {want}");
    }

    fn close_rgba(value: &str, (r, g, b, alpha): (u8, u8, u8, f64)) {
        close(value, &format!("#{r:02x}{g:02x}{b:02x}"));
        let got: f64 = value.trim_end_matches(')').rsplit(',').next().unwrap().trim().parse().unwrap();
        assert!((got - alpha).abs() < 0.001, "{value}: alpha {alpha}");
    }

    #[test]
    fn the_default_palette_reproduces_the_tokens() {
        let p = map(palette(DEFAULT_ACCENT));
        close(&p["--primary"], "#17c3a2");
        close(&p["--primary-hi"], "#3fe0c0");
        close(&p["--primary-lo"], "#0f8d77");
        close(&p["--primary-light"], "#7be3d2");
        close(&p["--s-app"], "#0b1412");
        close(&p["--text"], "#e6f6f2");
        close(&p["--text-2"], "#bed7d2");
        close(&p["--text-3"], "#8da7a2");
        close(&p["--text-4"], "#59706c");
        close_rgba(&p["--s-shell"], (10, 19, 17, 0.38));
        close_rgba(&p["--s-row"], (15, 28, 25, 0.84));
        close_rgba(&p["--s-sec"], (13, 25, 22, 0.86));
        close_rgba(&p["--s-input"], (7, 14, 12, 0.72));
        close_rgba(&p["--s-pop"], (12, 22, 20, 0.97));
        close_rgba(&p["--line-ctl"], (123, 227, 210, 0.26));
        close(&p["--primary-top"], "#22d6b2");
        close(&p["--primary-bottom"], "#13b494");
        close(&p["--primary-top-hover"], "#34e3c0");
        close(&p["--text-code"], "#cfe9e3");
        close(&p["--s-knob"], "#b9cfca");
        assert_eq!(p["--text-3-rgb"], "141, 167, 162");
        assert_eq!(p["--s-scrim-rgb"], "3, 8, 7");
        assert_eq!((p["--tint-h"].as_str(), p["--tint-s"].as_str()), ("168.5", "1"));
        assert_eq!(p["--accent-rgb"], "23, 195, 162");
        assert_eq!(p["--ring"], "0 0 0 3px rgba(23, 195, 162, 0.18)");
        assert_eq!((p["--bg-filter"].as_str(), p["--bg-tint"].as_str()), ("none", "transparent"));
    }

    #[test]
    fn a_blue_accent_moves_every_neutral_hue_and_keeps_its_lightness() {
        let accent = to_hsl(parse_hex("#3d9cf5").unwrap());
        let p = map(palette("#3d9cf5"));
        let app = to_hsl(rgb_of(&p["--s-app"]));
        let today = to_hsl((11, 20, 18));
        assert!((app.h - accent.h).abs() < 4.0, "{} vs {}", app.h, accent.h);
        assert!((app.l - today.l).abs() < 0.01);
        assert_eq!(p["--bg-filter"], "grayscale(1)");
        assert_eq!(hsl_hue(&p["--bg-tint"]).round(), accent.h.round(), "{}", p["--bg-tint"]);
    }

    #[test]
    fn the_shades_under_pictures_and_the_quiet_fills_take_the_accent() {
        let accent = to_hsl(parse_hex("#f0506e").unwrap());
        let p = map(palette("#f0506e"));
        for name in ["--text-3-rgb", "--s-scrim-rgb"] {
            let rgb = rgb_of(&format!("rgba({}, 1)", p[name]));
            assert!((to_hsl(rgb).h - accent.h).abs() < 6.0, "{name} = {}", p[name]);
        }
        assert_eq!(p["--text-3-rgb"], {
            let (r, g, b) = rgb_of(&p["--text-3"]);
            format!("{r}, {g}, {b}")
        });
    }

    #[test]
    fn a_grey_accent_desaturates_the_neutrals_and_the_background() {
        let p = map(palette("#c0c0c0"));
        assert!(to_hsl(rgb_of(&p["--s-app"])).s < 0.05);
        assert!(to_hsl(rgb_of(&p["--text-2"])).s < 0.05);
        assert_eq!(p["--tint-s"], "0.00");
        assert_eq!(p["--bg-filter"], "grayscale(1)");
        assert!(p["--bg-tint"].contains(" 0% "), "{}", p["--bg-tint"]);
    }

    #[test]
    fn an_extreme_accent_keeps_valid_colours() {
        for accent in ["#ffffff", "#000000"] {
            let p = map(palette(accent));
            for (name, value) in &p {
                let valid = value.starts_with('#') && parse_hex(value).is_some()
                    || value.starts_with("rgba(")
                    || value.contains(", ")
                    || name.starts_with("--tint-") && value.parse::<f64>().is_ok()
                    || name.ends_with("tint") && (value.starts_with("hsl(") || value == "transparent")
                    || name.ends_with("filter")
                    || name.ends_with("ring");
                assert!(valid, "{accent}: {name} = {value}");
            }
            assert_ne!(p["--primary-hi"], p["--primary-lo"], "{accent}");
        }
    }

    /// WCAG contrast of two colours.
    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let lum = |(r, g, b): (u8, u8, u8)| {
            let ch = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn a_muted_accent_still_tints_the_neutrals_its_way() {
        let accent = to_hsl(parse_hex("#6d5d8a").unwrap());
        let p = map(palette("#6d5d8a"));
        let app = to_hsl(rgb_of(&p["--s-app"]));
        assert!((app.h - accent.h).abs() < 6.0, "{} vs {}", app.h, accent.h);
        let tint: f64 = p["--tint-h"].parse().unwrap();
        assert!((tint - accent.h).abs() < 1.0, "{tint} vs {}", accent.h);
    }

    #[test]
    fn a_dark_accent_is_lifted_until_it_reads() {
        for accent in ["#000000", "#1e1e5a", "#0000ff", "#3a0000"] {
            let p = map(palette(accent));
            let primary = rgb_of(&p["--primary"]);
            assert!(
                contrast(primary, rgb_of(&p["--s-app"])) >= 3.0,
                "{accent}: {} on the app",
                p["--primary"]
            );
            // The primary button's ink: hsl(var(--tint-h) calc(var(--tint-s) * 84.6%) 7.6%).
            let tint: f64 = p["--tint-h"].parse().unwrap();
            let share: f64 = p["--tint-s"].parse().unwrap();
            let ink = to_rgb(Hsl { h: tint, s: 0.846 * share, l: 0.076 });
            assert!(contrast(primary, ink) >= 3.0, "{accent}: the ink on {}", p["--primary"]);
        }
        assert_eq!(
            map(palette(DEFAULT_ACCENT))["--primary"],
            DEFAULT_ACCENT,
            "a bright accent stays as chosen"
        );
    }

    /// The window's own background, under the page before it is drawn, is the page's.
    #[test]
    fn the_window_background_is_the_page_background() {
        assert_eq!(window_background(DEFAULT_ACCENT), (11, 20, 18), "tauri.conf.json's #0B1412");
        for accent in ["#3d9cf5", "#f0506e", "#c0c0c0", "teal"] {
            assert_eq!(window_background(accent), rgb_of(&map(palette(accent))["--s-app"]), "{accent}");
        }
    }

    #[test]
    fn a_bad_accent_is_the_default() {
        assert_eq!(palette("teal"), palette(DEFAULT_ACCENT));
    }

    #[test]
    fn accents_normalize() {
        assert_eq!(normalize_accent(" #17C3A2 ").as_deref(), Some("#17c3a2"));
        assert_eq!(normalize_accent("#abc").as_deref(), Some("#aabbcc"));
        for bad in ["", "17c3a2", "#12345", "#gggggg", "teal", "#"] {
            assert_eq!(normalize_accent(bad), None, "{bad}");
        }
    }

    #[test]
    fn presets_are_distinct_valid_colours() {
        let ids: std::collections::HashSet<&str> = ACCENT_PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), ACCENT_PRESETS.len());
        for p in ACCENT_PRESETS {
            assert_eq!(normalize_accent(p.hex).as_deref(), Some(p.hex), "{}", p.id);
            assert_eq!(p.label_key, format!("appearance_accent_{}", p.id));
        }
        assert_eq!((ACCENT_PRESETS[0].id, ACCENT_PRESETS[0].hex), ("default", DEFAULT_ACCENT));
    }

    #[test]
    fn every_font_has_three_stacks() {
        for font in UiFont::ALL {
            let (ui, head, mono) = font_stacks(font);
            assert!(!ui.is_empty() && !head.is_empty() && !mono.is_empty(), "{font:?}");
        }
        assert!(font_stacks(UiFont::Minecraft).0.starts_with("\"Monocraft\""));
        assert_eq!(
            font_stacks(UiFont::Standard),
            (
                "\"Inter\", \"Segoe UI\", system-ui, sans-serif",
                "\"Exo 2\", \"Inter\", sans-serif",
                "\"JetBrains Mono\", Consolas, monospace"
            ),
            "today's tokens.css"
        );
    }

    #[test]
    fn appearance_vars_round_trip_as_json() {
        let vars = appearance_vars(UiFont::Minecraft, "#9b6cf6");
        let json = serde_json::to_string(&vars).unwrap();
        let back: Vec<(String, String)> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), vars.len());
        assert!(back.iter().zip(&vars).all(|(b, v)| b.0 == v.0 && b.1 == v.1));
        assert!(back.iter().any(|(k, v)| k == "--f-ui" && v.contains("Monocraft")));
    }
}
