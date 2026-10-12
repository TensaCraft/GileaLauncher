//! A build's files under a group's paths: their size, time and hash, a hash read again only
//! when the size or time moved since the last pass.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use super::blacklist::blocked;
use super::paths::key;
use super::store::FileRecord;

/// A file of a build under a group's paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// As the build spells it, relative to its folder.
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
    /// SHA-256, in hex.
    pub hash: String,
}

pub fn sha256(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// `relative` is `managed`, or inside one of them.
pub(crate) fn is_managed(relative: &str, managed: &[String]) -> bool {
    let relative = key(relative);
    managed.iter().map(|m| key(m)).any(|m| relative == m || relative.starts_with(&format!("{m}/")))
}

/// `path` (relative, `/`-separated) as the folder `game` spells it: each part as its entry is
/// named (Windows finds a name in any case); a part not there yet is kept as written.
fn spelled(game: &Path, path: &str) -> String {
    let mut dir = game.to_path_buf();
    let mut parts = Vec::new();
    for part in path.split('/') {
        let real = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|name| key(name) == key(part))
            .unwrap_or_else(|| part.to_string());
        dir.push(&real);
        parts.push(real);
    }
    parts.join("/")
}

/// The files of `build` (its folder `game`) under `paths`: folders walked (links not followed),
/// files taken as they are, the blocked and the `managed` left out. A file whose size and time
/// are those `previous` holds for this build keeps its hash; any other is hashed.
pub fn scan(
    game: &Path,
    paths: &[String],
    managed: &[String],
    previous: &BTreeMap<String, FileRecord>,
    build: &str,
) -> BTreeMap<String, Found> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for path in paths {
        let path = spelled(game, path);
        let full = game.join(&path);
        let Ok(meta) = fs::symlink_metadata(&full) else { continue };
        if meta.is_file() {
            files.push((path, full));
        } else if meta.is_dir() {
            walk(&full, &path, &mut files);
        }
    }
    let mut found = BTreeMap::new();
    for (relative, full) in files {
        if blocked(&relative) || is_managed(&relative, managed) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&full) else { continue };
        let (size, modified_ms) = (meta.len(), modified_ms(&meta));
        let k = key(&relative);
        let known = previous
            .get(&k)
            .and_then(|record| record.held.get(build))
            .filter(|seen| seen.size == size && seen.modified_ms == modified_ms)
            .map(|seen| seen.hash.clone());
        let hash = match known {
            Some(hash) => hash,
            None => match sha256(&full) {
                Ok(hash) => hash,
                Err(e) => {
                    tracing::warn!("Unable to read {}: {e}", full.display());
                    continue;
                }
            },
        };
        found.insert(k, Found { path: relative, size, modified_ms, hash });
    }
    found
}

/// The files below `dir` (relative `prefix`), links not followed.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}/{name}");
        if kind.is_dir() {
            if !blocked(&relative) {
                walk(&entry.path(), &relative, out);
            }
        } else if kind.is_file() {
            out.push((relative, entry.path()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::backend::store::{FileRecord, Seen};

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn keys(found: &BTreeMap<String, Found>) -> Vec<&str> {
        found.keys().map(String::as_str).collect()
    }

    #[test]
    fn folders_files_and_what_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path();
        put(game, "options.txt", b"keys");
        put(game, "xaero/minimap.txt", b"map");
        put(game, "xaero/waypoints/w1.txt", b"home");
        put(game, "xaero/cache.tmp", b"leftover");
        put(game, "config/jei/x.json", b"{}");
        put(game, "config/server/a.toml", b"the server's");
        let paths = ["options.txt", "xaero", "config", "servers.dat"].map(String::from);
        let found = scan(game, &paths, &["config/server".to_string()], &BTreeMap::new(), "aero");
        assert_eq!(
            keys(&found),
            vec!["config/jei/x.json", "options.txt", "xaero/minimap.txt", "xaero/waypoints/w1.txt"]
        );
        let options = &found["options.txt"];
        assert_eq!((options.path.as_str(), options.size), ("options.txt", 4));
        assert_eq!(options.hash.len(), 64, "SHA-256 in hex");
    }

    #[test]
    fn an_unchanged_file_is_not_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path();
        put(game, "options.txt", b"aaaa");
        let paths = ["options.txt".to_string()];
        let first = scan(game, &paths, &[], &BTreeMap::new(), "aero").remove("options.txt").unwrap();
        let mut previous = BTreeMap::new();
        let mut record = FileRecord { path: first.path.clone(), ..FileRecord::default() };
        record.held.insert(
            "aero".into(),
            Seen { hash: first.hash.clone(), size: first.size, modified_ms: first.modified_ms },
        );
        previous.insert("options.txt".to_string(), record);
        // Other bytes behind the same size and time: taken as unchanged, not read.
        let at = fs::metadata(game.join("options.txt")).unwrap().modified().unwrap();
        fs::write(game.join("options.txt"), b"bbbb").unwrap();
        fs::File::options().write(true).open(game.join("options.txt")).unwrap().set_modified(at).unwrap();
        assert_eq!(scan(game, &paths, &[], &previous, "aero")["options.txt"].hash, first.hash);
        assert_ne!(
            scan(game, &paths, &[], &previous, "lite")["options.txt"].hash,
            first.hash,
            "another build's record"
        );
        let later = at + Duration::from_secs(5);
        fs::File::options().write(true).open(game.join("options.txt")).unwrap().set_modified(later).unwrap();
        assert_ne!(
            scan(game, &paths, &[], &previous, "aero")["options.txt"].hash,
            first.hash,
            "its time moved"
        );
    }

    #[cfg(windows)]
    #[test]
    fn case_only_differences_are_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path();
        put(game, "Options.TXT", b"keys");
        put(game, "Xaero/Minimap.txt", b"map");
        let paths = ["options.txt".to_string(), "xaero".to_string()];
        let found = scan(game, &paths, &[], &BTreeMap::new(), "aero");
        assert_eq!(keys(&found), vec!["options.txt", "xaero/minimap.txt"]);
        assert_eq!(found["options.txt"].path, "Options.TXT", "written as the source spells it");
        assert_eq!(found["xaero/minimap.txt"].path, "Xaero/Minimap.txt");
    }
}
