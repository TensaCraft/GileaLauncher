//! The groups and what the launcher saw of their files, in the launcher's state:
//! `<state>/sync/groups.json` and a snapshot per group, `<state>/sync/<id>.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use launcher_core::storage::json::write_json_file;
use launcher_core::storage::path_lock;
use launcher_shared::{AppError, AppResult, ErrorCode, Text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::paths::{key, normalize, overlaps, taken};
use crate::dto::{GroupState, MESSAGE, SyncGroup};

/// The version of a file the group agreed on at its last pass.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Version {
    pub hash: String,
    pub size: u64,
    pub modified_ms: u64,
    /// The build it came from.
    pub from: String,
}

/// What a build held of a file after the last pass it took part in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Seen {
    pub hash: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// One synced file: its spelling, the agreed version (none: removed everywhere), and each build's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub agreed: Option<Version>,
    #[serde(default)]
    pub held: BTreeMap<String, Seen>,
}

/// What the launcher saw of a group's files after its last pass, and where the group stands.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// The builds that have taken part in a pass (a build not among them joins).
    #[serde(default)]
    pub members: BTreeSet<String>,
    /// The paths synced so far (a path not among them joins).
    #[serde(default)]
    pub paths: BTreeSet<String>,
    /// Each member's folder mark (`.launcher/sync-id`) when it last took part: a build made again
    /// under the same key has another, and joins anew.
    #[serde(default)]
    pub marks: BTreeMap<String, String>,
    /// By `paths::key`.
    #[serde(default)]
    pub files: BTreeMap<String, FileRecord>,
    #[serde(default)]
    pub last_ms: Option<u64>,
    #[serde(default)]
    pub state: GroupState,
    #[serde(default)]
    pub note: Option<Text>,
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn invalid(why: &str, field: &str, message: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, why.to_string())
        .with_param("field", field)
        .with_param(MESSAGE, message)
}

/// A group id names its files in `<state>/sync/`: letters, digits, `-` and `_` only.
fn checked(id: &str) -> AppResult<&str> {
    let fits = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if fits {
        Ok(id)
    } else {
        Err(AppError::new(ErrorCode::InvalidInput, format!("no sync group id {id:?}")))
    }
}

fn read<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn write(path: &Path, value: &Value) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    write_json_file(path, value, 2).map_err(|e| io_error(path, e))
}

#[derive(Default, Deserialize)]
struct GroupsFile {
    #[serde(default)]
    groups: Vec<SyncGroup>,
}

/// The groups and their snapshots, in `<state>/sync/`.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(state_dir: &Path) -> Store {
        Store { dir: state_dir.join("sync") }
    }

    /// Where the group's lock and snapshot live.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn groups_path(&self) -> PathBuf {
        self.dir.join("groups.json")
    }

    fn snapshot_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn groups(&self) -> Vec<SyncGroup> {
        read::<GroupsFile>(&self.groups_path()).groups
    }

    pub fn group(&self, id: &str) -> AppResult<SyncGroup> {
        let id = checked(id)?;
        self.groups().into_iter().find(|g| g.id == id).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("no sync group {id}"))
                .with_param("group", id)
                .with_param(MESSAGE, "sync_error_group_gone")
        })
    }

    fn write_groups(&self, groups: &[SyncGroup]) -> AppResult<()> {
        write(&self.groups_path(), &json!({"groups": groups}))
    }

    /// Saves `group`, a new one with a new id: refused unless it has a name, two or more builds
    /// that exist, its starting build among them, and paths that may be synced and that no other
    /// group holds for one of its builds. Paths are written one way, and one inside another is
    /// dropped.
    pub fn save(&self, mut group: SyncGroup, builds_exist: &dyn Fn(&str) -> bool) -> AppResult<SyncGroup> {
        if !group.id.is_empty() {
            checked(&group.id)?;
        }
        group.name = group.name.trim().to_string();
        if group.name.is_empty() {
            return Err(invalid("a sync group needs a name", "name", "sync_error_name"));
        }
        let mut builds: Vec<String> = Vec::new();
        for build in &group.builds {
            if !builds.contains(build) {
                builds.push(build.clone());
            }
        }
        if builds.len() < 2 {
            return Err(invalid("a sync group needs two builds or more", "builds", "sync_error_builds"));
        }
        if let Some(missing) = builds.iter().find(|b| !builds_exist(b)) {
            return Err(
                invalid("no such build", "builds", "sync_error_build_missing").with_param("version", missing)
            );
        }
        if !builds.contains(&group.start) {
            return Err(invalid("the starting build is not in the group", "start", "sync_error_start"));
        }
        let mut paths: Vec<String> = Vec::new();
        for raw in &group.paths {
            let path = normalize(raw)?;
            if !paths.iter().any(|p| key(p) == key(&path)) {
                paths.push(path);
            }
        }
        // One inside another: the outer one takes it.
        let all = paths.clone();
        paths.retain(|p| !all.iter().any(|outer| outer != p && overlaps(outer, p) && outer.len() < p.len()));
        if paths.is_empty() {
            return Err(invalid("a sync group needs a path", "paths", "sync_error_paths"));
        }
        let lock = path_lock(&self.groups_path());
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut groups = self.groups();
        let editing = Some(group.id.as_str()).filter(|id| !id.is_empty());
        for build in &builds {
            for path in &paths {
                if let Some(other) = taken(&groups, editing, build, path) {
                    return Err(invalid(
                        "another group syncs this path for a build",
                        "paths",
                        "sync_error_taken",
                    )
                    .with_param("group", other)
                    .with_param("path", path)
                    .with_param("version", build));
                }
            }
        }
        if group.id.is_empty() {
            group.id = uuid::Uuid::new_v4().simple().to_string();
        }
        group.builds = builds;
        group.paths = paths;
        match groups.iter_mut().find(|g| g.id == group.id) {
            Some(old) => *old = group.clone(),
            None => groups.push(group.clone()),
        }
        self.write_groups(&groups)?;
        Ok(group)
    }

    /// Removes the group and its snapshot; nothing in the builds is touched.
    pub fn delete(&self, id: &str) -> AppResult<()> {
        let id = checked(id)?;
        let lock = path_lock(&self.groups_path());
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut groups = self.groups();
        groups.retain(|g| g.id != id);
        self.write_groups(&groups)?;
        let snapshot = self.snapshot_path(id);
        match fs::remove_file(&snapshot) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_error(&snapshot, e)),
            _ => Ok(()),
        }
    }

    /// The group's snapshot; an empty one when it never synced.
    pub fn snapshot(&self, id: &str) -> Snapshot {
        checked(id).map(|id| read(&self.snapshot_path(id))).unwrap_or_default()
    }

    pub fn write_snapshot(&self, id: &str, snapshot: &Snapshot) -> AppResult<()> {
        let id = checked(id)?;
        let value = serde_json::to_value(snapshot).map_err(|e| AppError::internal(e.to_string()))?;
        write(&self.snapshot_path(id), &value)
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::ErrorCode;

    use super::*;
    use crate::dto::SyncMode;

    fn group(builds: &[&str], paths: &[&str]) -> SyncGroup {
        SyncGroup {
            id: String::new(),
            name: "Мінікарта".into(),
            builds: builds.iter().map(|b| b.to_string()).collect(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            mode: SyncMode::Latest,
            start: builds.first().map(|b| b.to_string()).unwrap_or_default(),
        }
    }

    fn exist(key: &str) -> bool {
        ["aero", "lite", "zeta"].contains(&key)
    }

    #[test]
    fn a_group_is_saved_validated_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(store.groups().is_empty());
        let saved = store
            .save(group(&["aero", "lite"], &[r"xaero\", "options.txt", "xaero/minimap.txt"]), &exist)
            .unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.paths, vec!["xaero", "options.txt"], "written one way, an inner path dropped");
        assert_eq!(store.groups(), vec![saved.clone()]);

        let refused = |g: SyncGroup| store.save(g, &exist).unwrap_err().code;
        assert_eq!(refused(group(&["aero"], &["options.txt"])), ErrorCode::InvalidInput, "one build");
        assert_eq!(
            refused(group(&["aero", "ghost"], &["options.txt"])),
            ErrorCode::InvalidInput,
            "a missing build"
        );
        assert_eq!(
            refused(SyncGroup { start: "zeta".into(), ..group(&["aero", "lite"], &["servers.dat"]) }),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            refused(group(&["aero", "lite"], &["saves/World"])),
            ErrorCode::InvalidInput,
            "never synced"
        );
        assert_eq!(refused(group(&["aero", "lite"], &[])), ErrorCode::InvalidInput, "no path");
        assert_eq!(
            refused(SyncGroup { name: " ".into(), ..group(&["aero", "lite"], &["servers.dat"]) }),
            ErrorCode::InvalidInput
        );
        let taken = store.save(group(&["zeta", "lite"], &["xaero/waypoints"]), &exist).unwrap_err();
        assert_eq!(
            taken.params.get("group").map(String::as_str),
            Some("Мінікарта"),
            "held by the other group for lite"
        );

        let renamed = store.save(SyncGroup { name: "Карта".into(), ..saved.clone() }, &exist).unwrap();
        assert_eq!((renamed.id.as_str(), store.groups().len()), (saved.id.as_str(), 1), "saved over itself");
        store.delete(&saved.id).unwrap();
        assert!(store.groups().is_empty());
    }

    #[test]
    fn a_refused_group_names_what_the_player_reads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(group(&["aero", "lite"], &["xaero"]), &exist).unwrap();
        let message = |g: SyncGroup| store.save(g, &exist).unwrap_err().params.get("message").cloned();
        let named = |g: SyncGroup| message(g).unwrap_or_default();
        assert_eq!(
            named(SyncGroup { name: " ".into(), ..group(&["aero", "lite"], &["a.txt"]) }),
            "sync_error_name"
        );
        assert_eq!(named(group(&["aero"], &["a.txt"])), "sync_error_builds");
        assert_eq!(named(group(&["aero", "ghost"], &["a.txt"])), "sync_error_build_missing");
        assert_eq!(
            named(SyncGroup { start: "zeta".into(), ..group(&["aero", "lite"], &["a.txt"]) }),
            "sync_error_start"
        );
        assert_eq!(named(group(&["aero", "lite"], &[])), "sync_error_paths");
        assert_eq!(named(group(&["aero", "lite"], &["saves/World"])), "sync_error_path_blocked");
        assert_eq!(named(group(&["aero", "lite"], &["../x"])), "sync_error_path_invalid");
        assert_eq!(named(group(&["zeta", "lite"], &["xaero/waypoints"])), "sync_error_taken");
        assert_eq!(store.group("gone").unwrap_err().params.get("message").unwrap(), "sync_error_group_gone");
    }

    #[test]
    fn a_group_id_never_leaves_the_sync_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let outside = dir.path().join("x.json");
        fs::write(&outside, "{}").unwrap();
        assert_eq!(store.delete("../x").unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(store.group("../x").unwrap_err().code, ErrorCode::InvalidInput);
        assert!(store.write_snapshot("../x", &Snapshot::default()).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "{}", "untouched");
        let forged = SyncGroup { id: "../evil".into(), ..group(&["aero", "lite"], &["options.txt"]) };
        assert_eq!(store.save(forged, &exist).unwrap_err().code, ErrorCode::InvalidInput);
        assert!(store.groups().is_empty() && !dir.path().join("evil.json").exists());
        let kept = SyncGroup { id: "a1b2".into(), ..group(&["aero", "lite"], &["options.txt"]) };
        assert_eq!(store.save(kept, &exist).unwrap().id, "a1b2", "an id of its own letters is kept");
    }

    #[test]
    fn a_snapshot_round_trips_and_survives_a_rename() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let saved = store.save(group(&["aero", "lite"], &["options.txt"]), &exist).unwrap();
        assert_eq!(store.snapshot(&saved.id), Snapshot::default(), "none yet");
        let mut snapshot = Snapshot { last_ms: Some(5), state: GroupState::Synced, ..Snapshot::default() };
        snapshot.members.insert("aero".into());
        snapshot.paths.insert("options.txt".into());
        let version = Version { hash: "ab".into(), size: 3, modified_ms: 7, from: "aero".into() };
        let mut record =
            FileRecord { path: "options.txt".into(), agreed: Some(version), held: Default::default() };
        record.held.insert("aero".into(), Seen { hash: "ab".into(), size: 3, modified_ms: 7 });
        snapshot.files.insert("options.txt".into(), record);
        store.write_snapshot(&saved.id, &snapshot).unwrap();
        store.save(SyncGroup { name: "Інша назва".into(), ..saved.clone() }, &exist).unwrap();
        assert_eq!(store.snapshot(&saved.id), snapshot);
        store.delete(&saved.id).unwrap();
        assert_eq!(store.snapshot(&saved.id), Snapshot::default(), "it goes with its group");
    }
}
