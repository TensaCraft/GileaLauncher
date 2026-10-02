//! What Launcher installed from Modrinth: `<game>/.launcher/modrinth-content.json`
//! in the original's schema 2. A record counts only while its file — or that file switched off —
//! still has the recorded hash.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use launcher_core::content::jar::inspect_mod_jar;
use launcher_core::net::downloader::{HashKind, hash_file};
use launcher_shared::ContentKind;
use serde_json::{Map, Value, json};

use super::catalog::text;

pub const PROVENANCE: &str = ".launcher/modrinth-content.json";
const SCHEMA: u64 = 2;

/// The record's tab key (the original's `content_key`).
pub fn content_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mods",
        ContentKind::ResourcePacks => "resourcepacks",
        ContentKind::ShaderPacks => "shaders",
    }
}

/// One installed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub kind: ContentKind,
    pub filename: String,
    pub project_id: String,
    pub project_slug: String,
    pub project_title: String,
    pub version_id: String,
    pub version_number: String,
    /// `sha512` or `sha1`.
    pub hash_algorithm: &'static str,
    pub file_hash: String,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({
            "content_key": content_key(self.kind),
            "filename": self.filename,
            "project_id": self.project_id,
            "project_slug": self.project_slug,
            "project_title": self.project_title,
            "version_id": self.version_id,
            "version_number": self.version_number,
            "hash_algorithm": self.hash_algorithm,
            "file_hash": self.file_hash,
        })
    }
}

/// The document on disk; `None` when it is missing or not a JSON object.
pub fn read(game: &Path) -> Option<Map<String, Value>> {
    let text = fs::read_to_string(game.join(PROVENANCE)).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.as_object().cloned()
}

/// A record's hash when the check supports its algorithm and the digest is well formed.
pub(crate) fn record_hash(record: &Value) -> Option<(&'static str, HashKind, String)> {
    let algorithm = text(record, "hash_algorithm").to_ascii_lowercase();
    let expected = text(record, "file_hash").to_ascii_lowercase();
    let (name, kind, len) = supported(&algorithm)?;
    (expected.len() == len && expected.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some((name, kind, expected))
}

/// `document` (or a new one) without the records of `removed` files (with or without `.disabled`)
/// and with `records`, each in place of any record of its file; every other field stays.
pub fn with_records(
    document: Option<Map<String, Value>>,
    removed: &[String],
    records: &[(String, Record)],
) -> Value {
    let mut document = document.unwrap_or_default();
    let mut files = match document.remove("files") {
        Some(Value::Object(files)) => files,
        _ => Map::new(),
    };
    for path in removed {
        files.remove(path);
        files.remove(path.strip_suffix(".disabled").unwrap_or(path));
    }
    for (relative, record) in records {
        files.remove(&format!("{relative}.disabled"));
        files.insert(relative.clone(), record.to_json());
    }
    document.insert("schema_version".into(), json!(SCHEMA));
    document.insert("files".into(), Value::Object(files));
    Value::Object(document)
}

pub fn algorithm_name(kind: HashKind) -> &'static str {
    match kind {
        HashKind::Sha1 => "sha1",
        HashKind::Sha256 => "sha256",
        HashKind::Sha512 => "sha512",
    }
}

/// A recorded algorithm the check supports, with its digest length.
fn supported(algorithm: &str) -> Option<(&'static str, HashKind, usize)> {
    match algorithm {
        "sha512" => Some(("sha512", HashKind::Sha512, 128)),
        "sha1" => Some(("sha1", HashKind::Sha1, 40)),
        _ => None,
    }
}

/// A mod jar's own id and name (its `fabric.mod.json` and the like), when it declares them.
pub type ModMeta = Option<(String, Option<String>)>;

/// A jar's size, modification time and what it declares.
type ModEntry = (u64, Option<SystemTime>, ModMeta);

/// Digests and mod jars' ids by path, reused while a file keeps its size and modification time.
#[derive(Default)]
pub struct DigestCache {
    digests: Mutex<HashMap<DigestKey, Digest>>,
    mods: Mutex<HashMap<PathBuf, ModEntry>>,
}

/// A file and the algorithm.
type DigestKey = (PathBuf, &'static str);
/// Size, modification time and digest.
type Digest = (u64, Option<SystemTime>, String);

impl DigestCache {
    pub(crate) fn digest(&self, path: &Path, name: &'static str, kind: HashKind) -> Option<String> {
        let meta = fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
        let stamp = (meta.len(), meta.modified().ok());
        let key = (path.to_path_buf(), name);
        if let Some((len, modified, digest)) =
            self.digests.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
            && (*len, *modified) == stamp
        {
            return Some(digest.clone());
        }
        let digest = hash_file(path, kind).ok()?;
        self.digests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (stamp.0, stamp.1, digest.clone()));
        Some(digest)
    }

    /// The mod id and name the jar at `path` declares.
    pub(crate) fn mod_meta(&self, path: &Path) -> ModMeta {
        let meta = fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
        let stamp = (meta.len(), meta.modified().ok());
        if let Some((len, modified, found)) = self.mods.lock().unwrap_or_else(|e| e.into_inner()).get(path)
            && (*len, *modified) == stamp
        {
            return found.clone();
        }
        let found = inspect_mod_jar(path, None).ok().map(|d| (d.mod_id, d.name));
        self.mods
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_path_buf(), (stamp.0, stamp.1, found.clone()));
        found
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record() -> Record {
        Record {
            kind: ContentKind::Mods,
            filename: "sodium.jar".into(),
            project_id: "AANobbMI".into(),
            project_slug: "sodium".into(),
            project_title: "Sodium".into(),
            version_id: "v2".into(),
            version_number: "0.6.0".into(),
            hash_algorithm: "sha512",
            file_hash: "ab".repeat(64),
        }
    }

    #[test]
    fn a_record_joins_what_is_there() {
        let old = json!({"schema_version": 1, "note": "kept", "files": {
            "mods/other.jar": {"project_id": "x", "project_slug": null},
            "mods/sodium.jar.disabled": {"project_id": "stale"},
            "mods/gone.jar": {"project_id": "g"},
            "mods/gone.jar.disabled": {"project_id": "g"}}});
        let records = [("mods/sodium.jar".to_string(), record())];
        let doc = with_records(old.as_object().cloned(), &["mods/gone.jar.disabled".to_string()], &records);
        assert!(
            doc["files"].get("mods/gone.jar").is_none()
                && doc["files"].get("mods/gone.jar.disabled").is_none()
        );
        assert_eq!(doc["schema_version"], 2);
        assert_eq!(doc["note"], "kept");
        assert_eq!(doc["files"]["mods/other.jar"]["project_slug"], json!(null));
        assert!(doc["files"].get("mods/sodium.jar.disabled").is_none(), "the same file's old record goes");
        assert_eq!(
            doc["files"]["mods/sodium.jar"],
            json!({"content_key": "mods", "filename": "sodium.jar", "project_id": "AANobbMI", "project_slug": "sodium",
                   "project_title": "Sodium", "version_id": "v2", "version_number": "0.6.0",
                   "hash_algorithm": "sha512", "file_hash": "ab".repeat(64)})
        );
        let fresh = with_records(
            None,
            &[],
            &[("shaderpacks/a.zip".to_string(), Record { kind: ContentKind::ShaderPacks, ..record() })],
        );
        assert_eq!(fresh["files"]["shaderpacks/a.zip"]["content_key"], "shaders");
    }
}
