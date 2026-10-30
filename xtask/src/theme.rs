//! Every colour of the design is a variable of `tokens.css`: no other stylesheet, and no inline
//! style of the window's code, names a colour by value, so the accent (and any later theme)
//! changes it in one place.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::brand::repo_files;

/// The one stylesheet that names colours.
const TOKENS: &str = "crates/ui-kit/styles/tokens.css";

/// Where the window's code may set an inline style.
const UI_CODE: [&str; 3] = ["crates/ui-kit/src/", "crates/launcher-ui/src/", "modules/"];

/// Colour keywords a stylesheet could name instead of a variable.
const NAMED: [&str; 24] = [
    "white", "black", "red", "green", "blue", "gray", "grey", "silver", "orange", "yellow", "purple", "pink",
    "teal", "cyan", "magenta", "lime", "navy", "maroon", "olive", "aqua", "fuchsia", "gold", "brown",
    "violet",
];

fn without_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        rest = rest[open..].find("*/").map_or("", |close| &rest[open + close + 2..]);
    }
    out.push_str(rest);
    out
}

/// `#` and three to eight hex digits, ending there.
fn has_hex(value: &str) -> bool {
    value.match_indices('#').any(|(at, _)| {
        let digits = value[at + 1..].chars().take_while(char::is_ascii_hexdigit).count();
        let after = value[at + 1 + digits..].chars().next();
        matches!(digits, 3 | 4 | 6 | 8) && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// `rgb(`/`rgba(` with a number in it, any `hsl(`/`hsla(`, or a colour keyword.
fn has_function_or_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let numbered = ["rgb(", "rgba("].iter().any(|f| {
        lower.match_indices(f).any(|(at, _)| {
            lower[at + f.len()..].trim_start().chars().next().is_some_and(|c| c.is_ascii_digit())
        })
    });
    let words = lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
    numbered
        || lower.contains("hsl(")
        || lower.contains("hsla(")
        || words.into_iter().any(|w| NAMED.contains(&w))
}

/// `value` without its quoted strings and `url(…)`s, which name no colour.
fn unquoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    loop {
        let next = [rest.find('"'), rest.find('\''), rest.find("url(")].into_iter().flatten().min();
        let Some(at) = next else { break };
        out.push_str(&rest[..at]);
        let close =
            if rest[at..].starts_with("url(") { ')' } else { rest[at..].chars().next().unwrap_or('"') };
        let from = if close == ')' { at + 4 } else { at + 1 };
        rest = rest[from..].find(close).map_or("", |end| &rest[from + end + 1..]);
    }
    out.push_str(rest);
    out
}

/// Three numbers alone (`69, 194, 126`): the r, g, b a `--x-rgb` variable holds.
fn is_triplet(value: &str) -> bool {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.parse::<u8>().is_ok())
}

fn names_a_colour(value: &str) -> bool {
    let value = unquoted(value);
    has_hex(&value) || has_function_or_name(&value) || is_triplet(&value)
}

/// The values of `css`'s declarations (`property: value`), each with its property.
fn declarations(css: &str) -> Vec<(String, String)> {
    without_comments(css)
        .split(['{', '}', ';'])
        .filter_map(|part| {
            let (property, value) = part.split_once(':')?;
            let property = property.trim();
            let is_property =
                !property.is_empty() && property.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            is_property.then(|| (property.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// The declarations of `css` that name a colour by value.
fn colours_in(css: &str) -> Vec<String> {
    declarations(css)
        .into_iter()
        .filter(|(_, value)| names_a_colour(value))
        .map(|(property, value)| format!("{property}: {value}"))
        .collect()
}

/// What a line of code says inside its `"…"` strings, each apart.
fn quoted(line: &str) -> String {
    line.split('"').skip(1).step_by(2).collect::<Vec<_>>().join(" ")
}

/// The inline styles of a line of the window's code that name a colour by value: in code a colour
/// sits inside a string.
fn inline_colour(line: &str) -> bool {
    let strings = quoted(line);
    line.contains("style") && (has_hex(&strings) || has_function_or_name(&strings))
}

fn stylesheets(root: &Path) -> Vec<String> {
    repo_files(root).into_iter().filter(|rel| rel.ends_with(".css")).collect()
}

pub fn offenders(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for rel in stylesheets(root).into_iter().filter(|rel| rel != TOKENS) {
        let Ok(css) = fs::read_to_string(root.join(&rel)) else { continue };
        found.extend(colours_in(&css).into_iter().map(|d| format!("{rel}: {d}")));
    }
    let code = repo_files(root).into_iter().filter(|rel| {
        rel.ends_with(".rs") && UI_CODE.iter().any(|dir| rel.starts_with(dir)) && !rel.contains("/tests/")
    });
    for rel in code {
        let Ok(text) = fs::read_to_string(root.join(&rel)) else { continue };
        for (n, line) in text.lines().enumerate().filter(|(_, line)| inline_colour(line)) {
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}

/// `--name` read through `var(--name…)` in `text`; a name a `format!` completes (`--loader-{x}`)
/// is no name yet.
fn read_vars(text: &str) -> BTreeSet<String> {
    text.match_indices("var(--")
        .filter_map(|(at, _)| {
            let name = &text[at + 4..];
            let end = name.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(name.len());
            (!name[end..].starts_with('{')).then(|| name[..end].to_string())
        })
        .collect()
}

/// `--name` given a value in `text` (`--name:`).
fn defined_vars(text: &str) -> BTreeSet<String> {
    text.match_indices("--")
        .filter_map(|(at, _)| {
            let rest = &text[at..];
            let end = rest[2..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))? + 2;
            (rest[end..].trim_start().starts_with(':') && end > 2).then(|| rest[..end].to_string())
        })
        .collect()
}

/// The variables a stylesheet or the window's code reads that nothing gives a value.
pub fn undefined(root: &Path) -> Vec<String> {
    let files: Vec<String> = repo_files(root)
        .into_iter()
        .filter(|rel| {
            rel.ends_with(".css") || rel.ends_with(".rs") && UI_CODE.iter().any(|dir| rel.starts_with(dir))
        })
        .collect();
    let texts: Vec<(String, String)> =
        files.into_iter().filter_map(|rel| Some((fs::read_to_string(root.join(&rel)).ok()?, rel))).collect();
    let defined: BTreeSet<String> = texts.iter().flat_map(|(text, _)| defined_vars(text)).collect();
    let mut found = Vec::new();
    for (text, rel) in &texts {
        for name in read_vars(text).difference(&defined) {
            found.push(format!("{rel}: {name}"));
        }
    }
    found
}

/// The literal value of each `--name` in `tokens`.
fn token_values(tokens: &str) -> Vec<(String, String)> {
    declarations(tokens).into_iter().filter(|(name, _)| name.starts_with("--")).collect()
}

/// The `#rrggbb` colour `--x` and its `--x-rgb` triplet that disagree.
pub fn mismatched_triplets(tokens: &str) -> Vec<String> {
    let values = token_values(tokens);
    let hex_of = |name: &str| values.iter().find(|(n, _)| n == name).map(|(_, v)| v.to_ascii_lowercase());
    let mut found = Vec::new();
    for (name, triplet) in values.iter().filter(|(n, _)| n.ends_with("-rgb")) {
        // The accent's triplets are named after the accent, its colours after `--primary`.
        let base = name.trim_end_matches("-rgb");
        let colour_name = match base.strip_prefix("--accent") {
            Some(rest) => format!("--primary{rest}"),
            None => base.to_string(),
        };
        let Some(colour) = hex_of(&colour_name) else { continue };
        let Some(hex) = colour.strip_prefix('#').filter(|h| h.len() == 6) else { continue };
        let channels: Vec<String> = (0..3)
            .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .map(|c| c.to_string())
            .collect();
        if channels.join(", ") != *triplet {
            found.push(format!("{name}: {triplet} is not {colour}"));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
    }

    #[test]
    fn the_guard_sees_a_colour_named_by_value() {
        for css in [
            ".x { color: #fff; }",
            ".x { background: rgba(3, 8, 7, 0.7); }",
            ".x { box-shadow: 0 0 8px rgb( 0 0 0 / 50%); }",
            ".x { color: hsl(var(--tint-h) 20% 8%); }",
            ".x { border: 1px solid white; }",
            ".x {\n  fill: #8fe9b2;\n}",
            ".chip--x { --chip: 69, 194, 126; }",
        ] {
            assert_eq!(colours_in(css).len(), 1, "{css}");
        }
        for css in [
            ".x { color: var(--primary); background: rgba(var(--accent-rgb), 0.2); }",
            "#app { color: transparent; border-color: currentColor; }",
            ".x { content: \"#fff\"; }",
            ".x { grid-area: a1; animation: fade 120ms; font-family: \"Material Icons\"; }",
            "/* rgba(1, 2, 3, 0.5) */ .x { color: inherit; }",
            ".x:hover { color: var(--text); }",
        ] {
            assert_eq!(colours_in(css), Vec::<String>::new(), "{css}");
        }
    }

    /// In the window's code a colour sits inside a string: the guard reads the strings.
    #[test]
    fn the_guard_sees_a_colour_in_an_inline_style_of_the_code() {
        for line in [
            r#"<div style="color:#fff">"#,
            r#"<span style=format!("background: rgba(0, 0, 0, .5); width:{w}px")>"#,
            r#"let style = "border: 1px solid white";"#,
        ] {
            assert!(inline_colour(line), "{line}");
        }
        for line in [
            r#"<div style="color: var(--text)">"#,
            r#"<span style=format!("--swatch:{c}")>"#,
            r#"<div class="white-space">"#,
            r#"// the #fff of the old style"#,
        ] {
            assert!(!inline_colour(line), "{line}");
        }
    }

    #[test]
    fn no_stylesheet_and_no_inline_style_names_a_colour() {
        assert_eq!(offenders(root()), Vec::<String>::new());
    }

    #[test]
    fn every_variable_read_has_a_value() {
        assert_eq!(undefined(root()), Vec::<String>::new());
    }

    #[test]
    fn each_colour_agrees_with_its_triplet() {
        assert_eq!(
            mismatched_triplets(".x { --ok: #45c27e; --ok-rgb: 69, 194, 126; }"),
            Vec::<String>::new()
        );
        assert_eq!(mismatched_triplets(".x { --ok: #45c27e; --ok-rgb: 69, 194, 127; }").len(), 1);
        // The accent's triplets are named after the accent, its colours after `--primary`.
        assert_eq!(
            mismatched_triplets(".x { --primary-light: #7be3d2; --accent-light-rgb: 123, 227, 211; }").len(),
            1
        );
        let tokens = fs::read_to_string(root().join(TOKENS)).unwrap();
        assert_eq!(mismatched_triplets(&tokens), Vec::<String>::new());
    }
}
