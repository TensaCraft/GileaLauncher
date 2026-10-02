//! `config.json` store with merge-on-save semantics.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use super::json::{JsonRead, read_json_object, write_json_file};
use super::path_lock;

pub const CONFIG_INDENT: usize = 4;

pub fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[derive(Default)]
struct Inner {
    data: Map<String, Value>,
    baseline: Map<String, Value>,
    changed: BTreeSet<String>,
    deleted: BTreeSet<String>,
    /// The file on disk could not be parsed the last time it was read.
    file_invalid: bool,
}

pub struct ConfigStore {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl ConfigStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let store = Self { path: path.into(), inner: Mutex::new(Inner::default()) };
        store.reload();
        store
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Re-reads the file. Unreadable or invalid content keeps the current in-memory values.
    pub fn reload(&self) {
        let loaded = match read_json_object(&self.path) {
            Ok(JsonRead::Object(map)) => Some(map),
            Ok(JsonRead::Missing) => Some(Map::new()),
            Ok(JsonRead::Invalid(reason)) => {
                tracing::warn!("Ignoring invalid config file {}: {reason}", self.path.display());
                None
            }
            Err(e) => {
                tracing::error!("Unable to read config file {}: {e}", self.path.display());
                None
            }
        };
        let mut inner = self.lock();
        match loaded {
            Some(map) => {
                inner.baseline = map.clone();
                inner.data = map;
                inner.changed.clear();
                inner.deleted.clear();
                inner.file_invalid = false;
            }
            None => inner.file_invalid = true,
        }
    }

    /// False while the file on disk is corrupt; the next explicit change backs it up and rewrites it.
    pub fn is_healthy(&self) -> bool {
        !self.lock().file_invalid
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().data.get(key).cloned()
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Reads `"yes"`/`"no"` (and legacy JSON booleans).
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "yes" | "true" | "1" | "on" => true,
                "no" | "false" | "0" | "off" => false,
                _ => default,
            },
            Some(Value::Bool(b)) => b,
            _ => default,
        }
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        match self.get(key)? {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.lock().data.contains_key(key)
    }

    pub fn keys(&self) -> Vec<String> {
        self.lock().data.keys().cloned().collect()
    }

    pub fn set(&self, key: &str, value: Value) -> io::Result<()> {
        self.set_many(vec![(key.to_string(), value)])
    }

    pub fn set_bool(&self, key: &str, value: bool) -> io::Result<()> {
        self.set(key, Value::String(yes_no(value).into()))
    }

    pub fn set_many(&self, entries: Vec<(String, Value)>) -> io::Result<()> {
        let mut inner = self.lock();
        for (key, value) in entries {
            inner.deleted.remove(&key);
            inner.changed.insert(key.clone());
            inner.data.insert(key, value);
        }
        self.save_locked(&mut inner)
    }

    /// Adds values for keys that are not set yet. Persists only while the file is healthy, so a
    /// corrupt `config.json` is never rewritten just because the launcher started.
    pub fn set_defaults(&self, entries: Vec<(String, Value)>) -> io::Result<()> {
        let mut inner = self.lock();
        let mut added = false;
        for (key, value) in entries {
            if !inner.data.contains_key(&key) {
                inner.changed.insert(key.clone());
                inner.data.insert(key, value);
                added = true;
            }
        }
        if added && !inner.file_invalid { self.save_locked(&mut inner) } else { Ok(()) }
    }

    pub fn delete(&self, key: &str) -> io::Result<()> {
        let mut inner = self.lock();
        inner.data.remove(key);
        inner.changed.remove(key);
        inner.deleted.insert(key.to_string());
        self.save_locked(&mut inner)
    }

    fn save_locked(&self, inner: &mut Inner) -> io::Result<()> {
        let file_lock = path_lock(&self.path);
        let _guard = file_lock.lock().unwrap_or_else(|e| e.into_inner());

        let mut merged = match read_json_object(&self.path)? {
            JsonRead::Object(map) => map,
            JsonRead::Missing => Map::new(),
            JsonRead::Invalid(reason) => {
                // Keep the damaged file for the user, then write everything this instance knows.
                let backup = self.backup_corrupt_file()?;
                tracing::warn!(
                    "Config file {} is invalid ({reason}); saved a copy to {} and rewrote it",
                    self.path.display(),
                    backup.display()
                );
                inner.data.clone()
            }
        };
        for (key, value) in &inner.data {
            // Keys changed by this instance (explicitly or by in-place mutation) override the file.
            let touched = inner.changed.contains(key) || inner.baseline.get(key) != Some(value);
            if touched {
                merged.insert(key.clone(), value.clone());
            }
        }
        let removed: Vec<String> = inner
            .deleted
            .iter()
            .cloned()
            .chain(inner.baseline.keys().filter(|k| !inner.data.contains_key(*k)).cloned())
            .collect();
        for key in removed {
            merged.remove(&key);
        }

        write_json_file(&self.path, &Value::Object(merged.clone()), CONFIG_INDENT)?;
        inner.data = merged.clone();
        inner.baseline = merged;
        inner.changed.clear();
        inner.deleted.clear();
        inner.file_invalid = false;
        Ok(())
    }

    fn backup_corrupt_file(&self) -> io::Result<PathBuf> {
        super::json::backup_corrupt_file(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn store(dir: &tempfile::TempDir) -> ConfigStore {
        ConfigStore::open(dir.path().join("config.json"))
    }

    fn corrupt_backups(dir: &tempfile::TempDir) -> Vec<String> {
        fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("config.json.corrupt-"))
            .collect()
    }

    #[test]
    fn save_over_corrupt_file_keeps_every_memory_value_and_backs_up_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = store(&dir);
        cfg.set_many(vec![("a".into(), json!(1)), ("b".into(), json!(2))]).unwrap();
        fs::write(cfg.path(), "{ broken").unwrap();
        cfg.reload();
        cfg.set("c", json!(3)).unwrap();
        let fresh = store(&dir);
        assert_eq!(fresh.get("a"), Some(json!(1)));
        assert_eq!(fresh.get("b"), Some(json!(2)));
        assert_eq!(fresh.get("c"), Some(json!(3)));
        let backups = corrupt_backups(&dir);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(fs::read_to_string(dir.path().join(&backups[0])).unwrap(), "{ broken");
    }

    #[test]
    fn corrupt_file_is_backed_up_before_the_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "null").unwrap();
        let cfg = ConfigStore::open(&path);
        assert!(!cfg.is_healthy());
        cfg.set("lang", json!("uk_UA")).unwrap();
        assert!(cfg.is_healthy());
        let backups = corrupt_backups(&dir);
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(dir.path().join(&backups[0])).unwrap(), "null");
    }

    #[test]
    fn defaults_on_a_corrupt_file_stay_in_memory_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let garbage = b"{\"lang\":\"uk_UA\",\"compact_sidebar\":\"no\"";
        fs::write(&path, garbage).unwrap();
        let cfg = ConfigStore::open(&path);
        cfg.set_defaults(vec![("world_backups_enabled".into(), json!("no"))]).unwrap();
        assert_eq!(cfg.get("world_backups_enabled"), Some(json!("no")));
        assert_eq!(fs::read(&path).unwrap(), garbage);
        assert!(corrupt_backups(&dir).is_empty());
    }

    #[test]
    fn defaults_fill_only_missing_keys_and_persist_when_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = store(&dir);
        cfg.set("flag", json!("no")).unwrap();
        cfg.set_defaults(vec![("flag".into(), json!("yes")), ("count".into(), json!(3))]).unwrap();
        let fresh = store(&dir);
        assert_eq!(fresh.get("flag"), Some(json!("no")));
        assert_eq!(fresh.get("count"), Some(json!(3)));
    }

    #[test]
    fn missing_file_starts_empty_and_set_persists_with_indent_4() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = store(&dir);
        assert_eq!(cfg.get("lang"), None);
        cfg.set("lang", json!("uk_UA")).unwrap();
        let text = fs::read_to_string(cfg.path()).unwrap();
        assert_eq!(text, "{\n    \"lang\": \"uk_UA\"\n}");
    }

    #[test]
    fn bools_are_stored_as_yes_no() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = store(&dir);
        cfg.set_bool("auto_update", false).unwrap();
        assert_eq!(cfg.get("auto_update"), Some(json!("no")));
        assert!(!cfg.get_bool("auto_update", true));
        assert!(cfg.get_bool("missing", true));
        cfg.set("legacy", json!(true)).unwrap();
        assert!(cfg.get_bool("legacy", false));
    }

    #[test]
    fn stale_writer_merges_only_its_own_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = store(&dir);
        let b = store(&dir); // opened before `a` writes: stale view
        a.set("a", json!(1)).unwrap();
        b.set("b", json!(2)).unwrap();
        let fresh = store(&dir);
        assert_eq!(fresh.get("a"), Some(json!(1)));
        assert_eq!(fresh.get("b"), Some(json!(2)));
    }

    #[test]
    fn explicit_set_wins_over_other_writer() {
        let dir = tempfile::tempdir().unwrap();
        let a = store(&dir);
        let b = store(&dir);
        a.set("x", json!(1)).unwrap();
        b.set("x", json!(2)).unwrap();
        assert_eq!(store(&dir).get("x"), Some(json!(2)));
    }

    #[test]
    fn delete_removes_key_but_keeps_other_writers_keys() {
        let dir = tempfile::tempdir().unwrap();
        let a = store(&dir);
        a.set("keep", json!("k")).unwrap();
        a.set("gone", json!("g")).unwrap();
        let b = store(&dir);
        a.set("late", json!(3)).unwrap();
        b.delete("gone").unwrap();
        let fresh = store(&dir);
        assert_eq!(fresh.get("gone"), None);
        assert_eq!(fresh.get("keep"), Some(json!("k")));
        assert_eq!(fresh.get("late"), Some(json!(3)));
    }

    #[test]
    fn corrupt_file_is_not_overwritten_until_explicit_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for garbage in [&b""[..], b"null", b"[]", b"\x00\x01garbage"] {
            fs::write(&path, garbage).unwrap();
            let cfg = ConfigStore::open(&path);
            assert!(cfg.keys().is_empty());
            assert_eq!(fs::read(&path).unwrap(), garbage);
        }
        let cfg = ConfigStore::open(&path);
        cfg.set("lang", json!("en_US")).unwrap();
        assert_eq!(ConfigStore::open(&path).get("lang"), Some(json!("en_US")));
    }

    #[test]
    fn corrupt_reload_keeps_last_good_values() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = store(&dir);
        cfg.set("lang", json!("uk_UA")).unwrap();
        fs::write(cfg.path(), "{ broken").unwrap();
        cfg.reload();
        assert_eq!(cfg.get("lang"), Some(json!("uk_UA")));
    }

    #[test]
    fn save_error_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("as_dir");
        fs::create_dir(&path).unwrap();
        let cfg = ConfigStore::open(&path);
        assert!(cfg.set("k", json!(1)).is_err());
    }
}
