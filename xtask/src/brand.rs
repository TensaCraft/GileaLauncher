//! The code carries no brand: the launcher's name, identifier and URLs live in the build profile
//! (`build-profiles/*.toml`, read through `branding`), so a rebrand changes the profile, not code.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Where the brand may appear (paths from the repository root): the profiles, the app's product
/// config, the one module that reads the profile, this guard's own cases, the docs and the
/// app's icons.
const ALLOWED: [&str; 7] = [
    "build-profiles/",
    "crates/launcher-app/tauri.conf.json",
    "crates/launcher-app/icons/",
    "crates/launcher-shared/src/branding.rs",
    "xtask/src/brand.rs",
    "docs/",
    "README.md",
];

/// Whether `line` names the brand: `ualauncher` in any case, `ua://`, `--ua-`, a word that
/// starts with `ua-`, `ua_` or `UA_`, a word `ua` on its own (`/opt/ua/`, `into_ua`), or a name
/// that starts with `Ua` followed by anything but a lowercase letter (`UaError`, `Ua(`).
pub fn branded(line: &str) -> bool {
    // The Ukrainian locale code (`uk_ua`, Minecraft's `lang:uk_ua`) is no brand.
    let line = &line.replace("uk_ua", "uk_xx");
    let lower = line.to_lowercase();
    if lower.contains("ualauncher") || lower.contains("ua://") || lower.contains("--ua-") {
        return true;
    }
    let bytes = line.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    (0..bytes.len().saturating_sub(1)).any(|i| {
        let start = i == 0 || !word(bytes[i - 1]);
        let next = bytes.get(i + 2).copied();
        let pair = &bytes[i..i + 2];
        let prefixed = start && matches!(bytes.get(i..i + 3), Some(b"ua-" | b"ua_" | b"UA_"));
        let alone = pair == b"ua"
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && next.is_none_or(|b| !b.is_ascii_alphanumeric() && b != b'_');
        let capital = start && pair == b"Ua" && next.is_none_or(|b| !b.is_ascii_lowercase());
        prefixed || alone || capital
    })
}

/// The repository's files git knows of (tracked, or new and not ignored), as `/` paths.
pub(crate) fn repo_files(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .expect("git lists the repository's files");
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

/// The brand the profiles name (their `app_name`s, lower case): kept out of code as well.
pub fn brand_words(root: &Path) -> Vec<String> {
    let mut words: Vec<String> = fs::read_dir(root.join("build-profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(".toml").map(str::to_string))
        .filter_map(|profile| crate::profile::load_profile(root, &profile).ok())
        .map(|spec| spec.app_name().to_lowercase())
        .filter(|name| name.len() >= 3)
        .collect();
    words.sort();
    words.dedup();
    words
}

/// `line` names one of `words` (any case).
pub fn names_brand(line: &str, words: &[String]) -> bool {
    let lower = line.to_lowercase();
    words.iter().any(|word| lower.contains(word.as_str()))
}

/// What the user sees: the interface and its texts, the tray, the browser page after signing in.
/// None of it names the app (`APP_NAME`, a text's `{app}`): the brand stays in file names.
const SHOWN: [&str; 5] = [
    "crates/launcher-ui/",
    "crates/ui-kit/",
    "assets/langs/",
    "crates/launcher-app/src/tray.rs",
    "crates/launcher-core/src/auth/page.rs",
];

/// The one place in the interface that shows the app's name: About (owner, 2026-10-02).
const NAMED: &str = "crates/launcher-ui/src/pages/settings/about.rs";

/// The lines of what the user sees (`SHOWN`, the modules' interface parts) that name the app.
pub fn shown_offenders(root: &Path) -> Vec<String> {
    let shown = |rel: &str| {
        rel != NAMED
            && (SHOWN.iter().any(|s| rel.starts_with(s))
                || (rel.starts_with("modules/") && rel.contains("/src/ui")))
    };
    let mut found = Vec::new();
    for rel in repo_files(root).into_iter().filter(|rel| shown(rel)) {
        let Ok(text) = fs::read_to_string(root.join(&rel)) else { continue };
        for (n, line) in
            text.lines().enumerate().filter(|(_, l)| l.contains("APP_NAME") || l.contains("{app}"))
        {
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}

pub fn offenders(root: &Path) -> Vec<String> {
    let words = brand_words(root);
    let mut found = Vec::new();
    for rel in repo_files(root) {
        if ALLOWED.iter().any(|a| rel.starts_with(a)) {
            continue;
        }
        // Text only: pictures and other binaries are not code.
        let Ok(text) = fs::read_to_string(root.join(&rel)) else { continue };
        for (n, line) in text.lines().enumerate().filter(|(_, l)| branded(l) || names_brand(l, &words)) {
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_words_are_found_and_neutral_ones_are_not() {
        for line in [
            "use ua_core::x;",
            "class=\"ua-btn\"",
            "--ua-text-3",
            "ua://builds",
            "UA_APP_NAME",
            ".ualauncher/x",
            "UaLauncher",
            "UaError",
            "-> UaResult<()>",
            "FlowError::Ua(e)",
            "fn into_ua(self)",
            "/opt/ua/Launcher",
            "Ua<Launcher>",
            "run -p ua-app --",
        ] {
            assert!(branded(line), "{line}");
        }
        for line in [
            "uk_UA.json",
            "usual",
            "aqua-marine",
            "qua_lity",
            "--text-3",
            "app://builds",
            "Моди й шейдери",
            "AppError",
            "quality",
            "usual_ua_like",
            "Uganda",
            "Update",
            "lang:uk_ua",
        ] {
            assert!(!branded(line), "{line}");
        }
    }

    #[test]
    fn the_brand_from_the_profiles_stays_out_of_code() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let words = brand_words(root);
        assert!(words.contains(&"gilealauncher".to_string()), "{words:?}");
        assert!(names_brand("let title = \"GileaLauncher\";", &words));
        assert!(names_brand("com.gilealauncher.instance", &words));
        assert!(!names_brand("let title = APP_NAME;", &words));
    }

    #[test]
    fn the_code_carries_no_brand() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let found = offenders(root);
        assert!(
            found.is_empty(),
            "{} brand-bound lines, e.g.:\n{}",
            found.len(),
            found[..found.len().min(40)].join("\n")
        );
    }

    #[test]
    fn the_launcher_shows_no_brand() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let found = shown_offenders(root);
        assert!(found.is_empty(), "the interface names the app:\n{}", found.join("\n"));
        let about = fs::read_to_string(root.join(NAMED)).unwrap();
        assert!(about.contains("APP_NAME"), "About is where the app's name shows");
    }

    /// `"key": "value"` in a JSON text (the first one).
    fn json_field<'a>(text: &'a str, key: &str) -> &'a str {
        let pattern = format!("\"{key}\": \"");
        let start = text.find(&pattern).unwrap_or_else(|| panic!("{key}")) + pattern.len();
        &text[start..start + text[start..].find('"').unwrap()]
    }

    /// `or(option_env!("VAR"), "default")` in branding.rs.
    fn branding_default<'a>(text: &'a str, var: &str) -> &'a str {
        let pattern = format!("option_env!(\"{var}\"), \"");
        let at = text.find(&pattern).unwrap_or_else(|| panic!("{var}")) + pattern.len();
        &text[at..at + text[at..].find('"').unwrap()]
    }

    #[test]
    fn brand_values_agree_across_profiles_the_app_config_and_branding() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let env = |spec: &crate::profile::BuildSpec, var: &str| {
            spec.env.iter().find(|(k, _)| k == var).map(|(_, v)| v.clone()).unwrap_or_default()
        };
        let standard = crate::profile::load_profile(root, "standard").unwrap();
        let (name, id) = (env(&standard, "LAUNCHER_APP_NAME"), env(&standard, "LAUNCHER_IDENTIFIER"));
        let branding = fs::read_to_string(root.join("crates/launcher-shared/src/branding.rs")).unwrap();
        assert_eq!(branding_default(&branding, "LAUNCHER_APP_NAME"), name, "branding.rs default name");
        assert_eq!(branding_default(&branding, "LAUNCHER_IDENTIFIER"), id, "branding.rs default identifier");
        assert_eq!(
            branding_default(&branding, "LAUNCHER_ISSUES_URL"),
            env(&standard, "LAUNCHER_ISSUES_URL"),
            "branding.rs default issues address"
        );
        let tauri = fs::read_to_string(root.join("crates/launcher-app/tauri.conf.json")).unwrap();
        for key in ["productName", "mainBinaryName"] {
            assert_eq!(json_field(&tauri, key), name, "tauri.conf.json {key}");
        }
        let words = brand_words(root);
        assert!(!names_brand(json_field(&tauri, "title"), &words), "the window's title names no brand");
        assert_eq!(json_field(&tauri, "identifier"), id, "tauri.conf.json identifier");
        // One app config serves every profile: they name the same app.
        for entry in fs::read_dir(root.join("build-profiles")).unwrap().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(profile) = file.strip_suffix(".toml") else { continue };
            let spec = crate::profile::load_profile(root, profile).unwrap();
            assert_eq!(
                (env(&spec, "LAUNCHER_APP_NAME"), env(&spec, "LAUNCHER_IDENTIFIER")),
                (name.clone(), id.clone()),
                "{file}"
            );
        }
    }
}
