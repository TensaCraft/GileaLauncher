//! Build records: every build keeps its record in its own folder,
//! `<minecraft>/games/<id>/version.json`, with the schema of the original's `versions.json`
//! entries. Minecraft itself (versions, libraries, assets, runtimes) stays shared in the Minecraft
//! folder; `games/` holds only the instances. A save re-reads the record and writes only the keys
//! this copy of the build changed since it was read, so edits made meanwhile (by another copy or
//! another launcher) survive. An old `versions.json` in the state folder is imported once.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Map, Value, json};

use super::json::{JsonRead, backup_corrupt_file, read_json_object, write_json_file};
use crate::builds::ids::{new_build_id, normalize_string};
use crate::lock::path_key;

pub const VERSIONS_FILE: &str = "versions.json";
/// The record in each build folder.
pub const RECORD_FILE: &str = "version.json";
/// Whether `path` (relative, `/`-separated) is one the launcher keeps in a build folder — the
/// build's record and everything named `.launcher…` — so content from outside never lands there.
pub fn reserved_in_build(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower == RECORD_FILE
        || lower == ".launcher"
        || lower.starts_with(".launcher/")
        || lower.starts_with(".launcher-")
}

/// An imported `versions.json` is kept under this suffix.
pub const IMPORTED_SUFFIX: &str = ".imported";
/// The folder of build folders inside the Minecraft folder.
pub const GAMES_DIR: &str = "games";
const CREATE_ATTEMPTS: usize = 8;
/// In a new build's folder while its files are put in place: the launcher that claimed it (`pid`,
/// and when that process started), so a folder an install that never finished left is told apart.
pub const CLAIM_FILE: &str = ".launcher-claim.json";
/// A folder being removed is first renamed with this prefix: hidden from the list at once.
const REMOVING_PREFIX: &str = ".removing-";

/// Marks `folder` as claimed by this launcher.
fn write_claim(folder: &Path) {
    let me = std::process::id();
    let claim = json!({"pid": me, "started": crate::launch::ledger::process_started(me)});
    if let Err(e) = fs::write(folder.join(CLAIM_FILE), claim.to_string()) {
        tracing::warn!("Cannot mark {} as claimed: {e}", folder.display());
    }
}

/// The claim in `folder` was made by a launcher that runs no more (its process gone, or the id
/// now another process's). A folder with no claim, or one being written, is nobody's to clear.
fn claim_abandoned(folder: &Path) -> bool {
    let Ok(raw) = fs::read(folder.join(CLAIM_FILE)) else { return false };
    let Ok(claim) = serde_json::from_slice::<Value>(&raw) else { return false };
    let Some(pid) = claim["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()) else { return false };
    match crate::launch::ledger::process_started(pid) {
        None => true,
        Some(started) => claim["started"].as_str().is_some_and(|claimed| claimed != started),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The record a copy of a build was read as (`None` for a build not yet in the registry).
/// Copies compare by content, so it never takes part in `==`.
#[derive(Debug, Clone, Default)]
struct Snapshot(Option<Map<String, Value>>);

impl PartialEq for Snapshot {
    fn eq(&self, _: &Snapshot) -> bool {
        true
    }
}

/// One build, as the original's `Version` class holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    /// Key in `versions.json` (a raw name for old records, the version id for new ones).
    pub key: String,
    /// `normalize_string(key)`: folder names and shortcuts use it.
    pub version_id: String,
    pub id: String,
    pub name: String,
    /// Minecraft version.
    pub version: Option<String>,
    /// Installed component id (`1.21.1`, `fabric-loader-…`, `neoforge-…`).
    pub loader: Option<String>,
    pub client: Option<String>,
    /// Game folder, absolute or relative to the Minecraft folder.
    pub path: Option<String>,
    pub loader_version: Option<String>,
    pub force_update: bool,
    pub options: Map<String, Value>,
    pub image: Option<String>,
    pub is_remote: bool,
    pub remote_pack_id: Option<Value>,
    pub description: String,
    snapshot: Snapshot,
}

fn with_gpu_default(mut options: Map<String, Value>) -> Map<String, Value> {
    if !options.contains_key("gpuMode") {
        let mode = crate::java::gpu::platform_default(crate::paths::Os::current());
        options.insert("gpuMode".to_string(), json!(mode.as_str()));
    }
    options
}

impl Build {
    /// A new build named `name`, keyed by its version id (`VersionStore::create` makes it unique).
    pub fn new(name: &str) -> Build {
        let version_id = new_build_id(name, |_| false);
        Build {
            key: version_id.clone(),
            id: version_id.clone(),
            name: name.to_string(),
            version_id,
            version: None,
            loader: None,
            client: None,
            path: None,
            loader_version: None,
            force_update: false,
            options: with_gpu_default(Map::new()),
            image: None,
            is_remote: false,
            remote_pack_id: None,
            description: String::new(),
            snapshot: Snapshot(None),
        }
    }

    pub fn from_record(key: &str, record: &Map<String, Value>) -> Build {
        let text = |field: &str| record.get(field).and_then(Value::as_str).map(str::to_string);
        let version_id = normalize_string(key);
        let options = match record.get("options") {
            Some(Value::Object(options)) => options.clone(),
            _ => Map::new(),
        };
        let loader = text("loader");
        Build {
            key: key.to_string(),
            id: text("id").unwrap_or_else(|| version_id.clone()),
            name: text("name").unwrap_or_else(|| version_id.clone()),
            version: text("version"),
            client: text("client").filter(|c| !c.is_empty()).or_else(|| loader.clone()),
            loader,
            path: text("path"),
            loader_version: text("loader_version"),
            force_update: record.get("force_update").is_some_and(truthy),
            options: with_gpu_default(options),
            image: text("image"),
            is_remote: record.get("is_remote").is_some_and(truthy),
            remote_pack_id: record.get("remote_pack_id").cloned().filter(|v| !v.is_null()),
            description: match record.get("description") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            },
            version_id,
            snapshot: Snapshot(None),
        }
    }

    /// A copy bound to `record`, the content of build folder `folder` (named `key`); `path` is
    /// that folder.
    fn loaded(key: &str, record: &Map<String, Value>, folder: &Path) -> Build {
        let mut build = Build::from_record(key, record);
        build.path = Some(folder.to_string_lossy().into_owned());
        let mut snapshot = build.to_record();
        if !matches!(record.get("options"), Some(Value::Object(_)) | None) {
            // Broken options on disk: the defaults we use are a change to write back.
            snapshot.remove("options");
        }
        build.snapshot = Snapshot(Some(snapshot));
        build
    }

    /// The 13 keys the original writes (`Version.to_dict`).
    pub fn to_record(&self) -> Map<String, Value> {
        let text = |v: &Option<String>| v.clone().map(Value::String).unwrap_or(Value::Null);
        Map::from_iter(
            [
                ("id", json!(self.id)),
                ("name", json!(self.name)),
                ("version", text(&self.version)),
                ("loader", text(&self.loader)),
                ("client", text(&self.client)),
                ("path", text(&self.path)),
                ("loader_version", text(&self.loader_version)),
                ("force_update", json!(self.force_update)),
                ("options", Value::Object(self.options.clone())),
                ("image", text(&self.image)),
                ("is_remote", json!(self.is_remote)),
                ("remote_pack_id", self.remote_pack_id.clone().unwrap_or(Value::Null)),
                ("description", json!(self.description)),
            ]
            .map(|(k, v)| (k.to_string(), v)),
        )
    }
}

/// Three-way merge of one record: keys `current` changed relative to `baseline` are applied to
/// `persisted`; nested objects merge recursively; keys removed since `baseline` are removed.
pub fn merge_changes(
    current: &Map<String, Value>,
    baseline: &Map<String, Value>,
    persisted: &Map<String, Value>,
) -> Map<String, Value> {
    let mut result = persisted.clone();
    for key in baseline.keys() {
        if !current.contains_key(key) {
            result.remove(key);
        }
    }
    for (key, value) in current {
        if baseline.get(key) == Some(value) {
            continue;
        }
        let merged = match (value, result.get(key)) {
            (Value::Object(changed), Some(Value::Object(existing))) => {
                let previous = match baseline.get(key) {
                    Some(Value::Object(previous)) => previous.clone(),
                    _ => Map::new(),
                };
                Value::Object(merge_changes(changed, &previous, existing))
            }
            _ => value.clone(),
        };
        result.insert(key.clone(), merged);
    }
    result
}

/// What a record file holds.
enum RecordRead {
    Missing,
    Record(Map<String, Value>),
    Unreadable,
}

fn read_record(file: &Path) -> RecordRead {
    match read_json_object(file) {
        Ok(JsonRead::Object(map)) => RecordRead::Record(map),
        Ok(JsonRead::Missing) => RecordRead::Missing,
        Ok(JsonRead::Invalid(reason)) => {
            tracing::warn!("{} is unreadable ({reason})", file.display());
            RecordRead::Unreadable
        }
        Err(e) => {
            tracing::warn!("Unable to read {}: {e}", file.display());
            RecordRead::Unreadable
        }
    }
}

/// One plain folder name (no separators, `.` or `..`).
fn is_folder_name(key: &str) -> bool {
    let mut parts = Path::new(key).components();
    matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None)) && !key.contains(['/', '\\'])
}

fn io_error(e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

fn folder_error(e: io::Error, path: &Path) -> AppError {
    AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string()).with_param("path", path.to_string_lossy())
}

pub struct VersionStore {
    minecraft_dir: PathBuf,
    games_dir: PathBuf,
    /// Serializes this launcher's writes; other launchers are handled by the merge.
    writes: Mutex<()>,
}

impl VersionStore {
    /// The builds in `<minecraft_dir>/games`; an old `<state_dir>/versions.json` is imported first.
    pub fn open(state_dir: &Path, minecraft_dir: &Path) -> VersionStore {
        let store = VersionStore {
            minecraft_dir: minecraft_dir.to_path_buf(),
            games_dir: minecraft_dir.join(GAMES_DIR),
            writes: Mutex::new(()),
        };
        store.import_registry(&state_dir.join(VERSIONS_FILE));
        store
    }

    fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.writes.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The Minecraft folder: shared versions, libraries, assets and runtimes.
    pub fn minecraft_dir(&self) -> &Path {
        &self.minecraft_dir
    }

    /// The folder holding one folder per build.
    pub fn games_dir(&self) -> &Path {
        &self.games_dir
    }

    fn load(&self, key: &str) -> Option<Build> {
        if !is_folder_name(key) {
            return None;
        }
        let folder = self.games_dir.join(key);
        match read_record(&folder.join(RECORD_FILE)) {
            RecordRead::Record(record) => Some(Build::loaded(key, &record, &folder)),
            RecordRead::Missing | RecordRead::Unreadable => None,
        }
    }

    /// Every build: the folders in `games/` with a readable record, by folder name. Links and
    /// hidden folders are skipped.
    pub fn list(&self) -> Vec<Build> {
        let Ok(entries) = fs::read_dir(&self.games_dir) else { return Vec::new() };
        let mut keys: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        keys.sort();
        keys.iter().filter_map(|key| self.load(key)).collect()
    }

    /// By folder name, then by version id or record id.
    pub fn get(&self, id: &str) -> Option<Build> {
        if id.starts_with('.') {
            return None;
        }
        self.load(id).or_else(|| self.list().into_iter().find(|b| b.version_id == id || b.id == id))
    }

    pub fn get_by_name(&self, name: &str) -> Option<Build> {
        self.list().into_iter().find(|b| b.name == name)
    }

    /// Adds a new build in a folder of its own, `games/<id>`, that did not exist before (`key`,
    /// `version_id`, `id` and `path` are replaced). Refuses an empty name or one another build has
    /// (ignoring case), as the original does.
    pub fn create(&self, build: &mut Build) -> AppResult<()> {
        let id = self.claim_folder(&build.name)?;
        if let Err(e) = self.create_in(build, &id) {
            let _ = fs::remove_dir(self.games_dir.join(&id));
            return Err(e);
        }
        Ok(())
    }

    /// The trimmed name of a new build, refusing an empty one or one of `builds` has (ignoring
    /// case).
    fn new_name(name: &str, builds: &[Build]) -> AppResult<String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::new(ErrorCode::VersionNameEmpty, "the build name is empty"));
        }
        if builds.iter().any(|b| b.name.trim().to_lowercase() == name.to_lowercase()) {
            return Err(AppError::new(ErrorCode::VersionExists, "a build with this name exists")
                .with_param("name", &name));
        }
        Ok(name)
    }

    /// Claims a new, empty folder `games/<id>` for a build named `name` and returns `id`. It is
    /// not a build until `create_in` writes its record, so its files can be put in place first.
    pub fn claim_folder(&self, name: &str) -> AppResult<String> {
        let _writes = self.write_lock();
        let builds = self.list();
        let name = Self::new_name(name, &builds)?;
        fs::create_dir_all(&self.games_dir).map_err(|e| folder_error(e, &self.games_dir))?;
        let taken = |id: &str| {
            fs::symlink_metadata(self.games_dir.join(id)).is_ok()
                || builds
                    .iter()
                    .any(|b| [&b.key, &b.version_id, &b.id].iter().any(|t| t.eq_ignore_ascii_case(id)))
        };
        let mut claimed = None;
        for _ in 0..CREATE_ATTEMPTS {
            let id = new_build_id(&name, taken);
            match fs::create_dir(self.games_dir.join(&id)) {
                Ok(()) => {
                    write_claim(&self.games_dir.join(&id));
                    claimed = Some(id);
                    break;
                }
                // Another launcher took this folder a moment ago.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(folder_error(e, &self.games_dir.join(&id))),
            }
        }
        claimed.ok_or_else(|| {
            AppError::new(ErrorCode::DirectoryCreateFailed, "no free build folder")
                .with_param("path", self.games_dir.to_string_lossy())
        })
    }

    /// Makes the claimed folder `id` the new build `build` by writing its record (`key`,
    /// `version_id`, `id` and `path` are replaced). The name is checked again: another build may
    /// have taken it meanwhile.
    pub fn create_in(&self, build: &mut Build, id: &str) -> AppResult<()> {
        let _writes = self.write_lock();
        let name = Self::new_name(&build.name, &self.list())?;
        build.name = name;
        build.key = id.to_string();
        build.version_id = id.to_string();
        build.id = id.to_string();
        build.path = Some(self.games_dir.join(id).to_string_lossy().into_owned());
        build.snapshot = Snapshot(None);
        self.save_locked(build)?;
        let _ = fs::remove_file(self.games_dir.join(id).join(CLAIM_FILE));
        Ok(())
    }

    /// Removes the folders installs claimed and never finished (the launcher ended first): no
    /// record, and a claim no running launcher holds; and what an earlier removal left. How many
    /// claimed folders went.
    pub fn clear_abandoned_claims(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.games_dir) else { return 0 };
        let mut cleared = 0;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let folder = entry.path();
            if name.starts_with(REMOVING_PREFIX) {
                let _ = fs::remove_dir_all(&folder);
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            // Renamed aside while no build can be created: the slow removal runs without the lock.
            let aside = {
                let _writes = self.write_lock();
                let abandoned = matches!(read_record(&folder.join(RECORD_FILE)), RecordRead::Missing)
                    && claim_abandoned(&folder);
                let aside = self.games_dir.join(format!("{REMOVING_PREFIX}{name}"));
                (abandoned && fs::rename(&folder, &aside).is_ok()).then_some(aside)
            };
            if let Some(aside) = aside {
                tracing::info!("Removing the folder {name} an unfinished install left");
                if let Err(e) = fs::remove_dir_all(&aside) {
                    tracing::warn!("The folder {name} an unfinished install left stays for now: {e}");
                }
                cleared += 1;
            }
        }
        cleared
    }

    /// Writes one build: the fields this copy changed since it was read are merged into whatever
    /// its record holds now. Afterwards `build` is the up-to-date copy.
    pub fn save(&self, build: &mut Build) -> AppResult<()> {
        let _writes = self.write_lock();
        self.save_locked(build)
    }

    fn save_locked(&self, build: &mut Build) -> AppResult<()> {
        if !is_folder_name(&build.key) {
            return Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("invalid build folder {:?}", build.key),
            )
            .with_param("id", &build.key));
        }
        let folder = self.games_dir.join(&build.key);
        let file = folder.join(RECORD_FILE);
        let current = build.to_record();
        let mut record = match (&build.snapshot.0, read_record(&file)) {
            (None, RecordRead::Missing) => current,
            (None, _) => {
                return Err(AppError::new(ErrorCode::VersionExists, "a build with this id exists")
                    .with_param("name", &build.name));
            }
            (Some(_), RecordRead::Missing) => {
                return Err(AppError::new(
                    ErrorCode::VersionNotFound,
                    "the build was removed by another writer",
                )
                .with_param("version", &build.name));
            }
            (Some(baseline), RecordRead::Record(persisted)) => merge_changes(&current, baseline, &persisted),
            (Some(_), RecordRead::Unreadable) => {
                // This copy is the best content left; the damaged file is kept next to it.
                if let Err(e) = backup_corrupt_file(&file) {
                    tracing::warn!("Unable to keep a copy of {}: {e}", file.display());
                }
                current
            }
        };
        record.insert("path".to_string(), json!(format!("{GAMES_DIR}/{}", build.key)));
        fs::create_dir_all(&folder).map_err(|e| folder_error(e, &folder))?;
        write_json_file(&file, &Value::Object(record.clone()), 4).map_err(io_error)?;
        *build = Build::loaded(&build.key, &record, &folder);
        Ok(())
    }

    /// Forgets a build: its record goes. With `delete_files` its folder goes too, but only when it
    /// lies strictly inside the Minecraft folder.
    pub fn remove(&self, key: &str, delete_files: bool) -> AppResult<()> {
        let _writes = self.write_lock();
        if !is_folder_name(key) || key.starts_with('.') {
            return Ok(());
        }
        let folder = self.games_dir.join(key);
        let file = folder.join(RECORD_FILE);
        if fs::symlink_metadata(&file).is_err() {
            return Ok(());
        }
        fs::remove_file(&file).map_err(io_error)?;
        if !delete_files {
            return Ok(());
        }
        let Some(dir) = self.folder_for_deletion(&folder) else { return Ok(()) };
        if dir.is_dir()
            && let Err(e) = fs::remove_dir_all(&dir)
        {
            return Err(AppError::new(ErrorCode::VersionFilesRemain, e.to_string())
                .with_param("path", dir.to_string_lossy()));
        }
        Ok(())
    }

    fn folder_for_deletion(&self, folder: &Path) -> Option<PathBuf> {
        let root = fs::canonicalize(&self.minecraft_dir).ok()?;
        let candidate = fs::canonicalize(folder).ok()?;
        (candidate != root && candidate.starts_with(&root)).then_some(candidate)
    }

    /// Moves the records of an old `versions.json` into their build folders, once; the file is then
    /// renamed to `versions.json.imported`. Records pointing outside `games/` are skipped, and a
    /// folder that already has a record keeps it.
    fn import_registry(&self, registry: &Path) {
        let data = match read_json_object(registry) {
            Ok(JsonRead::Object(data)) => data,
            Ok(JsonRead::Missing) => return,
            Ok(JsonRead::Invalid(reason)) => {
                tracing::warn!("Not importing {} ({reason})", registry.display());
                return;
            }
            Err(e) => {
                tracing::warn!("Unable to read {}: {e}", registry.display());
                return;
            }
        };
        let _writes = self.write_lock();
        for (key, value) in &data {
            let Value::Object(record) = value else {
                tracing::warn!("Skipping invalid build record {key:?} in {}", registry.display());
                continue;
            };
            let Some(name) = self.import_folder(key, record) else {
                tracing::warn!(
                    "Not importing build {key:?}: its folder is outside {}",
                    self.games_dir.display()
                );
                continue;
            };
            let folder = self.games_dir.join(&name);
            let file = folder.join(RECORD_FILE);
            if fs::symlink_metadata(&file).is_ok() {
                continue;
            }
            let mut record = record.clone();
            record.insert("path".to_string(), json!(format!("{GAMES_DIR}/{name}")));
            let written =
                fs::create_dir_all(&folder).and_then(|()| write_json_file(&file, &Value::Object(record), 4));
            if let Err(e) = written {
                tracing::warn!("Unable to import build {key:?} into {}: {e}", folder.display());
            }
        }
        let imported = registry.with_file_name(format!("{VERSIONS_FILE}{IMPORTED_SUFFIX}"));
        if let Err(e) = fs::rename(registry, &imported) {
            tracing::warn!("Unable to rename {}: {e}", registry.display());
        }
    }

    /// The folder in `games/` an old record belongs to: where its `path` points (relative paths
    /// start at the Minecraft folder), else its version id.
    fn import_folder(&self, key: &str, record: &Map<String, Value>) -> Option<String> {
        let raw = record.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty());
        let Some(raw) = raw else { return Some(normalize_string(key)) };
        let path = Path::new(raw);
        let path = if path.is_absolute() { path.to_path_buf() } else { self.minecraft_dir.join(path) };
        let name = path.file_name()?.to_str()?.to_string();
        let inside = path.parent().is_some_and(|parent| path_key(parent) == path_key(&self.games_dir));
        (inside && is_folder_name(&name) && !name.starts_with('.')).then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct Dirs {
        _tmp: tempfile::TempDir,
        state: PathBuf,
        mc: PathBuf,
        games: PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let mc = tmp.path().join("minecraft");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&mc).unwrap();
        Dirs { games: mc.join(GAMES_DIR), state, mc, _tmp: tmp }
    }

    fn put(d: &Dirs, folder: &str, record: Value) {
        fs::create_dir_all(d.games.join(folder)).unwrap();
        fs::write(d.games.join(folder).join(RECORD_FILE), record.to_string()).unwrap();
    }

    fn record(d: &Dirs, folder: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(d.games.join(folder).join(RECORD_FILE)).unwrap()).unwrap()
    }

    #[test]
    fn the_launcher_s_own_paths_in_a_build_folder_are_reserved() {
        for path in [
            "version.json",
            "VERSION.JSON",
            ".launcher",
            ".launcher/modrinth-pack.json",
            ".LAUNCHER/x",
            ".launcher-pack.mrpack",
        ] {
            assert!(reserved_in_build(path), "{path}");
        }
        for path in ["config/version.json", "mods/a.jar", "launcher.txt", "options.txt"] {
            assert!(!reserved_in_build(path), "{path}");
        }
    }

    #[test]
    fn records_load_from_build_folders_with_the_original_defaults() {
        let d = dirs();
        put(
            &d,
            "aeronautics",
            json!({"name": "Aeronautics (Roxy)", "version": "1.21.1", "loader": "neoforge-21.1.77",
                   "path": "somewhere/else", "options": "broken", "custom": 7}),
        );
        put(&d, "my pack", json!({"id": "legacy_id"}));
        fs::create_dir_all(d.games.join("no-record")).unwrap();
        put(&d, ".hidden", json!({}));
        fs::create_dir_all(d.games.join("broken")).unwrap();
        fs::write(d.games.join("broken").join(RECORD_FILE), b"{ broken").unwrap();
        let store = VersionStore::open(&d.state, &d.mc);
        let b = store.get("aeronautics").unwrap();
        assert_eq!(
            (b.key.as_str(), b.version_id.as_str(), b.id.as_str()),
            ("aeronautics", "aeronautics", "aeronautics")
        );
        assert_eq!(
            b.path.as_deref().map(PathBuf::from),
            Some(d.games.join("aeronautics")),
            "the folder is where the record is"
        );
        assert_eq!(b.client.as_deref(), Some("neoforge-21.1.77"), "client falls back to the loader");
        assert_eq!(
            b.options.get("gpuMode"),
            Some(&json!(crate::java::gpu::platform_default(crate::paths::Os::current()).as_str())),
            "invalid options become defaults"
        );
        assert_eq!(store.get_by_name("Aeronautics (Roxy)").unwrap().key, "aeronautics");
        let pack = store.get("my_pack").unwrap();
        assert_eq!((pack.key.as_str(), pack.name.as_str()), ("my pack", "my_pack"), "found by version id");
        assert_eq!(store.get("legacy_id").unwrap().key, "my pack", "found by record id");
        let keys: Vec<String> = store.list().into_iter().map(|b| b.key).collect();
        assert_eq!(keys, ["aeronautics", "my pack"], "folders without a readable record are not builds");
        assert!(store.get("missing").is_none());
        assert!(store.get("../state").is_none() && store.get("").is_none());
    }

    #[test]
    fn a_new_build_is_written_in_full_into_its_folder() {
        let d = dirs();
        let store = VersionStore::open(&d.state, &d.mc);
        let mut build = Build::new("Моя збірка");
        build.version = Some("1.21.1".into());
        store.save(&mut build).unwrap();
        let saved = record(&d, "moja_zbirka");
        assert_eq!(saved.as_object().unwrap().len(), 13);
        assert_eq!(saved["name"], json!("Моя збірка"));
        assert_eq!(saved["path"], json!("games/moja_zbirka"));
        assert_eq!(
            saved["options"],
            json!({"gpuMode": crate::java::gpu::platform_default(crate::paths::Os::current()).as_str()})
        );
        let text = fs::read_to_string(d.games.join("moja_zbirka").join(RECORD_FILE)).unwrap();
        assert!(text.contains("Моя збірка"), "not escaped");
        assert!(!d.state.join(VERSIONS_FILE).exists(), "no registry file any more");
        assert_eq!(VersionStore::open(&d.state, &d.mc).get("moja_zbirka").unwrap(), build);
    }

    #[test]
    fn save_keeps_changes_made_by_another_writer() {
        let d = dirs();
        put(&d, "a", json!({"name": "A", "options": {"jvmArguments": ["-Xmx4G"]}, "custom": 1}));
        let store = VersionStore::open(&d.state, &d.mc);
        let mut a = store.get("a").unwrap();
        // Another launcher instance edits the record after we loaded it.
        let mut other = record(&d, "a");
        other["description"] = json!("from elsewhere");
        other["options"]["homePinned"] = json!(true);
        put(&d, "a", other);
        a.options.insert("jvmArguments".into(), json!(["-Xmx8G"]));
        store.save(&mut a).unwrap();
        let saved = record(&d, "a");
        assert_eq!(saved["options"]["jvmArguments"], json!(["-Xmx8G"]));
        assert_eq!(saved["options"]["homePinned"], json!(true));
        assert_eq!(saved["description"], json!("from elsewhere"));
        assert_eq!(saved["custom"], json!(1), "unknown keys survive");
        assert!(saved["options"].get("gpuMode").is_none(), "the injected default is not a change");
        assert_eq!(saved["path"], json!("games/a"));
    }

    #[test]
    fn saving_a_build_removed_elsewhere_is_an_error() {
        let d = dirs();
        put(&d, "a", json!({"name": "A"}));
        let store = VersionStore::open(&d.state, &d.mc);
        let mut a = store.get("a").unwrap();
        fs::remove_file(d.games.join("a").join(RECORD_FILE)).unwrap();
        a.description = "edited".into();
        assert_eq!(store.save(&mut a).unwrap_err().code, ErrorCode::VersionNotFound);
        assert!(!d.games.join("a").join(RECORD_FILE).exists());
    }

    #[test]
    fn a_damaged_record_is_replaced_by_the_loaded_copy_and_kept_aside() {
        let d = dirs();
        put(&d, "a", json!({"name": "A"}));
        let store = VersionStore::open(&d.state, &d.mc);
        let mut a = store.get("a").unwrap();
        fs::write(d.games.join("a").join(RECORD_FILE), b"{ broken").unwrap();
        a.description = "kept".into();
        store.save(&mut a).unwrap();
        assert_eq!(record(&d, "a")["description"], json!("kept"));
        assert_eq!(record(&d, "a")["name"], json!("A"));
        let backups = fs::read_dir(d.games.join("a"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn remove_forgets_the_record_and_deletes_only_its_own_folder() {
        let d = dirs();
        put(&d, "keep", json!({"name": "Keep"}));
        put(&d, "gone", json!({"name": "Gone"}));
        fs::write(d.games.join("keep").join("options.txt"), b"fov:90").unwrap();
        let store = VersionStore::open(&d.state, &d.mc);
        store.remove("keep", false).unwrap();
        assert!(d.games.join("keep").join("options.txt").is_file(), "files stay");
        assert!(!d.games.join("keep").join(RECORD_FILE).exists(), "the build is forgotten");
        store.remove("gone", true).unwrap();
        assert!(!d.games.join("gone").exists());
        for odd in ["never-existed", ".", "..", "../minecraft", ""] {
            store.remove(odd, true).unwrap();
        }
        assert!(d.mc.is_dir() && d.games.is_dir(), "the Minecraft and games folders stay");
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_stale_copy_does_not_revert_newer_changes() {
        let d = dirs();
        put(&d, "a", json!({"name": "Aero"}));
        let store = VersionStore::open(&d.state, &d.mc);
        let mut install = store.get("a").unwrap();
        // Meanwhile the UI saves its own changes from a fresher copy.
        let mut ui = store.get("a").unwrap();
        ui.name = "Renamed".into();
        ui.options.insert("jvmArguments".into(), json!(["-Xmx8G"]));
        store.save(&mut ui).unwrap();
        // The install finishes with the copy it took before.
        install.loader = Some("1.21.1".into());
        store.save(&mut install).unwrap();
        let saved = record(&d, "a");
        assert_eq!(saved["name"], json!("Renamed"));
        assert_eq!(saved["options"]["jvmArguments"], json!(["-Xmx8G"]));
        assert_eq!(saved["loader"], json!("1.21.1"));
        let current = store.get("a").unwrap();
        assert_eq!((current.name.as_str(), current.loader.as_deref()), ("Renamed", Some("1.21.1")));
        assert_eq!(install, current, "the saved copy is brought up to date");
    }

    #[test]
    fn create_refuses_a_taken_or_empty_name() {
        let d = dirs();
        let store = VersionStore::open(&d.state, &d.mc);
        store.create(&mut Build::new("Моя збірка")).unwrap();
        let err = store.create(&mut Build::new(" моя ЗБІРКА ")).unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionExists);
        assert_eq!(store.create(&mut Build::new("   ")).unwrap_err().code, ErrorCode::VersionNameEmpty);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn create_gives_each_build_its_own_new_folder() {
        let d = dirs();
        let leftover = d.games.join("moja_zbirka");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join("keep.txt"), b"old world").unwrap();
        let store = VersionStore::open(&d.state, &d.mc);
        let mut first = Build::new("Моя збірка");
        let mut second = Build::new("Моя-збірка");
        let mut con = Build::new("CON");
        let mut long = Build::new(&"дуже довга назва ".repeat(20));
        for build in [&mut first, &mut second, &mut con, &mut long] {
            store.create(build).unwrap();
            crate::builds::ids::validate_component_id(&build.key).unwrap();
            assert_eq!(
                (build.version_id.as_str(), build.id.as_str()),
                (build.key.as_str(), build.key.as_str())
            );
            assert_eq!(build.path.as_deref().map(PathBuf::from), Some(d.games.join(&build.key)));
            assert!(d.games.join(&build.key).join(RECORD_FILE).is_file());
        }
        assert_eq!((first.key.as_str(), second.key.as_str()), ("moja_zbirka_2", "moja_zbirka_3"));
        assert_eq!(
            fs::read(leftover.join("keep.txt")).unwrap(),
            b"old world",
            "a leftover folder is never taken"
        );
        assert!(long.key.len() <= crate::builds::ids::MAX_NEW_ID_CHARS, "{}", long.key);
        assert_eq!(store.list().len(), 4);
    }

    #[test]
    fn saving_a_new_build_never_merges_into_an_existing_one() {
        let d = dirs();
        put(&d, "moja_zbirka", json!({"name": "Моя збірка", "version": "1.20.1"}));
        let store = VersionStore::open(&d.state, &d.mc);
        let err = store.save(&mut Build::new("Моя збірка")).unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionExists);
        assert_eq!(record(&d, "moja_zbirka")["version"], json!("1.20.1"));
    }

    #[test]
    fn an_old_registry_is_imported_once() {
        let d = dirs();
        let outside = d.state.join("outside");
        put(&d, "taken", json!({"name": "Already here"}));
        fs::write(
            d.state.join(VERSIONS_FILE),
            json!({
                "Aeronautics": {"name": "Aeronautics (Roxy)", "path": "games/aeronautics", "version": "1.21.1"},
                "plain": {},
                "absolute": {"name": "Abs", "path": d.games.join("abs").to_string_lossy()},
                "outside": {"name": "Out", "path": outside.to_string_lossy()},
                "taken": {"name": "Imported over", "path": "games/taken"},
                "junk": 7
            })
            .to_string(),
        )
        .unwrap();
        let store = VersionStore::open(&d.state, &d.mc);
        assert_eq!(record(&d, "aeronautics")["name"], json!("Aeronautics (Roxy)"));
        assert_eq!(record(&d, "aeronautics")["path"], json!("games/aeronautics"));
        assert_eq!(record(&d, "plain"), json!({"path": "games/plain"}));
        assert_eq!(record(&d, "abs")["path"], json!("games/abs"));
        assert_eq!(record(&d, "taken")["name"], json!("Already here"), "an existing record wins");
        assert!(!outside.exists() && !d.games.join("outside").exists());
        let keys: Vec<String> = store.list().into_iter().map(|b| b.key).collect();
        assert_eq!(keys, ["abs", "aeronautics", "plain", "taken"]);
        assert!(!d.state.join(VERSIONS_FILE).exists());
        assert!(d.state.join(format!("{VERSIONS_FILE}{IMPORTED_SUFFIX}")).is_file(), "kept for the user");

        fs::write(d.state.join(VERSIONS_FILE), b"{ broken").unwrap();
        VersionStore::open(&d.state, &d.mc);
        assert!(d.state.join(VERSIONS_FILE).is_file(), "an unreadable registry is left alone");
    }

    #[test]
    fn a_claimed_folder_becomes_a_build_only_with_its_record() {
        let d = dirs();
        let store = VersionStore::open(&d.state, &d.mc);
        let id = store.claim_folder(" Копія ").unwrap();
        assert_eq!(id, "kopija");
        assert!(d.games.join(&id).is_dir());
        assert!(
            store.list().is_empty() && store.get(&id).is_none(),
            "not a build until its record is written"
        );
        assert_eq!(store.claim_folder("Копія").unwrap(), "kopija_2", "a claimed folder is taken");
        let mut build = Build::new("Копія");
        store.create_in(&mut build, &id).unwrap();
        assert_eq!(build.key, id);
        assert_eq!(build.path.as_deref().map(PathBuf::from), Some(d.games.join(&id)));
        assert_eq!(store.list().len(), 1);
        let mut same = Build::new(" копія ");
        assert_eq!(store.create_in(&mut same, "kopija_2").unwrap_err().code, ErrorCode::VersionExists);
        assert_eq!(store.claim_folder("  ").unwrap_err().code, ErrorCode::VersionNameEmpty);
        assert_eq!(store.claim_folder("КОПІЯ").unwrap_err().code, ErrorCode::VersionExists);
    }

    #[test]
    fn a_folder_an_install_that_ended_left_behind_is_cleared() {
        let d = dirs();
        let store = VersionStore::open(&d.state, &d.mc);
        let left = store.claim_folder("Left").unwrap();
        fs::write(d.games.join(&left).join("big.jar"), b"jar").unwrap();
        // The launcher that claimed it is gone.
        fs::write(d.games.join(&left).join(CLAIM_FILE), r#"{"pid": 0, "started": "1"}"#).unwrap();
        let filling = store.claim_folder("Filling").unwrap();
        let mut done = Build::new("Done");
        store.create(&mut done).unwrap();
        assert!(!d.games.join(&done.key).join(CLAIM_FILE).exists(), "a build's folder is no claim");
        fs::create_dir_all(d.games.join("mine")).unwrap();
        fs::create_dir_all(d.games.join("broken_record")).unwrap();
        fs::write(d.games.join("broken_record/version.json"), b"{ broken").unwrap();
        fs::write(d.games.join("broken_record").join(CLAIM_FILE), r#"{"pid": 0}"#).unwrap();

        assert_eq!(store.clear_abandoned_claims(), 1);
        assert!(!d.games.join(&left).exists(), "its install ended with the launcher");
        assert!(d.games.join(&filling).is_dir(), "this launcher is filling it");
        assert!(d.games.join(&done.key).is_dir());
        assert!(d.games.join("mine").is_dir(), "a folder the launcher did not claim is left alone");
        assert!(d.games.join("broken_record").is_dir(), "an unreadable record is still a build");
    }
}
