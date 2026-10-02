//! Asset indexes and objects: `assets/indexes/<name>.json`,
//! `assets/objects/<hash[..2]>/<hash>` and, for old versions, `assets/virtual/legacy`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Deserialize;

use crate::net::downloader::{DownloadTask, ExpectedHash, HashKind, hash_file};
use crate::safe_path::safe_relative;

pub fn index_path(assets_dir: &Path, name: &str) -> PathBuf {
    assets_dir.join("indexes").join(format!("{name}.json"))
}

/// `hash` must be 40 hex digits (checked by `AssetIndex::read`).
pub fn object_path(assets_dir: &Path, hash: &str) -> PathBuf {
    assets_dir.join("objects").join(&hash[..2]).join(hash)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: BTreeMap<String, AssetObject>,
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    #[serde(default)]
    pub map_to_resources: bool,
}

fn damaged(path: &Path, why: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("{}: {why}", path.display()))
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

impl AssetIndex {
    /// Reads an index; hashes are lower-cased, and an object with an unsafe name or hash
    /// rejects the whole index.
    pub fn read(path: &Path) -> AppResult<AssetIndex> {
        let bytes = fs::read(path).map_err(|e| io_error(path, e))?;
        let mut index: AssetIndex = serde_json::from_slice(&bytes).map_err(|e| damaged(path, e))?;
        for (name, object) in &mut index.objects {
            if !valid_hash(&object.hash) || safe_relative(name).is_none() {
                return Err(damaged(path, format!("unsafe object {name:?}")));
            }
            object.hash.make_ascii_lowercase();
        }
        Ok(index)
    }

    /// One download per distinct object, from `resources`.
    pub fn object_tasks(&self, assets_dir: &Path, resources: &str) -> Vec<DownloadTask> {
        let base = resources.trim_end_matches('/');
        let mut seen = HashSet::new();
        self.objects
            .values()
            .filter(|object| seen.insert(object.hash.clone()))
            .map(|object| {
                let hash = &object.hash;
                DownloadTask::new(format!("{base}/{}/{hash}", &hash[..2]), object_path(assets_dir, hash))
                    .size(object.size)
                    .hash(ExpectedHash::sha1(hash))
            })
            .collect()
    }

    /// Objects (by name) whose file is missing or has another size — with `deep`, also another
    /// SHA-1 (each distinct object is hashed once).
    pub fn damaged_objects(&self, assets_dir: &Path, deep: bool) -> usize {
        let mut verdicts: HashMap<&str, bool> = HashMap::new();
        self.objects
            .values()
            .filter(|object| {
                !*verdicts.entry(object.hash.as_str()).or_insert_with(|| {
                    let path = object_path(assets_dir, &object.hash);
                    fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == object.size)
                        && (!deep
                            || hash_file(&path, HashKind::Sha1).is_ok_and(|actual| actual == object.hash))
                })
            })
            .count()
    }

    /// Versions with a `virtual` index read assets by name from `assets/virtual/legacy`.
    pub fn materialize_virtual(&self, assets_dir: &Path) -> AppResult<()> {
        if !self.is_virtual {
            return Ok(());
        }
        self.copy_by_name(assets_dir, &assets_dir.join("virtual").join("legacy"))
    }

    /// Versions before 1.6 (`map_to_resources`) read assets by name from `<game>/resources`.
    pub fn materialize_resources(&self, assets_dir: &Path, game_dir: &Path) -> AppResult<()> {
        if !self.map_to_resources {
            return Ok(());
        }
        self.copy_by_name(assets_dir, &game_dir.join("resources"))
    }

    fn copy_by_name(&self, assets_dir: &Path, root: &Path) -> AppResult<()> {
        for (name, object) in &self.objects {
            let Some(relative) = safe_relative(name) else { continue };
            let target = root.join(relative);
            if fs::metadata(&target).is_ok_and(|m| m.len() == object.size) {
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            fs::copy(object_path(assets_dir, &object.hash), &target).map_err(|e| io_error(&target, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha1::{Digest, Sha1};

    fn hash(bytes: &[u8]) -> String {
        hex::encode(Sha1::digest(bytes))
    }

    fn write_index(dir: &Path, index: serde_json::Value) -> PathBuf {
        let path = index_path(dir, "17");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, index.to_string()).unwrap();
        path
    }

    #[test]
    fn an_index_yields_one_download_per_object() {
        let dir = tempfile::tempdir().unwrap();
        let (icon, sound) = (hash(b"png"), hash(b"ogg!"));
        let path = write_index(
            dir.path(),
            json!({"objects": {
                "icons/icon_16x16.png": {"hash": icon, "size": 3},
                "minecraft/sounds/a.ogg": {"hash": sound, "size": 4},
                "minecraft/sounds/a_copy.ogg": {"hash": sound, "size": 4}
            }}),
        );
        let index = AssetIndex::read(&path).unwrap();
        let tasks = index.object_tasks(dir.path(), "https://resources.example/");
        assert_eq!(tasks.len(), 2);
        let task = tasks.iter().find(|t| t.url.ends_with(&sound)).unwrap();
        assert_eq!(task.url, format!("https://resources.example/{}/{sound}", &sound[..2]));
        assert_eq!(task.dest, dir.path().join("objects").join(&sound[..2]).join(&sound));
        assert_eq!((task.size, task.hash.clone()), (Some(4), Some(ExpectedHash::sha1(&sound))));
        assert_eq!(index.damaged_objects(dir.path(), false), 3);
        fs::create_dir_all(object_path(dir.path(), &icon).parent().unwrap()).unwrap();
        fs::write(object_path(dir.path(), &icon), b"png").unwrap();
        assert_eq!(index.damaged_objects(dir.path(), false), 2);
        let sound_path = object_path(dir.path(), &sound);
        fs::create_dir_all(sound_path.parent().unwrap()).unwrap();
        fs::write(&sound_path, b"OGG!").unwrap();
        assert_eq!(index.damaged_objects(dir.path(), false), 0, "right sizes");
        assert_eq!(index.damaged_objects(dir.path(), true), 2, "both names of the wrong sound");
    }

    #[test]
    fn unsafe_indexes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let good = hash(b"x");
        let escaping = write_index(dir.path(), json!({"objects": {"../../evil": {"hash": good, "size": 1}}}));
        assert_eq!(AssetIndex::read(&escaping).unwrap_err().code, ErrorCode::InvalidInput);
        let bad_hash = write_index(dir.path(), json!({"objects": {"a": {"hash": "../zz", "size": 1}}}));
        assert_eq!(AssetIndex::read(&bad_hash).unwrap_err().code, ErrorCode::InvalidInput);
        fs::write(index_path(dir.path(), "17"), "{ broken").unwrap();
        assert_eq!(
            AssetIndex::read(&index_path(dir.path(), "17")).unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn virtual_indexes_are_copied_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let sound = hash(b"ogg!");
        let path = write_index(
            dir.path(),
            json!({"virtual": true, "objects": {"sound/a.ogg": {"hash": sound, "size": 4}}}),
        );
        fs::create_dir_all(object_path(dir.path(), &sound).parent().unwrap()).unwrap();
        fs::write(object_path(dir.path(), &sound), b"ogg!").unwrap();
        let index = AssetIndex::read(&path).unwrap();
        assert!(index.is_virtual);
        index.materialize_virtual(dir.path()).unwrap();
        let copy = dir.path().join("virtual").join("legacy").join("sound").join("a.ogg");
        assert_eq!(fs::read(copy).unwrap(), b"ogg!");
        let plain = AssetIndex { is_virtual: false, ..index };
        plain.materialize_virtual(dir.path()).unwrap();
    }

    #[test]
    fn resource_indexes_are_copied_into_the_game_folder() {
        let dir = tempfile::tempdir().unwrap();
        let sound = hash(b"ogg!");
        let path = write_index(
            dir.path(),
            json!({"map_to_resources": true, "objects": {"sound/a.ogg": {"hash": sound, "size": 4}}}),
        );
        fs::create_dir_all(object_path(dir.path(), &sound).parent().unwrap()).unwrap();
        fs::write(object_path(dir.path(), &sound), b"ogg!").unwrap();
        let game = dir.path().join("game");
        let index = AssetIndex::read(&path).unwrap();
        index.materialize_resources(dir.path(), &game).unwrap();
        assert_eq!(fs::read(game.join("resources").join("sound").join("a.ogg")).unwrap(), b"ogg!");
        let plain = AssetIndex { map_to_resources: false, ..index };
        let other = dir.path().join("other");
        plain.materialize_resources(dir.path(), &other).unwrap();
        assert!(!other.exists());
    }
}
