//! Build ids and names.

use launcher_shared::{AppError, AppResult, ErrorCode};

pub const MAX_ID_CHARS: usize = 180;
pub const MAX_NEW_ID_CHARS: usize = 64;

pub(crate) const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Ukrainian letters as the original's `transliterate` package writes them (language "uk",
/// reversed). Other letters (including Russian ё ы э ъ) stay and later become `_`.
fn transliterate(c: char) -> Option<&'static str> {
    Some(match c {
        'ь' | 'Ь' => "'",
        'є' => "ye",
        'Є' => "Ye",
        'ж' => "zh",
        'Ж' => "Zh",
        'ї' => "yi",
        'Ї' => "Yi",
        'х' => "kh",
        'Х' => "Kh",
        'ц' => "ts",
        'Ц' => "Ts",
        'ч' => "ch",
        'Ч' => "Ch",
        'ш' => "sh",
        'Ш' => "Sh",
        'щ' => "shch",
        'Щ' => "Shch",
        'ю' => "ju",
        'Ю' => "Ju",
        'я' => "ja",
        'Я' => "Ja",
        'а' => "a",
        'А' => "A",
        'б' => "b",
        'Б' => "B",
        'в' => "v",
        'В' => "V",
        'г' => "h",
        'Г' => "H",
        'ґ' => "g",
        'Ґ' => "G",
        'д' => "d",
        'Д' => "D",
        'е' => "e",
        'Е' => "E",
        'з' => "z",
        'З' => "Z",
        'и' => "y",
        'И' => "Y",
        'і' => "i",
        'І' => "I",
        'й' => "j",
        'Й' => "J",
        'к' => "k",
        'К' => "K",
        'л' => "l",
        'Л' => "L",
        'м' => "m",
        'М' => "M",
        'н' => "n",
        'Н' => "N",
        'о' => "o",
        'О' => "O",
        'п' => "p",
        'П' => "P",
        'р' => "r",
        'Р' => "R",
        'с' => "s",
        'С' => "S",
        'т' => "t",
        'Т' => "T",
        'у' => "u",
        'У' => "U",
        'ф' => "f",
        'Ф' => "F",
        _ => return None,
    })
}

fn random_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("the OS random number generator is available");
    bytes.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect()
}

/// The build id of a name, byte-for-byte as the original: Ukrainian transliteration, then every
/// character outside `[a-zA-Z0-9]` becomes `_`, lowercase. Empty → 10 random `[a-z0-9]`.
pub fn normalize_string(text: &str) -> String {
    let mut latin = String::with_capacity(text.len());
    for c in text.chars() {
        match transliterate(c) {
            Some(s) => latin.push_str(s),
            None => latin.push(c),
        }
    }
    let id: String =
        latin.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    if id.is_empty() { random_id() } else { id }
}

/// A single safe folder name for `versions/<id>` or `games/<id>`.
pub fn validate_component_id(id: &str) -> AppResult<()> {
    let stem = id.split('.').next().unwrap_or_default();
    let reason = if id.is_empty() {
        Some("empty")
    } else if id.trim() != id {
        Some("surrounding whitespace")
    } else if id.chars().count() > MAX_ID_CHARS {
        Some("too long")
    } else if id == "." || id == ".." {
        Some("dot name")
    } else if id.ends_with(' ') || id.ends_with('.') {
        Some("trailing space or dot")
    } else if id.chars().any(|c| "<>:\"/\\|?*".contains(c) || c.is_control()) {
        Some("forbidden character")
    } else if WINDOWS_RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        Some("reserved Windows name")
    } else {
        None
    };
    match reason {
        None => Ok(()),
        Some(reason) => Err(AppError::new(ErrorCode::InvalidInput, format!("invalid id {id:?}: {reason}"))
            .with_param("id", id)),
    }
}

/// `base`, or `base (2)`, `base (3)`… — the first name nobody uses (case-insensitive).
pub fn unique_name(base: &str, taken: &[String]) -> String {
    let base = base.trim();
    let is_taken = |name: &str| taken.iter().any(|t| t.to_lowercase() == name.to_lowercase());
    if !is_taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} ({n})")).find(|name| !is_taken(name)).expect("an unused name exists")
}

/// The folder id for a new build named `name`: normalized, at most `MAX_NEW_ID_CHARS` long, a
/// valid folder name, and one `taken` does not report.
pub fn new_build_id(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut base = normalize_string(name);
    base.truncate(MAX_NEW_ID_CHARS - 4);
    unique_id(&base, |id| taken(id) || validate_component_id(id).is_err())
}

/// `base_id`, or `base_id_2`, `base_id_3`… — the first id for which `taken` is false, so a new or
/// copied build never reuses (or deletes) an existing folder.
pub fn unique_id(base_id: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base_id) {
        return base_id.to_string();
    }
    (2..).map(|n| format!("{base_id}_{n}")).find(|id| !taken(id)).expect("an unused id exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_build_ids_are_short_valid_and_unused() {
        assert_eq!(new_build_id("Моя збірка", |_| false), "moja_zbirka");
        assert_eq!(new_build_id("Моя збірка", |id| id == "moja_zbirka"), "moja_zbirka_2");
        assert_eq!(new_build_id("CON", |_| false), "con_2");
        assert_eq!(new_build_id("lpt1", |id| id == "lpt1_2"), "lpt1_3");
        let long = new_build_id(&"x".repeat(300), |id| id.len() == MAX_NEW_ID_CHARS - 4);
        assert!(long.len() <= MAX_NEW_ID_CHARS && validate_component_id(&long).is_ok(), "{long}");
    }

    #[test]
    fn normalize_matches_the_original_launcher() {
        // Vectors produced by the original's SecurityService.normalize_string.
        let cases = [
            ("Aeronautics (Roxy)", "aeronautics__roxy_"),
            ("Моя збірка 1.21", "moja_zbirka_1_21"),
            ("Щука їжак ґава Ь", "shchuka_yizhak_gava__"),
            ("Привет мир ёж ы э ъ", "pryvet_myr__zh______"),
            ("UPPER Єва Юля Яна", "upper_yeva_julja_jana"),
            ("a-b.c/d\\e", "a_b_c_d_e"),
            ("   ", "___"),
            ("Ẓalgo ñ", "_algo__"),
            ("Tensa 2.0 — фінал", "tensa_2_0___final"),
        ];
        for (name, id) in cases {
            assert_eq!(normalize_string(name), id, "{name}");
        }
    }

    #[test]
    fn empty_name_gets_a_random_id() {
        let a = normalize_string("");
        let b = normalize_string("");
        assert_eq!(a.len(), 10);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_ne!(a, b);
    }

    #[test]
    fn component_ids_are_safe_folder_names() {
        for good in ["1.21.1", "fabric-loader-0.16.10-1.21.1", "neoforge-21.1.77", "moja_zbirka", "Ідея"]
        {
            assert!(validate_component_id(good).is_ok(), "{good}");
        }
        let long = "x".repeat(MAX_ID_CHARS + 1);
        for bad in [
            "",
            " a",
            "a ",
            ".",
            "..",
            "a.",
            "a/b",
            "a\\b",
            "c:x",
            "a*b",
            "a\u{0}b",
            "con",
            "AUX",
            "Com1",
            "lpt9.txt",
            long.as_str(),
        ] {
            assert_eq!(validate_component_id(bad).unwrap_err().code, ErrorCode::InvalidInput, "{bad:?}");
        }
    }

    #[test]
    fn names_get_a_numbered_suffix_when_taken() {
        let taken = vec!["Aeronautics".to_string(), "aeronautics (2)".to_string()];
        assert_eq!(unique_name("Fresh", &taken), "Fresh");
        assert_eq!(unique_name("AERONAUTICS", &taken), "AERONAUTICS (3)");
        assert_eq!(unique_name("  Aeronautics  ", &[]), "Aeronautics");
    }

    #[test]
    fn ids_never_collide_with_taken_folders() {
        // "Моя збірка" and "Моя-збірка" normalize to the same id: the second one gets a suffix.
        let first = normalize_string("Моя збірка");
        let second = normalize_string("Моя-збірка");
        assert_eq!(first, second);
        let taken = [first.clone(), format!("{first}_2")];
        assert_eq!(unique_id(&second, |id| taken.iter().any(|t| t == id)), format!("{first}_3"));
        assert_eq!(unique_id("free", |_| false), "free");
    }
}
