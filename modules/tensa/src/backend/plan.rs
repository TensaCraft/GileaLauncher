//! The plan that brings a server build's folder in line with the server: which
//! files the server manages, which of them to download, and which files of its managed folders
//! are stale. No file of the build is changed here (only the launcher's note of the hashes it
//! read); applying the plan is a file transaction.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use launcher_core::net::downloader::{DownloadTask, ExpectedHash, HashKind, hash_file};
use launcher_core::storage::atomic::atomic_write;
use launcher_core::storage::journal::{contained, normalized_path};
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Map, Value, json};

use super::api::TensaApi;
use super::manifest::{
    PreserveRule, expected_hash, launchers_own, preserve_rules, preserved, relative_path, safe_relative,
    size, under, validate,
};
use super::pack::{Pack, truthy};

const FORCE_KEYS: [&str; 2] = ["force_update", "forceUpdate"];
const LOCKED_KEYS: [&str; 2] = ["force_update_locked", "forceUpdateLocked"];
const SCOPE_KEYS: [&str; 6] =
    ["force_update_scope", "forceUpdateScope", "sync_root", "syncRoot", "sync_directory", "syncDirectory"];
const MARKER_KEYS: [&str; 4] =
    ["force_update_directory", "forceUpdateDirectory", "force_update_folder", "forceUpdateFolder"];

/// A file to download into the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Its path in the build (`mods/a.jar`).
    pub relative: String,
    pub url: String,
    pub size: Option<u64>,
    pub hash: Option<ExpectedHash>,
}

impl PlannedFile {
    /// Its download to `dest` (a staged path).
    pub fn task(&self, dest: PathBuf) -> DownloadTask {
        let mut task = DownloadTask::new(self.url.clone(), dest);
        task.size = self.size;
        task.hash = self.hash.clone();
        task
    }
}

/// What a sync changes in a build's folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// The folders the server owns, sorted: their files the server does not list are stale.
    pub managed_dirs: Vec<String>,
    pub downloads: Vec<PlannedFile>,
    /// Files to delete, relative to the build's folder.
    pub stale: Vec<String>,
    /// Every managed file was planned, changed or not.
    pub force: bool,
}

impl SyncPlan {
    pub fn has_changes(&self) -> bool {
        !self.downloads.is_empty() || !self.stale.is_empty()
    }
}

/// What the server listed: a force-update manifest, or the older list of files.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Manifest(Value),
    Files(Vec<Value>),
}

fn any_truthy(file: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| file.get(*key).is_some_and(truthy))
}

fn has_force_metadata(file: &Value) -> bool {
    FORCE_KEYS
        .iter()
        .chain(&LOCKED_KEYS)
        .chain(&SCOPE_KEYS)
        .chain(&MARKER_KEYS)
        .any(|key| file.get(*key).is_some())
}

fn text<'a>(file: &'a Value, key: &str) -> Option<&'a str> {
    file.get(key).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
}

/// The folder a file is in: its `path`, else the parent of its path.
fn file_dir(file: &Value, relative: Option<&str>) -> Option<String> {
    text(file, "path")
        .and_then(safe_relative)
        .or_else(|| relative.and_then(|r| r.rsplit_once('/')).map(|(parent, _)| parent.to_string()))
}

/// The folder a file says the server manages, if it names one.
fn explicit_scope(file: &Value, relative: Option<&str>) -> Option<String> {
    if let Some(scope) = SCOPE_KEYS.iter().find_map(|key| text(file, key)) {
        return safe_relative(scope);
    }
    if let Some(folder) = text(file, "force_update_directory").or_else(|| text(file, "forceUpdateDirectory"))
    {
        return safe_relative(folder);
    }
    if any_truthy(file, &MARKER_KEYS) {
        return file_dir(file, relative);
    }
    if text(file, "sync_scope").is_some_and(|s| s.eq_ignore_ascii_case("directory")) {
        return file_dir(file, relative);
    }
    let folder_type = text(file, "type").map(str::to_lowercase);
    if matches!(folder_type.as_deref(), Some("directory" | "folder" | "dir")) {
        return relative.map(str::to_string).or_else(|| file_dir(file, relative));
    }
    let raw = text(file, "relative_path").map(|r| r.replace('\\', "/")).unwrap_or_default();
    if raw.ends_with('/') && any_truthy(file, &FORCE_KEYS) {
        return safe_relative(&raw);
    }
    if text(file, "download_url").is_none() && any_truthy(file, &FORCE_KEYS) {
        return relative.map(str::to_string).or_else(|| file_dir(file, relative));
    }
    None
}

struct Entry<'a> {
    file: &'a Value,
    relative: Option<String>,
    forced: bool,
    locked: bool,
    downloadable: bool,
}

/// Folders every downloadable file of which is forced, and locked too unless it is `mods`.
fn inferred_scopes(entries: &[Entry<'_>]) -> BTreeSet<String> {
    let mut by_dir: BTreeMap<String, Vec<&Entry<'_>>> = BTreeMap::new();
    for entry in entries.iter().filter(|e| e.relative.is_some() && e.downloadable) {
        if let Some(dir) = file_dir(entry.file, entry.relative.as_deref()) {
            by_dir.entry(dir).or_default().push(entry);
        }
    }
    by_dir
        .into_iter()
        .filter(|(dir, entries)| {
            entries.iter().all(|e| e.forced) && (entries.iter().all(|e| e.locked) || dir == "mods")
        })
        .map(|(dir, _)| dir)
        .collect()
}

/// The launcher's note, in the build, of the hashes it read of the managed files: a file whose
/// size and time stay is not hashed again (each launch would read gigabytes).
pub const CHECKED: &str = ".launcher-tensa-checked.json";

/// The hashes noted before (`known`) and those of this plan (`seen`), by path.
struct Checked {
    path: PathBuf,
    known: Map<String, Value>,
    seen: Map<String, Value>,
}

impl Checked {
    fn open(root: &Path) -> Checked {
        let path = root.join(CHECKED);
        let known = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|note| note.get("files").and_then(Value::as_object).cloned())
            .unwrap_or_default();
        Checked { path, known, seen: Map::new() }
    }

    /// The hash of `kind` of the file at `dest`: the noted one while its size and time stay, else
    /// read now.
    fn hash(&mut self, relative: &str, dest: &Path, meta: &fs::Metadata, kind: HashKind) -> Option<String> {
        let name = match kind {
            HashKind::Sha1 => "sha1",
            HashKind::Sha256 => "sha256",
            HashKind::Sha512 => "sha512",
        };
        let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        let Some(modified_ms) = modified.map(|d| d.as_millis() as u64) else {
            return hash_file(dest, kind).ok();
        };
        let mut note = json!({"size": meta.len(), "modified_ms": modified_ms});
        let noted = self
            .known
            .get(relative)
            .filter(|n| n["size"] == note["size"] && n["modified_ms"] == note["modified_ms"])
            .and_then(|n| n[name].as_str())
            .map(str::to_string);
        let hex = match noted {
            Some(hex) => hex,
            None => hash_file(dest, kind).ok()?,
        };
        note[name] = json!(hex);
        self.seen.insert(relative.to_string(), note);
        Some(hex)
    }

    /// Keeps the hashes of this plan (the files no longer listed go), when they changed.
    fn save(self) {
        if self.seen == self.known {
            return;
        }
        let note = json!({"files": self.seen});
        if let Err(e) = atomic_write(&self.path, note.to_string().as_bytes()) {
            tracing::debug!("No note of the hashes read in {}: {e}", self.path.display());
        }
    }
}

/// The file is on disk as the server lists it: its size (when known) and hash (when known).
fn unchanged(dest: &Path, file: &PlannedFile, checked: &mut Checked) -> bool {
    let Ok(meta) = fs::metadata(dest) else { return false };
    if !meta.is_file() || file.size.is_some_and(|size| size != meta.len()) {
        return false;
    }
    file.hash.as_ref().is_none_or(|hash| {
        checked.hash(&file.relative, dest, &meta, hash.kind).is_some_and(|hex| hex == hash.hex)
    })
}

/// A listed path, compared as the file system compares names (case-blind on Windows and macOS).
fn name_key(path: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) { path.to_lowercase() } else { path.to_string() }
}

/// `relative` can be reached in `root` without passing a link (a symbolic link or a junction):
/// the build's file transaction refuses anything else.
fn reachable(root: &Path, relative: &str) -> bool {
    let reachable = contained(root, relative).is_ok();
    if !reachable {
        tracing::warn!("Leaving {relative} alone: it is behind a link");
    }
    reachable
}

/// Every file under `dir` (links not followed), as paths relative to `root`; a name the build's
/// file transaction refuses is left alone.
fn files_under(root: &Path, dir: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            files_under(root, &path, found);
        } else if kind.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            let parts: Vec<String> =
                relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let relative = parts.join("/");
            if normalized_path(&relative).is_ok() {
                found.push(relative);
            } else {
                tracing::warn!("Leaving {relative} alone: its name cannot be managed");
            }
        }
    }
}

fn stale_files(
    root: &Path,
    managed: &BTreeSet<String>,
    expected: &HashSet<String>,
    rules: &[PreserveRule],
) -> Vec<String> {
    let mut stale = Vec::new();
    let mut seen = HashSet::new();
    for dir in managed {
        let path = root.join(dir);
        if !path.is_dir() {
            continue;
        }
        let mut found = Vec::new();
        files_under(root, &path, &mut found);
        found.sort();
        for relative in found {
            if !preserved(&relative, rules)
                && !launchers_own(&relative)
                && !expected.contains(&name_key(&relative))
                && seen.insert(relative.clone())
            {
                stale.push(relative);
            }
        }
    }
    stale
}

/// What to change in `root` (a build's folder) to match `source`; `force` plans every managed
/// file again, changed or not.
pub fn plan(root: &Path, source: &Source, preserve: &[PreserveRule], force: bool) -> SyncPlan {
    let (files, manifest_mode, mut managed): (&[Value], bool, BTreeSet<String>) = match source {
        Source::Manifest(manifest) => {
            let directories =
                manifest.get("directories").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let managed = directories
                .iter()
                .filter(|d| text(d, "sync_scope").is_none_or(|s| s.eq_ignore_ascii_case("directory")))
                .filter_map(|d| text(d, "path").and_then(safe_relative))
                .collect();
            let files =
                manifest.get("files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            (files, true, managed)
        }
        Source::Files(files) => (files.as_slice(), false, BTreeSet::new()),
    };
    let mut has_metadata = manifest_mode;
    let mut entries = Vec::new();
    for file in files.iter().filter(|f| f.is_object()) {
        let relative = relative_path(file);
        has_metadata |= has_force_metadata(file);
        if let Some(scope) = explicit_scope(file, relative.as_deref()) {
            managed.insert(scope);
        }
        entries.push(Entry {
            file,
            relative,
            forced: any_truthy(file, &FORCE_KEYS),
            locked: any_truthy(file, &LOCKED_KEYS),
            downloadable: text(file, "download_url").is_some(),
        });
    }
    if has_metadata && !manifest_mode {
        managed.extend(inferred_scopes(&entries));
    }
    managed.retain(|dir| reachable(root, dir));
    let mut expected = HashSet::new();
    let mut downloads = Vec::new();
    let mut checked = Checked::open(root);
    for entry in &entries {
        let (Some(relative), true) = (&entry.relative, entry.downloadable) else { continue };
        let is_managed = if manifest_mode {
            true
        } else if has_metadata {
            entry.forced || managed.iter().any(|dir| under(relative, dir))
        } else {
            relative.to_lowercase().starts_with("mods/")
        };
        if !is_managed || !reachable(root, relative) {
            continue;
        }
        expected.insert(name_key(relative));
        let planned = PlannedFile {
            relative: relative.clone(),
            url: text(entry.file, "download_url").unwrap_or_default().to_string(),
            size: size(entry.file),
            hash: expected_hash(entry.file),
        };
        if force || !unchanged(&root.join(relative), &planned, &mut checked) {
            downloads.push(planned);
        }
    }
    checked.save();
    let stale = stale_files(root, &managed, &expected, preserve);
    SyncPlan { managed_dirs: managed.into_iter().collect(), downloads, stale, force }
}

/// Plans a sync of `pack` into `root` from what the server lists: its force-update manifest when
/// it is whole, else its older list of files — unless the build names its force-update endpoint.
/// The server unavailable is a `Network` error naming the build (`pack`).
pub async fn prepare(api: &TensaApi, pack: &Pack, root: &Path, force: bool) -> AppResult<SyncPlan> {
    planned(listed(api, pack).await?, pack, root, force).await
}

/// What the server lists of `pack` (the asking part of `prepare`).
pub async fn listed(api: &TensaApi, pack: &Pack) -> AppResult<Source> {
    let unavailable = |why: AppError| {
        tracing::warn!("The server build {} is unavailable: {}", pack.id, why.detail);
        AppError::new(
            ErrorCode::Network,
            format!("the server build {} is unavailable: {}", pack.id, why.detail),
        )
        .with_param("pack", &pack.id)
    };
    let manifest = match api.force_manifest(&pack.id, pack.force_endpoint.as_deref()).await {
        Ok(manifest) => validate(&manifest).map(|()| manifest),
        Err(e) => Err(e),
    };
    let source = match manifest {
        Ok(manifest) => Source::Manifest(manifest),
        Err(e) if pack.force_endpoint.is_some() => return Err(unavailable(e)),
        Err(e) => {
            tracing::info!(
                "No whole force-update manifest for {} ({}); using its list of files",
                pack.id,
                e.detail
            );
            Source::Files(api.files(&pack.id, pack.files_endpoint.as_deref()).await.map_err(unavailable)?)
        }
    };
    Ok(source)
}

/// The sync of `pack` into `root` from what the server listed (the files' part of `prepare`).
pub async fn planned(source: Source, pack: &Pack, root: &Path, force: bool) -> AppResult<SyncPlan> {
    let own_rules = pack.preserve_rules.as_ref().filter(|rules| !rules.is_null());
    let rules = preserve_rules(own_rules.or(match &source {
        Source::Manifest(manifest) => manifest.get("preserve_rules"),
        Source::Files(_) => None,
    }));
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || plan(&root, &source, &rules, force))
        .await
        .map_err(|e| AppError::internal(e.to_string()))
}
