//! The user's own Java list and the cache of discovered Java, both kept in
//! `config.json` as `[{label: path}]` like the original.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ErrorCode, JavaEntry, JavaList};
use serde_json::{Map, Value, json};

use super::discovery::{DiscoveryRoots, discover, java_home_of, resolve_path};
use crate::storage::config::ConfigStore;

pub const CUSTOM_JAVA_KEY: &str = "custom_java_versions";
pub const LAUNCHER_JAVA_KEY: &str = "launcher_java_versions";
pub const LAUNCHER_JAVA_SCAN_KEY: &str = "launcher_java_versions_last_scan";
pub const RESCAN_AFTER_SECS: f64 = 24.0 * 60.0 * 60.0;

/// Seconds since 1970, as the original stores `launcher_java_versions_last_scan`.
pub fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

const EXECUTABLE_NAMES: [&str; 5] = ["java", "java.exe", "javaw", "javaw.exe", "minecraftjava.exe"];

fn path_key(path: &str) -> String {
    path.trim().to_lowercase()
}

/// `[{label: path}]`: the first pair of each object; empty labels or paths and repeated paths
/// (ignoring case) are skipped.
pub fn normalize_entries(value: Option<&Value>) -> Vec<JavaEntry> {
    let Some(Value::Array(items)) = value else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for item in items {
        let Value::Object(pairs) = item else { continue };
        let first = pairs.iter().find_map(|(label, path)| {
            let (label, path) = (label.trim(), path.as_str().unwrap_or_default().trim());
            (!label.is_empty() && !path.is_empty()).then(|| (label.to_string(), path.to_string()))
        });
        if let Some((label, path)) = first
            && seen.insert(path_key(&path))
        {
            entries.push(JavaEntry { label, path });
        }
    }
    entries
}

fn to_value(entries: &[JavaEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|e| {
                let mut pair = Map::new();
                pair.insert(e.label.clone(), Value::String(e.path.clone()));
                Value::Object(pair)
            })
            .collect(),
    )
}

fn io_error(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

/// A Java launcher the user picked: quotes and `~` are allowed; it must be an existing file named
/// like Java (`custom_java_invalid` otherwise). Returned without links.
pub fn resolve_java_executable(raw: &str) -> AppResult<PathBuf> {
    let invalid = || {
        AppError::new(ErrorCode::InvalidJavaExecutable, format!("not a Java executable: {raw}"))
            .with_param("path", raw)
    };
    let text = raw.trim().trim_matches('"').trim();
    if text.is_empty() {
        return Err(invalid());
    }
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            dirs::home_dir().ok_or_else(invalid)?.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(text),
    };
    let path = resolve_path(&expanded).ok_or_else(invalid)?;
    let named = path
        .file_name()
        .is_some_and(|n| EXECUTABLE_NAMES.contains(&n.to_string_lossy().to_lowercase().as_str()));
    if !path.is_file() || !named {
        return Err(invalid());
    }
    Ok(path)
}

/// The default label: the Java home's folder name.
pub fn label_from_path(path: &Path) -> String {
    java_home_of(path)
        .file_name()
        .or(path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub struct JavaService {
    config: Arc<ConfigStore>,
    roots: DiscoveryRoots,
}

impl JavaService {
    pub fn new(config: Arc<ConfigStore>, roots: DiscoveryRoots) -> JavaService {
        JavaService { config, roots }
    }

    pub fn list(&self) -> JavaList {
        JavaList {
            launcher: normalize_entries(self.config.get(LAUNCHER_JAVA_KEY).as_ref()),
            custom: normalize_entries(self.config.get(CUSTOM_JAVA_KEY).as_ref()),
        }
    }

    /// Adds (or relabels) a Java of the user's; an empty label becomes the Java home's name.
    pub fn add_custom(&self, label: &str, path: &str) -> AppResult<JavaList> {
        let path = resolve_java_executable(path)?;
        let label = match label.trim() {
            "" => label_from_path(&path),
            given => given.to_string(),
        };
        let text = path.to_string_lossy().into_owned();
        let mut custom: Vec<JavaEntry> =
            self.list().custom.into_iter().filter(|e| path_key(&e.path) != path_key(&text)).collect();
        custom.push(JavaEntry { label, path: text });
        self.config.set(CUSTOM_JAVA_KEY, to_value(&custom)).map_err(io_error)?;
        Ok(self.list())
    }

    pub fn remove_custom(&self, path: &str) -> AppResult<JavaList> {
        let custom: Vec<JavaEntry> =
            self.list().custom.into_iter().filter(|e| path_key(&e.path) != path_key(path)).collect();
        self.config.set(CUSTOM_JAVA_KEY, to_value(&custom)).map_err(io_error)?;
        Ok(self.list())
    }

    /// The discovery cache is empty, holds raw runtime names (`java-runtime-…`, from old versions)
    /// or is a day old.
    pub fn needs_rescan(&self, now_secs: f64) -> bool {
        let cached = self.list().launcher;
        let last = self.config.get(LAUNCHER_JAVA_SCAN_KEY).and_then(|v| v.as_f64()).unwrap_or(0.0);
        cached.is_empty()
            || cached.iter().any(|e| e.label.starts_with("java-runtime-"))
            || now_secs - last > RESCAN_AFTER_SECS
    }

    /// Searches this computer (blocking) and stores what it found with the time.
    pub fn rescan(&self, now_secs: f64) -> AppResult<JavaList> {
        let found: Vec<JavaEntry> = discover(&self.roots)
            .into_iter()
            .map(|f| JavaEntry { label: f.label, path: f.path.to_string_lossy().into_owned() })
            .collect();
        self.config
            .set_many(vec![
                (LAUNCHER_JAVA_KEY.to_string(), to_value(&found)),
                (LAUNCHER_JAVA_SCAN_KEY.to_string(), json!(now_secs)),
            ])
            .map_err(io_error)?;
        Ok(self.list())
    }

    /// "Scan" in the settings: every discovered Java not listed yet joins the user's list.
    pub fn import_discovered(&self, now_secs: f64) -> AppResult<usize> {
        let list = self.rescan(now_secs)?;
        let mut custom = list.custom;
        let mut seen: HashSet<String> = custom.iter().map(|e| path_key(&e.path)).collect();
        let mut added = 0;
        for entry in list.launcher {
            let Ok(path) = resolve_java_executable(&entry.path) else { continue };
            let text = path.to_string_lossy().into_owned();
            if seen.insert(path_key(&text)) {
                let label = if entry.label.trim().is_empty() { label_from_path(&path) } else { entry.label };
                custom.push(JavaEntry { label, path: text });
                added += 1;
            }
        }
        if added > 0 {
            self.config.set(CUSTOM_JAVA_KEY, to_value(&custom)).map_err(io_error)?;
        }
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct Setup {
        dir: tempfile::TempDir,
        service: JavaService,
        config: Arc<ConfigStore>,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(ConfigStore::open(dir.path().join("config.json")));
        let roots = DiscoveryRoots {
            minecraft_dir: dir.path().join("mc"),
            app_state_dir: dir.path().join("state"),
            common_roots: vec![dir.path().join("jdks")],
            ..DiscoveryRoots::default()
        };
        Setup { service: JavaService::new(config.clone(), roots), config, dir }
    }

    fn java_at(dir: &Path, folder: &str) -> PathBuf {
        let bin = dir.join(folder).join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("java"), b"java").unwrap();
        resolve_path(&bin.join("java")).unwrap()
    }

    #[test]
    fn entries_read_like_the_original() {
        let raw = json!([{"A": "C:/j/a.exe"}, {"B": "c:/J/A.EXE"}, {"": "x"}, {"C": ""}, "junk", {"D": "d.exe", "E": "e.exe"}]);
        let entries = normalize_entries(Some(&raw));
        let pairs: Vec<(&str, &str)> = entries.iter().map(|e| (e.label.as_str(), e.path.as_str())).collect();
        assert_eq!(pairs, [("A", "C:/j/a.exe"), ("D", "d.exe")]);
        assert!(normalize_entries(Some(&json!("broken"))).is_empty());
        assert!(normalize_entries(None).is_empty());
    }

    #[test]
    fn custom_java_is_checked_and_replaced() {
        let s = setup();
        let java = java_at(s.dir.path(), "jdk-21");
        let quoted = format!("  \"{}\"  ", java.display());
        let list = s.service.add_custom("", &quoted).unwrap();
        assert_eq!(
            list.custom,
            [JavaEntry { label: "jdk-21".into(), path: java.to_string_lossy().into_owned() }]
        );
        let list = s.service.add_custom("My JDK", &java.to_string_lossy()).unwrap();
        assert_eq!(list.custom.len(), 1, "the same Java is replaced");
        assert_eq!(list.custom[0].label, "My JDK");
        for bad in [
            "",
            "   ",
            "\"\"",
            &s.dir.path().join("jdk-21").to_string_lossy(),
            &s.dir.path().join("nope.exe").to_string_lossy(),
        ] {
            assert_eq!(
                s.service.add_custom("x", bad).unwrap_err().code,
                ErrorCode::InvalidJavaExecutable,
                "{bad:?}"
            );
        }
        let notepad = s.dir.path().join("notepad.exe");
        fs::write(&notepad, b"x").unwrap();
        assert_eq!(
            s.service.add_custom("x", &notepad.to_string_lossy()).unwrap_err().code,
            ErrorCode::InvalidJavaExecutable
        );
        let list = s.service.remove_custom(&java.to_string_lossy().to_uppercase()).unwrap();
        assert!(list.custom.is_empty(), "removal ignores case");
        assert_eq!(s.config.get(CUSTOM_JAVA_KEY), Some(json!([])));
    }

    #[test]
    fn the_scan_cache_refreshes_when_stale_and_scan_imports_new_java() {
        let s = setup();
        assert!(s.service.needs_rescan(1000.0), "an empty cache");
        let first = java_at(&s.dir.path().join("jdks"), "jdk-17");
        let list = s.service.rescan(1000.0).unwrap();
        assert_eq!(list.launcher.len(), 1);
        assert_eq!(s.config.get(LAUNCHER_JAVA_SCAN_KEY), Some(json!(1000.0)));
        assert!(!s.service.needs_rescan(1000.0 + 3600.0));
        assert!(s.service.needs_rescan(1000.0 + RESCAN_AFTER_SECS + 1.0), "a day later");
        s.config.set(LAUNCHER_JAVA_KEY, json!([{"java-runtime-gamma": "x"}])).unwrap();
        assert!(s.service.needs_rescan(1000.0), "raw runtime names from old versions");
        s.service.add_custom("Mine", &first.to_string_lossy()).unwrap();
        java_at(&s.dir.path().join("jdks"), "jdk-21");
        assert_eq!(s.service.import_discovered(2000.0).unwrap(), 1, "only the new one");
        assert_eq!(s.service.import_discovered(2000.0).unwrap(), 0);
        assert_eq!(s.service.list().custom.len(), 2);
    }
}
