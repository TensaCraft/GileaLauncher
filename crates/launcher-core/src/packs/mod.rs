//! Modpacks, whichever provider's: what a pack puts in a build (files to download, overrides to
//! take from its archive), checked before anything is written — plain relative paths only, no
//! launcher files, no duplicates, no file under another, within limits — and the record a build
//! keeps of the pack it came from. `engine` installs and updates them; `flow` runs a provider's
//! modpacks through it.

pub mod engine;
pub mod flow;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::net::downloader::{Credential, ExpectedHash, HashKind, hash_reader};
use crate::storage::journal::normalized_path;
use crate::storage::versions::reserved_in_build;

/// How much a pack may hold.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Files of the pack, and override entries.
    pub files: usize,
    /// One override.
    pub entry: u64,
    /// All overrides.
    pub total: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { files: 20_000, entry: 1024 * 1024 * 1024, total: 4 * 1024 * 1024 * 1024 }
    }
}

/// A file the pack downloads.
#[derive(Debug, Clone, PartialEq)]
pub struct PackFile {
    pub path: String,
    pub url: String,
    pub size: u64,
    pub hash: ExpectedHash,
    /// Its SHA-1 when the pack names it: what an update compares with the file the old version put.
    pub sha1: Option<String>,
    /// The provider's project it is a file of, when known: a pack mod whose project the player
    /// already has as a file of their own is not added again by an update.
    pub project: Option<String>,
    /// A secret header its host needs (an API key).
    pub credential: Option<Credential>,
    /// A copy already on this machine (one the user downloaded by hand): taken instead of
    /// downloading.
    pub local: Option<PathBuf>,
}

/// A file the pack carries: an entry of the archive and its place in the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub entry: String,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub name: String,
    pub version: String,
    pub summary: String,
    pub minecraft: String,
    /// `None` for plain Minecraft.
    pub loader: Option<(LoaderKind, String)>,
    pub files: Vec<PackFile>,
    pub overrides: Vec<Override>,
}

pub fn invalid(what: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidInput, what.into())
}

/// `raw` as a plain relative path of the build that is not the launcher's own.
pub fn place(raw: &str) -> AppResult<String> {
    let bad = || invalid(format!("unsafe path in the pack: {raw:?}")).with_param("name", raw);
    if raw.contains(['\\', '\0']) {
        return Err(bad());
    }
    let path = normalized_path(raw).map_err(|_| bad())?;
    if reserved_in_build(&path) {
        return Err(bad());
    }
    Ok(path)
}

/// The archive's overrides: the files under each of `prefixes` (`overrides/`), the later over
/// the earlier, by their place in the build.
pub fn overrides_of<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    prefixes: &[&str],
    limits: Limits,
) -> AppResult<Vec<Override>> {
    let mut by_path: BTreeMap<String, Override> = BTreeMap::new();
    let mut entries = 0;
    for prefix in prefixes {
        for i in 0..zip.len() {
            let entry = zip.by_index(i).map_err(|e| invalid(e.to_string()))?;
            let name = entry.name().to_string();
            let Some(rest) = name.strip_prefix(prefix) else { continue };
            if entry.is_dir() || rest.is_empty() {
                continue;
            }
            entries += 1;
            if entries > limits.files || entry.is_symlink() || entry.size() > limits.entry {
                return Err(
                    invalid(format!("override {name} is not allowed")).with_param("name", name.clone())
                );
            }
            let path = place(rest)?;
            by_path.insert(path.to_ascii_lowercase(), Override { entry: name, path, size: entry.size() });
        }
    }
    let total: u64 = by_path.values().map(|o| o.size).sum();
    if total > limits.total {
        return Err(invalid(format!("the overrides take {total} bytes")));
    }
    Ok(by_path.into_values().collect())
}

/// One file per place (whatever the case), and no file where another has a folder.
pub fn check_places(files: &[PackFile], overrides: &[Override]) -> AppResult<()> {
    let mut seen: HashSet<String> = HashSet::new();
    for path in files.iter().map(|f| &f.path).chain(overrides.iter().map(|o| &o.path)) {
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(invalid(format!("{path} twice in the pack")).with_param("name", path.clone()));
        }
    }
    for path in &seen {
        let mut rest = path.as_str();
        while let Some((up, _)) = rest.rsplit_once('/') {
            if seen.contains(up) {
                return Err(
                    invalid(format!("{up} is both a file and a folder")).with_param("name", up.to_string())
                );
            }
            rest = up;
        }
    }
    Ok(())
}

/// Writes each override from `archive` to its staged file, exactly the size the pack said.
pub fn extract(archive: &Path, targets: &[(Override, PathBuf)], progress: &dyn Fn(u64)) -> AppResult<()> {
    let io_err = |e: io::Error| AppError::new(ErrorCode::Io, e.to_string());
    let mut zip =
        ZipArchive::new(File::open(archive).map_err(io_err)?).map_err(|e| invalid(e.to_string()))?;
    let mut done = 0u64;
    for (item, dest) in targets {
        let mut entry = zip.by_name(&item.entry).map_err(|e| invalid(e.to_string()))?;
        if entry.is_symlink() || entry.size() != item.size {
            return Err(invalid(format!("{} changed in the pack", item.entry)));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let mut out = File::create(dest).map_err(io_err)?;
        let written = io::copy(&mut (&mut entry).take(item.size + 1), &mut out).map_err(io_err)?;
        if written != item.size {
            let _ = fs::remove_file(dest);
            return Err(invalid(format!("{} is not {} bytes", item.entry, item.size)));
        }
        done = done.saturating_add(item.size);
        progress(done);
    }
    Ok(())
}

/// The SHA-1 of each of `overrides` as `archive` carries it, by its place in the build.
pub fn override_sha1s(archive: &Path, overrides: &[Override]) -> AppResult<HashMap<String, String>> {
    let io_err = |e: io::Error| AppError::new(ErrorCode::Io, e.to_string());
    let mut zip =
        ZipArchive::new(File::open(archive).map_err(io_err)?).map_err(|e| invalid(e.to_string()))?;
    let mut found = HashMap::new();
    for item in overrides {
        let entry = zip.by_name(&item.entry).map_err(|e| invalid(e.to_string()))?;
        found.insert(
            item.path.clone(),
            hash_reader(&mut entry.take(item.size), HashKind::Sha1).map_err(io_err)?,
        );
    }
    Ok(found)
}

/// What a build installed from a modpack keeps of it (the provider's record file in the build).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRecord {
    pub schema_version: u32,
    pub project_id: String,
    pub version_id: String,
    #[serde(default)]
    pub version_number: String,
    /// Schema 2: what the pack's files run on — Minecraft, the loader component, its version, the
    /// client's name — and the Java its Minecraft installed.
    #[serde(default)]
    pub minecraft: Option<String>,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub loader_version: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub java: Option<String>,
    pub managed_files: Vec<String>,
    /// Schema 3: the SHA-1 of each file as the pack put it — a file whose bytes differ since is
    /// the player's, and an update leaves it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hashes: BTreeMap<String, String>,
}

/// At most this many bytes and files of a record are read (the original's limits).
const RECORD_MAX_BYTES: u64 = 16 * 1024 * 1024;
const RECORD_MAX_FILES: usize = 40_000;

impl PackRecord {
    /// The record in `game` at `record` (a path of the build), when it is one the launcher can use.
    pub fn read(game: &Path, record: &str) -> Option<PackRecord> {
        let path = game.join(record);
        if fs::metadata(&path).ok()?.len() > RECORD_MAX_BYTES {
            return None;
        }
        let record: PackRecord = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        (matches!(record.schema_version, 1..=3) && record.managed_files.len() <= RECORD_MAX_FILES)
            .then_some(record)
    }

    /// The files `pack` puts in a build.
    pub fn managed(pack: &Pack) -> Vec<String> {
        pack.files
            .iter()
            .map(|f| f.path.clone())
            .chain(pack.overrides.iter().map(|o| o.path.clone()))
            .collect()
    }
}
