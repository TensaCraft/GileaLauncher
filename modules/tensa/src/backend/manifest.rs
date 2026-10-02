//! What the server lists for a build: its files and folders, read safely, the
//! force-update manifest checked whole, and the rules that keep a player's own files.

use launcher_core::content::backups::BACKUPS;
use launcher_core::net::downloader::ExpectedHash;
use launcher_core::storage::journal::normalized_path;
use launcher_core::storage::versions::reserved_in_build;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::Value;

use super::pack::truthy;

/// A file or folder the launcher keeps in a build: the build's record, everything `.launcher…`,
/// and the mods a replacement took away (`mods/.backups`). A server never writes or removes them.
pub fn launchers_own(path: &str) -> bool {
    let lower = path.to_lowercase();
    let backups = format!("mods/{BACKUPS}");
    reserved_in_build(path) || lower == backups || lower.starts_with(&format!("{backups}/"))
}

/// A path from the server as a path inside the build: `\` becomes `/`, a leading `/` goes; a drive
/// (`C:`), `..`, nothing at all, a name the build's file transaction refuses (a part ending in `.`
/// or a space, a `:`, a Windows device) or one of the launcher's own (`launchers_own`) is no path.
pub fn safe_relative(raw: &str) -> Option<String> {
    let text = raw.trim().replace('\\', "/");
    let text = text.trim_start_matches('/');
    if text.chars().nth(1) == Some(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
        .filter(|path| !path.is_empty() && normalized_path(path).is_ok() && !launchers_own(path))
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).map_or("", str::trim)
}

/// A file's path as the server writes it (checked by `safe_relative` before use).
fn raw_path(file: &Value) -> String {
    let relative = string(file, "relative_path").replace('\\', "/");
    if !relative.is_empty() {
        return relative.trim_start_matches('/').to_string();
    }
    let path = string(file, "path").replace('\\', "/");
    let (path, name) = (path.trim_matches('/'), string(file, "name"));
    if !path.is_empty() && !name.is_empty() { format!("{path}/{name}") } else { name.to_string() }
}

/// A file's path in the build: `relative_path`, else `path` + `name`, else `name`.
pub fn relative_path(file: &Value) -> Option<String> {
    safe_relative(&raw_path(file))
}

/// A file's hash: SHA-256, else SHA-1.
pub fn expected_hash(file: &Value) -> Option<ExpectedHash> {
    let (sha256, sha1) = (string(file, "sha256"), string(file, "sha1"));
    if !sha256.is_empty() {
        Some(ExpectedHash::sha256(sha256))
    } else {
        (!sha1.is_empty()).then(|| ExpectedHash::sha1(sha1))
    }
}

/// A whole number from a number or a string.
fn whole(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// A file's size when the server gives one above zero.
pub fn size(file: &Value) -> Option<u64> {
    whole(file.get("size")).filter(|&size| size > 0)
}

/// A force-update manifest is whole: `files` and `directories` are lists, their counts match its
/// `summary` (counts that are no counts aside), and every file has a path, an address and a hash.
/// An unsafe path is left to the plan, which skips that file.
pub fn validate(manifest: &Value) -> AppResult<()> {
    let invalid = |detail: String| AppError::new(ErrorCode::InvalidInput, detail);
    let files = manifest.get("files").and_then(Value::as_array);
    let directories = manifest.get("directories").and_then(Value::as_array);
    let (Some(files), Some(directories)) = (files, directories) else {
        return Err(invalid("The force-update manifest has an invalid structure".into()));
    };
    if let Some(summary) = manifest.get("summary").filter(|s| s.is_object()) {
        if whole(summary.get("returned_files_count")).is_some_and(|n| n != files.len() as u64) {
            return Err(invalid("The force-update manifest's file count does not match its summary".into()));
        }
        if whole(summary.get("directories_count")).is_some_and(|n| n != directories.len() as u64) {
            return Err(invalid(
                "The force-update manifest's folder count does not match its summary".into(),
            ));
        }
    }
    for file in files {
        let path = raw_path(file);
        if path.is_empty() || string(file, "download_url").is_empty() || expected_hash(file).is_none() {
            let shown = if path.is_empty() { "<missing path>" } else { path.as_str() };
            return Err(invalid(format!("Incomplete force-update file entry: {shown}")));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreserveKind {
    File,
    Directory,
    Glob,
}

/// A path the server says to keep, whatever it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreserveRule {
    pub kind: PreserveKind,
    pub path: String,
}

/// The rules that apply: disabled ones, unknown kinds and unsafe paths go.
pub fn preserve_rules(raw: Option<&Value>) -> Vec<PreserveRule> {
    let rules = raw.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    rules
        .iter()
        .filter_map(|rule| {
            let rule = rule.as_object()?;
            if rule.get("enabled").is_some_and(|enabled| !truthy(enabled)) {
                return None;
            }
            let kind = match rule.get("type")?.as_str()?.trim().to_lowercase().as_str() {
                "file" => PreserveKind::File,
                "directory" => PreserveKind::Directory,
                "glob" => PreserveKind::Glob,
                _ => return None,
            };
            Some(PreserveRule { kind, path: safe_relative(rule.get("path")?.as_str()?)? })
        })
        .collect()
}

/// `path` is `root` or inside it, part by part.
pub(crate) fn under(path: &str, root: &str) -> bool {
    let root: Vec<&str> = root.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let parts: Vec<&str> = path.split('/').collect();
    !root.is_empty() && parts.len() >= root.len() && parts[..root.len()] == root[..]
}

/// `text` matches `pattern`: `*` is any run of characters, `?` one; case counts.
fn wildcard(text: &str, pattern: &str) -> bool {
    let (text, pattern): (Vec<char>, Vec<char>) = (text.chars().collect(), pattern.chars().collect());
    let (mut t, mut p, mut star, mut resume) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            t += 1;
            p += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// `path` matches the glob part by part from its end (as Python's `PurePosixPath.match`).
fn glob(path: &str, pattern: &str) -> bool {
    let (parts, patterns): (Vec<&str>, Vec<&str>) = (path.split('/').collect(), pattern.split('/').collect());
    patterns.len() <= parts.len()
        && parts.iter().rev().zip(patterns.iter().rev()).all(|(p, g)| wildcard(p, g))
}

/// `path` (inside the build) is kept by `rules`.
pub fn preserved(path: &str, rules: &[PreserveRule]) -> bool {
    rules.iter().any(|rule| match rule.kind {
        PreserveKind::File => path == rule.path,
        PreserveKind::Directory => under(path, &rule.path),
        PreserveKind::Glob => glob(path, &rule.path),
    })
}

#[cfg(test)]
mod tests {
    use launcher_core::net::downloader::HashKind;
    use serde_json::json;

    use super::*;

    #[test]
    fn unsafe_paths_are_no_paths() {
        // A part the file transaction would refuse (Windows reads it as another name) is none either.
        for bad in [
            "../a",
            "C:/a",
            "c:\\a",
            "a/../b",
            ".",
            "",
            "  ",
            "/",
            "..",
            "mods/x.jar.",
            "config/a:b.json",
            "mods /x.jar",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
        assert_eq!(safe_relative("/abs").as_deref(), Some("abs"));
        assert_eq!(safe_relative("a\\b").as_deref(), Some("a/b"));
        assert_eq!(safe_relative(" mods/./a.jar ").as_deref(), Some("mods/a.jar"));
        assert_eq!(safe_relative("config//x/").as_deref(), Some("config/x"));
    }

    #[test]
    fn server_paths_cannot_reach_the_launchers_files() {
        for own in [
            "version.json",
            "VERSION.JSON",
            ".launcher",
            ".launcher/modrinth-pack.json",
            ".Launcher/x",
            ".launcher-pack-sync.json",
            ".launcher-sync/stage/a.jar",
            "mods/.backups/sodium.jar.backup",
            "MODS/.Backups",
        ] {
            assert_eq!(safe_relative(own), None, "{own:?}");
        }
        for fine in ["config/version.json", "mods/a.jar", "launcher.txt", "config/.backups/x"] {
            assert_eq!(safe_relative(fine).as_deref(), Some(fine), "{fine:?}");
        }
    }

    #[test]
    fn windows_device_names_are_refused() {
        for device in ["con", "mods/AUX", "nul.txt", "config/com1.json", "lpt9", "Prn.jar", "mods/com0"] {
            assert_eq!(safe_relative(device), None, "{device:?}");
        }
        for fine in ["console.txt", "mods/auxiliary.jar", "com10.txt", "nullable/a", "lpt.txt"] {
            assert_eq!(safe_relative(fine).as_deref(), Some(fine), "{fine:?}");
        }
    }

    #[test]
    fn a_file_path_comes_from_relative_path_or_path_and_name() {
        assert_eq!(
            relative_path(&json!({"relative_path": "\\mods\\a.jar", "name": "b"})).as_deref(),
            Some("mods/a.jar")
        );
        assert_eq!(
            relative_path(&json!({"path": "/config/x/", "name": "c.json"})).as_deref(),
            Some("config/x/c.json")
        );
        assert_eq!(relative_path(&json!({"name": "options.txt"})).as_deref(), Some("options.txt"));
        assert_eq!(relative_path(&json!({"path": "mods"})), None, "a folder without a name is no file");
        assert_eq!(relative_path(&json!({"relative_path": "../evil.jar"})), None);
    }

    #[test]
    fn sha256_wins_over_sha1_and_sizes_must_be_positive() {
        let both = expected_hash(&json!({"sha1": "AA", "sha256": " BB "})).unwrap();
        assert_eq!((both.kind, both.hex.as_str()), (HashKind::Sha256, "bb"));
        let sha1 = expected_hash(&json!({"sha1": "AA", "sha256": ""})).unwrap();
        assert_eq!((sha1.kind, sha1.hex.as_str()), (HashKind::Sha1, "aa"));
        assert_eq!(expected_hash(&json!({"md5": "x"})), None);
        assert_eq!(size(&json!({"size": 12})), Some(12));
        assert_eq!(size(&json!({"size": "12"})), Some(12));
        for none in [json!({"size": 0}), json!({"size": -3}), json!({"size": "x"}), json!({})] {
            assert_eq!(size(&none), None, "{none}");
        }
    }

    #[test]
    fn a_manifest_must_match_its_summary_and_have_complete_files() {
        let file = json!({"relative_path": "mods/a.jar", "download_url": "u", "sha1": "aa"});
        assert!(validate(&json!({"files": [file], "directories": []})).is_ok());
        let counted = json!({
            "files": [file], "directories": [{"path": "mods"}],
            "summary": {"returned_files_count": 1, "directories_count": "1"}
        });
        assert!(validate(&counted).is_ok());
        let wrong = json!({"files": [file], "directories": [], "summary": {"returned_files_count": 2}});
        assert_eq!(validate(&wrong).unwrap_err().code, ErrorCode::InvalidInput);
        let ignored = json!({
            "files": [file], "directories": [],
            "summary": {"returned_files_count": "many", "directories_count": -1}
        });
        assert!(validate(&ignored).is_ok(), "counts that are no counts are ignored");
        assert!(validate(&json!({"files": {}, "directories": []})).is_err());
        let unhashed =
            json!({"files": [{"relative_path": "mods/b.jar", "download_url": "u"}], "directories": []});
        let error = validate(&unhashed).unwrap_err();
        assert!(error.detail.contains("mods/b.jar"), "{}", error.detail);
        let unsafe_path = json!({"files": [{"relative_path": "../x.jar", "download_url": "u", "sha1": "aa"}], "directories": []});
        assert!(
            validate(&unsafe_path).is_ok(),
            "an unsafe file is skipped when planning, not the whole manifest"
        );
        let nameless = json!({"files": [{"download_url": "u", "sha1": "aa"}], "directories": []});
        assert!(validate(&nameless).unwrap_err().detail.contains("<missing path>"));
    }

    #[test]
    fn preserve_rules_skip_disabled_unknown_and_unsafe_ones() {
        let raw = json!([
            {"type": "file", "path": "options.txt"},
            {"type": "Directory", "path": "saves", "enabled": "yes"},
            {"type": "glob", "path": "config/*.local", "enabled": true},
            {"type": "file", "path": "off.txt", "enabled": false},
            {"type": "regex", "path": ".*"},
            {"type": "file", "path": "../x"},
            "junk"
        ]);
        let rule = |kind, path: &str| PreserveRule { kind, path: path.into() };
        assert_eq!(
            preserve_rules(Some(&raw)),
            [
                rule(PreserveKind::File, "options.txt"),
                rule(PreserveKind::Directory, "saves"),
                rule(PreserveKind::Glob, "config/*.local")
            ]
        );
        assert!(preserve_rules(Some(&json!({"type": "file"}))).is_empty());
        assert!(preserve_rules(None).is_empty());
    }

    #[test]
    fn preserved_matches_files_folders_and_globs() {
        let rule = |kind, path: &str| PreserveRule { kind, path: path.into() };
        let rules = [
            rule(PreserveKind::File, "config/keep.txt"),
            rule(PreserveKind::Directory, "config/keep"),
            rule(PreserveKind::Glob, "config/*.json"),
            rule(PreserveKind::Glob, "*.txt"),
            rule(PreserveKind::Glob, "shots/img?.png"),
        ];
        for kept in ["config/keep.txt", "config/keep/x/y", "config/a.json", "mods/a.txt", "shots/img1.png"] {
            assert!(preserved(kept, &rules), "{kept}");
        }
        for gone in ["config/keep2/x", "config/x/a.json", "config/A.JSON", "mods/a.jar", "shots/img10.png"] {
            assert!(!preserved(gone, &rules), "{gone}");
        }
    }
}
