//! The sync service behind the commands, the launch hook and the game watcher: groups, a pass
//! over a group and the editor's tree.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use launcher_core::feedback::{FeedbackService, OperationSpec};
use launcher_core::launch::alive::game_open;
use launcher_core::launch::options::game_dir;
use launcher_core::lock::{Coordinator, Lease, LockError, LockPlace, OsFileLock};
use launcher_core::net::preflight::{SpaceRequest, preflight};
use launcher_core::storage::usage::dir_size;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{AppError, AppResult, ErrorCode, Level, Text};
use serde_json::Value;

use super::apply::{Applied, Member, apply};
use super::blacklist::blocked;
use super::decide::{Decision, Outcome, decide};
use super::paths::{key, normalize, taken};
use super::scan::{Found, is_managed, scan};
use super::store::{FileRecord, Seen, Snapshot, Store};
use crate::dto::{
    GROUPS_EVENT, GroupState, GroupView, Lock, MESSAGE, PassReport, SyncGroup, TreeEntry, error_text,
};

/// A pass that writes more shows as an operation; a smaller one passes quietly.
const QUIET_BYTES: u64 = 8 * 1024 * 1024;
/// How long a pass waits for another pass of its group to end.
const LOCK_WAIT: Duration = Duration::from_secs(30);
/// How many of a pass's conflicts its toast names.
const CONFLICTS_SHOWN: usize = 3;

/// The paths of a build its modules keep in line themselves (`Module::managed_files`).
pub type ManagedFiles = Arc<dyn Fn(&Build, &Path) -> Vec<String> + Send + Sync>;

/// What the service needs of the launcher.
pub struct Deps {
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub state_dir: PathBuf,
    /// Whether build `key`'s game runs.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub managed: ManagedFiles,
}

pub struct SyncService {
    deps: Deps,
    store: Store,
    /// Groups a pass runs for in this process.
    active: Mutex<HashSet<String>>,
}

/// A group held for one pass, here and across launchers on the same folder.
struct GroupLock<'a> {
    service: &'a SyncService,
    id: String,
    _file: OsFileLock,
}

impl Drop for GroupLock<'_> {
    fn drop(&mut self) {
        self.service.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn busy(group: &SyncGroup) -> AppError {
    AppError::new(ErrorCode::Busy, format!("sync group {} is busy", group.id))
        .with_param("group", &group.name)
        .with_param(MESSAGE, "sync_error_busy")
}

impl SyncService {
    pub fn new(deps: Deps) -> SyncService {
        let store = Store::new(&deps.state_dir);
        SyncService { deps, store, active: Mutex::new(HashSet::new()) }
    }

    fn mc_dir(&self) -> &Path {
        self.deps.versions.minecraft_dir()
    }

    fn game(&self, build: &Build) -> PathBuf {
        game_dir(build, self.mc_dir())
    }

    fn running(&self, build: &Build) -> bool {
        (self.deps.running)(&build.key) || game_open(&self.game(build))
    }

    fn view(&self, group: SyncGroup) -> GroupView {
        let snapshot = self.store.snapshot(&group.id);
        let existing = group.builds.iter().filter(|b| self.deps.versions.get(b).is_some()).count();
        if existing < 2 {
            let note = Some(Text::key("sync_idle_note"));
            return GroupView { group, last_ms: snapshot.last_ms, state: GroupState::Idle, note };
        }
        GroupView { group, last_ms: snapshot.last_ms, state: snapshot.state, note: snapshot.note }
    }

    /// Whether build `key` is in a group.
    pub fn in_a_group(&self, key: &str) -> bool {
        self.store.groups().iter().any(|g| g.builds.iter().any(|b| b == key))
    }

    pub fn groups(&self) -> Vec<GroupView> {
        self.store.groups().into_iter().map(|group| self.view(group)).collect()
    }

    pub fn save(&self, group: SyncGroup) -> AppResult<GroupView> {
        let saved = self.store.save(group, &|key| self.deps.versions.get(key).is_some())?;
        Ok(self.view(saved))
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        self.store.delete(id)
    }

    fn lock_group(&self, group: &SyncGroup) -> AppResult<GroupLock<'_>> {
        let started = Instant::now();
        loop {
            let free = self.active.lock().unwrap_or_else(|e| e.into_inner()).insert(group.id.clone());
            if free {
                let token = uuid::Uuid::new_v4().simple().to_string();
                let resource = self.store.dir().join(&group.id);
                match OsFileLock::try_acquire(&resource, "sync", LockPlace::Beside, "sync", &token) {
                    Ok(file) => return Ok(GroupLock { service: self, id: group.id.clone(), _file: file }),
                    Err(e) => {
                        self.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&group.id);
                        if let LockError::Io(e) = e {
                            return Err(AppError::new(ErrorCode::Io, e.to_string()));
                        }
                    }
                }
            }
            if started.elapsed() >= LOCK_WAIT {
                return Err(busy(group));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Syncs every group of build `key`; `held`: a build whose lease the caller holds (a launch).
    pub fn sync_build(&self, key: &str, held: Option<&str>) -> Vec<(String, AppResult<PassReport>)> {
        self.store
            .groups()
            .into_iter()
            .filter(|g| g.builds.iter().any(|b| b == key))
            .map(|g| (g.name.clone(), self.sync_group(&g.id, held)))
            .collect()
    }

    /// One pass over group `id`. A failure leaves the builds and the snapshot as they were, and the
    /// group's state says so.
    pub fn sync_group(&self, id: &str, held: Option<&str>) -> AppResult<PassReport> {
        let group = self.store.group(id)?;
        let _lock = self.lock_group(&group)?;
        // Saved again while it waited: the pass goes by the group as it is now.
        let group = self.store.group(id)?;
        let mut snapshot = self.store.snapshot(id);
        let result = self.pass(&group, &mut snapshot, held);
        if let Err(e) = &result {
            tracing::warn!("Sync of {} failed: {}", group.name, e.detail);
            let mut failed = self.store.snapshot(id);
            failed.state = GroupState::Failed;
            failed.note = Some(error_text(e));
            let _ = self.store.write_snapshot(id, &failed);
        }
        self.deps.feedback.module_event(GROUPS_EVENT, &Value::Null);
        result
    }

    /// The group's builds that take part, the starting build first: existing, not running, and
    /// held for the pass (`held` is the caller's).
    fn members(&self, group: &SyncGroup, held: Option<&str>) -> (Vec<(Build, Member)>, Vec<Lease>) {
        let order = group
            .builds
            .iter()
            .filter(|b| **b == group.start)
            .chain(group.builds.iter().filter(|b| **b != group.start));
        let (mut members, mut leases) = (Vec::new(), Vec::new());
        for key in order {
            let Some(build) = self.deps.versions.get(key) else { continue };
            let game = self.game(&build);
            if !game.is_dir() {
                continue;
            }
            if held != Some(key.as_str()) {
                if self.running(&build) {
                    continue;
                }
                match self.deps.instances.try_acquire(&game, "sync") {
                    Ok(lease) => leases.push(lease),
                    Err(_) => continue,
                }
            }
            let member = Member { key: key.clone(), game };
            members.push((build, member));
        }
        (members, leases)
    }

    fn wait(&self, group: &SyncGroup, snapshot: &mut Snapshot) -> AppResult<PassReport> {
        snapshot.state = GroupState::Waiting;
        snapshot.note = None;
        self.store.write_snapshot(&group.id, snapshot)?;
        Ok(PassReport { waiting: true, ..PassReport::default() })
    }

    fn pass(&self, group: &SyncGroup, snapshot: &mut Snapshot, held: Option<&str>) -> AppResult<PassReport> {
        let (members, _leases) = self.members(group, held);
        // Builds no longer in the group, or gone, are forgotten: one that comes back joins anew.
        let kept = |b: &String| group.builds.contains(b) && self.deps.versions.get(b).is_some();
        snapshot.members.retain(|b| kept(b));
        snapshot.marks.retain(|b, _| kept(b));
        for record in snapshot.files.values_mut() {
            record.held.retain(|b, _| kept(b));
        }
        if members.len() < 2 {
            return self.wait(group, snapshot);
        }
        // A member whose folder is not the one the group knew (made again) joins anew.
        let mut marks = BTreeMap::new();
        for (_, member) in &members {
            let mark = folder_mark(&member.game)?;
            if snapshot.marks.get(&member.key) != Some(&mark) {
                snapshot.members.remove(&member.key);
                for record in snapshot.files.values_mut() {
                    record.held.remove(&member.key);
                }
            }
            marks.insert(member.key.clone(), mark);
        }
        let mut found: BTreeMap<String, BTreeMap<String, Found>> = BTreeMap::new();
        let mut managed: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (build, member) in &members {
            let mut own = (self.deps.managed)(build, &member.game);
            let scanned = scan(&member.game, &group.paths, &own, &snapshot.files, &member.key);
            found.insert(member.key.clone(), scanned.found);
            // What it could not read is not touched there either.
            own.extend(scanned.unknown);
            managed.insert(member.key.clone(), own);
        }
        let keys: Vec<String> = members.iter().map(|(_, m)| m.key.clone()).collect();
        let outcome = decide(&Decision {
            mode: group.mode,
            start: &group.start,
            members: &keys,
            snapshot,
            found: &found,
            group_paths: &group.paths,
            untouchable: &managed,
        });
        if outcome.wait {
            return self.wait(group, snapshot);
        }
        let members: Vec<Member> = members.into_iter().map(|(_, m)| m).collect();
        let size_of =
            |build: &str, key: &str| found.get(build).and_then(|f| f.get(key)).map_or(0, |f| f.size);
        let requests: Vec<SpaceRequest> = members
            .iter()
            .map(|m| SpaceRequest {
                dir: m.game.clone(),
                bytes: outcome
                    .writes
                    .iter()
                    .filter(|w| w.build == m.key)
                    .map(|w| size_of(&w.from, &w.key))
                    .sum(),
                label: group.name.clone(),
            })
            .filter(|r| r.bytes > 0)
            .collect();
        preflight(&requests)?;
        let now = SystemTime::now();
        let total: u64 = requests.iter().map(|r| r.bytes).sum();
        let op = (total > QUIET_BYTES).then(|| {
            self.deps
                .feedback
                .begin(OperationSpec::new(Text::key("sync_operation").param("group", &group.name), "sync"))
        });
        let applied = match apply(&members, &outcome) {
            Ok(applied) => applied,
            Err(e) => {
                if let Some(op) = op {
                    op.fail(error_text(&e));
                }
                return Err(e);
            }
        };
        let quiet = op.is_none();
        if let Some(op) = op {
            op.finish();
        }
        record(snapshot, group, &members, &found, &managed, &outcome, &applied);
        snapshot.marks.extend(marks);
        snapshot.last_ms = Some(millis(now));
        snapshot.state = GroupState::Synced;
        snapshot.note = None;
        self.store.write_snapshot(&group.id, snapshot)?;
        let report = PassReport {
            written: outcome.writes.len() as u32,
            removed: outcome.removes.len() as u32,
            conflicts: outcome.conflicts.clone(),
            waiting: false,
        };
        let changed = report.written + report.removed;
        if quiet && changed > 0 {
            let note = Text::key("sync_done").param("group", &group.name).param("count", changed.to_string());
            self.deps.feedback.note(Level::Info, note);
        }
        if !report.conflicts.is_empty() {
            let mut paths: Vec<&str> =
                report.conflicts.iter().take(CONFLICTS_SHOWN).map(String::as_str).collect();
            if report.conflicts.len() > CONFLICTS_SHOWN {
                paths.push("…");
            }
            let title =
                Text::key("sync_conflict").param("group", &group.name).param("path", paths.join(", "));
            self.deps.feedback.toast(Level::Warning, title, None, None);
        }
        Ok(report)
    }

    /// The rows of folder `dir` (`""`: the root) in the union of `builds`' folders, folders first:
    /// the blocked left out, a row a module manages in every build that has it locked, and one
    /// another group holds for one of the builds locked too.
    pub fn tree(&self, builds: &[String], dir: &str, editing: Option<&str>) -> AppResult<Vec<TreeEntry>> {
        let dir = if dir.trim().is_empty() { String::new() } else { normalize(dir)? };
        let builds: Vec<Build> = builds.iter().filter_map(|k| self.deps.versions.get(k)).collect();
        let groups = self.store.groups();
        let mut rows: BTreeMap<String, TreeEntry> = BTreeMap::new();
        for build in &builds {
            let game = self.game(build);
            let managed = (self.deps.managed)(build, &game);
            for entry in fs::read_dir(game.join(&dir)).into_iter().flatten().flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                if !kind.is_dir() && !kind.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let relative = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
                if blocked(&relative) {
                    continue;
                }
                let size = if kind.is_dir() {
                    dir_size(&entry.path())
                } else {
                    entry.metadata().map_or(0, |m| m.len())
                };
                let row = rows.entry(key(&relative)).or_insert_with(|| TreeEntry {
                    path: relative.clone(),
                    name: name.clone(),
                    dir: kind.is_dir(),
                    size: 0,
                    present: 0,
                    total: builds.len() as u32,
                    lock: None,
                    managed_in: Vec::new(),
                });
                row.size = row.size.max(size);
                row.present += 1;
                row.dir |= kind.is_dir();
                if is_managed(&relative, &managed) {
                    row.managed_in.push(build.name.clone());
                }
            }
        }
        let mut rows: Vec<TreeEntry> = rows.into_values().collect();
        for row in &mut rows {
            row.lock = if !row.managed_in.is_empty() && row.managed_in.len() as u32 == row.present {
                Some(Lock::Managed)
            } else {
                builds
                    .iter()
                    .find_map(|b| taken(&groups, editing, &b.key, &row.path))
                    .map(|group| Lock::Taken { group })
            };
        }
        rows.sort_by(|a, b| {
            b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(rows)
    }
}

/// Where a build's folder keeps its mark for the sync groups.
const FOLDER_MARK: &str = ".launcher/sync-id";

/// The mark of the build folder `game`, made the first time: a folder made again has another.
fn folder_mark(game: &Path) -> AppResult<String> {
    let path = game.join(FOLDER_MARK);
    if let Some(mark) = fs::read_to_string(&path).ok().map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
    {
        return Ok(mark);
    }
    let mark = uuid::Uuid::new_v4().simple().to_string();
    let io_error = |e: std::io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::write(&path, &mark).map_err(io_error)?;
    Ok(mark)
}

/// The snapshot after a pass: the agreed versions, what each member holds now, and the builds
/// and paths that took part.
fn record(
    snapshot: &mut Snapshot,
    group: &SyncGroup,
    members: &[Member],
    found: &BTreeMap<String, BTreeMap<String, Found>>,
    untouchable: &BTreeMap<String, Vec<String>>,
    outcome: &Outcome,
    applied: &Applied,
) {
    for (k, agreed) in &outcome.agreed {
        let record = snapshot
            .files
            .entry(k.clone())
            .or_insert_with(|| FileRecord { path: k.clone(), ..FileRecord::default() });
        record.agreed = agreed.clone();
        if let Some(path) = outcome
            .writes
            .iter()
            .find(|w| &w.key == k)
            .map(|w| w.path.clone())
            .or_else(|| found.values().find_map(|files| files.get(k)).map(|f| f.path.clone()))
        {
            record.path = path;
        }
    }
    let keys: BTreeSet<String> = snapshot.files.keys().cloned().collect();
    for member in members {
        let files = found.get(&member.key);
        let own = untouchable.get(&member.key);
        for k in &keys {
            // A file it could not touch: what it held is still what it holds.
            if own.is_some_and(|own| is_managed(k, own)) {
                continue;
            }
            let Some(record) = snapshot.files.get_mut(k) else { continue };
            let removed = outcome.removes.iter().any(|r| r.build == member.key && &r.key == k);
            if let Some(seen) = applied.seen.get(&(member.key.clone(), k.clone())) {
                record.held.insert(member.key.clone(), seen.clone());
            } else if let Some(file) = files.and_then(|f| f.get(k)).filter(|_| !removed) {
                let seen = Seen { hash: file.hash.clone(), size: file.size, modified_ms: file.modified_ms };
                record.held.insert(member.key.clone(), seen);
            } else {
                record.held.remove(&member.key);
            }
        }
    }
    let covered = |k: &str| {
        group.paths.iter().any(|p| {
            let p = key(p);
            k == p || k.starts_with(&format!("{p}/"))
        })
    };
    snapshot.files.retain(|k, _| covered(k));
    snapshot.members.extend(members.iter().map(|m| m.key.clone()));
    snapshot.paths = group.paths.iter().cloned().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_group_reads_as_the_sync_message() {
        let group = SyncGroup { id: "g1".into(), name: "Карта".into(), ..SyncGroup::default() };
        let e = busy(&group);
        assert_eq!(e.params.get("message").map(String::as_str), Some("sync_error_busy"));
        assert_eq!(e.params.get("group").map(String::as_str), Some("Карта"));
    }
}
