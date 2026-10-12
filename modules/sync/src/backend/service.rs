//! The sync service behind the commands, the launch hook and the game watcher: groups, a pass
//! over a group, the editor's tree, history and restore.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use launcher_core::backups::{BackupsRoot, BuildBackups};
use launcher_core::feedback::{FeedbackService, OperationSpec};
use launcher_core::launch::alive::game_open;
use launcher_core::launch::options::game_dir;
use launcher_core::lock::{Coordinator, Lease, LockError, LockPlace, OsFileLock};
use launcher_core::net::preflight::{SpaceRequest, preflight};
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::transaction::{ApplyHooks, TransactionPlan, execute};
use launcher_core::storage::usage::dir_size;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{AppError, AppResult, ErrorCode, Level, Text};

use super::apply::{Applied, Member, SYNC_JOURNAL, apply, keep};
use super::blacklist::blocked;
use super::decide::{Decision, Outcome, Saved, decide};
use super::history;
use super::paths::{key, normalize, taken};
use super::scan::{Found, is_managed, scan};
use super::store::{FileRecord, Seen, Snapshot, Store};
use crate::dto::{
    GroupState, GroupView, HistoryEntry, Lock, PassReport, Reason, SyncGroup, SyncMode, TreeEntry,
};

/// A pass that writes more shows as an operation; a smaller one passes quietly.
const QUIET_BYTES: u64 = 8 * 1024 * 1024;
/// How long a pass waits for another pass of its group to end.
const LOCK_WAIT: Duration = Duration::from_secs(30);

/// The paths of a build its modules keep in line themselves (`Module::managed_files`).
pub type ManagedFiles = Arc<dyn Fn(&Build, &Path) -> Vec<String> + Send + Sync>;

/// What the service needs of the launcher.
pub struct Deps {
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub config: Arc<ConfigStore>,
    pub backups: BackupsRoot,
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

    fn shelf(&self, build: &Build) -> BuildBackups {
        BuildBackups::of_build(&(self.deps.backups)(), build)
    }

    fn running(&self, build: &Build) -> bool {
        (self.deps.running)(&build.key) || game_open(&self.game(build))
    }

    fn view(&self, group: SyncGroup) -> GroupView {
        let snapshot = self.store.snapshot(&group.id);
        let existing = group.builds.iter().filter(|b| self.deps.versions.get(b).is_some()).count();
        let state = if existing < 2 { GroupState::Idle } else { snapshot.state };
        GroupView { group, last_ms: snapshot.last_ms, state, note: snapshot.note }
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
        let mut snapshot = self.store.snapshot(id);
        let result = self.pass(&group, &mut snapshot, held);
        if let Err(e) = &result {
            tracing::warn!("Sync of {} failed: {}", group.name, e.detail);
            let mut failed = self.store.snapshot(id);
            failed.state = GroupState::Failed;
            failed.note = Some(Text::raw(e.detail.clone()));
            let _ = self.store.write_snapshot(id, &failed);
        }
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
            if held != Some(key.as_str()) {
                if self.running(&build) {
                    continue;
                }
                match self.deps.instances.try_acquire(&game, "sync") {
                    Ok(lease) => leases.push(lease),
                    Err(_) => continue,
                }
            }
            let member = Member { key: key.clone(), game, shelf: self.shelf(&build) };
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
        if members.len() < 2 {
            return self.wait(group, snapshot);
        }
        let mut found: BTreeMap<String, BTreeMap<String, Found>> = BTreeMap::new();
        let mut managed: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (build, member) in &members {
            let own = (self.deps.managed)(build, &member.game);
            found.insert(
                member.key.clone(),
                scan(&member.game, &group.paths, &own, &snapshot.files, &member.key),
            );
            managed.insert(member.key.clone(), own);
        }
        let keys: Vec<String> = members.iter().map(|(_, m)| m.key.clone()).collect();
        let mut outcome = decide(&Decision {
            mode: group.mode,
            start: &group.start,
            members: &keys,
            snapshot,
            found: &found,
            group_paths: &group.paths,
        });
        if outcome.wait {
            return self.wait(group, snapshot);
        }
        // What a module keeps in line in a build is never written or removed there.
        let mine = |build: &str, path: &str| managed.get(build).is_some_and(|own| is_managed(path, own));
        outcome.writes.retain(|w| !mine(&w.build, &w.path));
        outcome.removes.retain(|r| !mine(&r.build, &r.path));
        outcome.history.retain(|h| !mine(&h.build, &h.path));
        let members: Vec<Member> = members.into_iter().map(|(_, m)| m).collect();
        let size_of =
            |build: &str, key: &str| found.get(build).and_then(|f| f.get(key)).map_or(0, |f| f.size);
        let mut requests: Vec<SpaceRequest> = members
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
        let kept: u64 = outcome.history.iter().map(|s| size_of(&s.build, &s.key)).sum();
        if kept > 0 {
            requests.push(SpaceRequest {
                dir: (self.deps.backups)(),
                bytes: kept,
                label: group.name.clone(),
            });
        }
        preflight(&requests)?;
        let now = SystemTime::now();
        let total: u64 = requests.iter().map(|r| r.bytes).sum();
        let op = (total > QUIET_BYTES).then(|| {
            self.deps
                .feedback
                .begin(OperationSpec::new(Text::key("sync_operation").param("group", &group.name), "sync"))
        });
        let applied = match apply(&members, &outcome, group, now) {
            Ok(applied) => applied,
            Err(e) => {
                if let Some(op) = op {
                    op.fail(Text::raw(e.detail.clone()));
                }
                return Err(e);
            }
        };
        let quiet = op.is_none();
        if let Some(op) = op {
            op.finish();
        }
        record(snapshot, group, &members, &found, &outcome, &applied);
        snapshot.last_ms = Some(millis(now));
        snapshot.state = GroupState::Synced;
        snapshot.note = None;
        self.store.write_snapshot(&group.id, snapshot)?;
        for member in &members {
            history::sweep(&member.shelf, &self.deps.config, now);
        }
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
        for path in &report.conflicts {
            let title = Text::key("sync_conflict").param("group", &group.name).param("path", path);
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

    /// Every existing build of `group`, for its history.
    fn all_members(&self, group: &SyncGroup) -> Vec<Member> {
        group
            .builds
            .iter()
            .filter_map(|k| self.deps.versions.get(k))
            .map(|b| Member { key: b.key.clone(), game: self.game(&b), shelf: self.shelf(&b) })
            .collect()
    }

    pub fn history(&self, id: &str) -> AppResult<Vec<HistoryEntry>> {
        let group = self.store.group(id)?;
        Ok(history::entries(&self.all_members(&group), id))
    }

    /// Puts history entry `entry` back as a new change: into the base (mode `Base`), else into
    /// the build it was kept from (another that is not running, when that one is), dated now so it
    /// is the newest; then a pass spreads it.
    pub fn restore(&self, id: &str, entry: &str) -> AppResult<PassReport> {
        let group = self.store.group(id)?;
        let all = self.all_members(&group);
        let not_found = || {
            AppError::new(ErrorCode::NotFound, format!("no kept version {entry}")).with_param("path", entry)
        };
        let file = history::version_file(&all, entry).ok_or_else(not_found)?;
        let (owner, rest) = entry.split_once('/').ok_or_else(not_found)?;
        let path = normalize(rest.split_once('/').ok_or_else(not_found)?.1)?;
        let idle = |key: &str| self.deps.versions.get(key).filter(|b| !self.running(b));
        let target = match group.mode {
            SyncMode::Base => idle(&group.start),
            SyncMode::Latest => idle(owner).or_else(|| group.builds.iter().find_map(|k| idle(k))),
        }
        .ok_or_else(|| {
            AppError::new(ErrorCode::InstanceBusy, "the build to restore into runs")
                .with_param("version", &group.start)
        })?;
        {
            let _lock = self.lock_group(&group)?;
            let game = self.game(&target);
            let _lease = self.deps.instances.try_acquire(&game, "sync")?;
            let member = Member { key: target.key.clone(), game: game.clone(), shelf: self.shelf(&target) };
            if game.join(&path).is_file() {
                let saved = Saved {
                    build: target.key.clone(),
                    key: key(&path),
                    path: path.clone(),
                    from: Some(owner.to_string()),
                    reason: Reason::Restored,
                };
                keep(&member, &[&saved], &group, SystemTime::now())?;
            }
            let mut plan = TransactionPlan::new("build-sync-restore");
            plan.replacements = vec![path.clone()];
            plan.staged_bytes = fs::metadata(&file).map_or(0, |m| m.len());
            execute(
                &game,
                SYNC_JOURNAL,
                plan,
                |transaction| {
                    let staged = transaction.stage_path(&path)?;
                    fs::copy(&file, &staged).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
                    fs::File::options()
                        .write(true)
                        .open(&staged)
                        .and_then(|f| f.set_modified(SystemTime::now()))
                        .map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
                },
                ApplyHooks::default(),
            )?;
        }
        self.sync_group(id, None)
    }
}

/// The snapshot after a pass: the agreed versions, what each member holds now, and the builds
/// and paths that took part.
fn record(
    snapshot: &mut Snapshot,
    group: &SyncGroup,
    members: &[Member],
    found: &BTreeMap<String, BTreeMap<String, Found>>,
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
        for k in &keys {
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
