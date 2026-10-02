//! Sizes and counts as the player's language writes them: «1,5 МБ», «1,3 тис.» in Ukrainian,
//! "1.5 MB", "1.3K" in English.

/// The language writes Ukrainian (`uk_UA`).
fn ukrainian(lang: &str) -> bool {
    lang.starts_with("uk")
}

/// A file's size: KB below a megabyte (whole, at least 1), MB and GB with one decimal.
pub fn size(bytes: u64, lang: &str) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let (kb, mb, gb) = if ukrainian(lang) { ("КБ", "МБ", "ГБ") } else { ("KB", "MB", "GB") };
    let b = bytes as f64;
    if b >= GB {
        format!("{} {gb}", decimal(b / GB, lang))
    } else if b >= MB {
        format!("{} {mb}", decimal(b / MB, lang))
    } else {
        format!("{} {kb}", (b / KB).ceil().max(1.0) as u64)
    }
}

/// `value` with one decimal, the language's separator.
fn decimal(value: f64, lang: &str) -> String {
    let text = format!("{value:.1}");
    if ukrainian(lang) { text.replace('.', ",") } else { text }
}

/// A count of downloads: as is below a thousand, then thousands and millions with one decimal
/// (none when it is `.0`).
pub fn count(n: u64, lang: &str) -> String {
    let (thousands, millions) = if ukrainian(lang) { (" тис.", " млн") } else { ("K", "M") };
    let one = |value: f64, suffix: &str| {
        let text = decimal(value, lang);
        let text = text.strip_suffix(".0").or_else(|| text.strip_suffix(",0")).unwrap_or(&text).to_string();
        format!("{text}{suffix}")
    };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => one(n as f64 / 1_000.0, thousands),
        _ => one(n as f64 / 1_000_000.0, millions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn sizes_read_as_the_language_writes_them() {
        assert_eq!(size(MB + MB / 2, "uk_UA"), "1,5 МБ");
        assert_eq!(size(900 * 1024, "uk_UA"), "900 КБ");
        assert_eq!(size(10, "uk_UA"), "1 КБ");
        assert_eq!(size(2 * 1024 * MB + 100 * MB, "uk_UA"), "2,1 ГБ");
        assert_eq!(size(MB + MB / 2, "en_US"), "1.5 MB");
        assert_eq!(size(900 * 1024, "en_US"), "900 KB");
        assert_eq!(size(2 * 1024 * MB + 100 * MB, "en_US"), "2.1 GB");
    }

    #[test]
    fn counts_read_as_the_language_writes_them() {
        assert_eq!(count(999, "uk_UA"), "999");
        assert_eq!(count(1_260, "uk_UA"), "1,3 тис.");
        assert_eq!(count(1_000, "uk_UA"), "1 тис.");
        assert_eq!(count(12_345_678, "uk_UA"), "12,3 млн");
        assert_eq!(count(1_260, "en_US"), "1.3K");
        assert_eq!(count(3_000_000, "en_US"), "3M");
    }
}
