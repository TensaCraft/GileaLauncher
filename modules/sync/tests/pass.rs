//! Sync passes over real folders: what each build ends up with, what history keeps, and what is
//! never touched.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use launcher_core::backups::resolver;
use launcher_core::feedback::{FeedbackService, NullSink};
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use module_sync::backend::service::{Deps, SyncService};
use module_sync::dto::{GroupState, Lock, Reason, SyncGroup, SyncMode};

struct World {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    state: PathBuf,
    versions: Arc<VersionStore>,
    instances: Arc<Coordinator>,
    running: Arc<Mutex<HashSet<String>>>,
    managed: Arc<Mutex<HashMap<String, Vec<String>>>>,
    service: SyncService,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let mc = tmp.path().join("minecraft");
    let state = tmp.path().join("state");
    fs::create_dir_all(&mc).unwrap();
    let config = Arc::new(ConfigStore::open(tmp.path().join("config.json")));
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let instances = Arc::new(Coordinator::instances());
    let running = Arc::new(Mutex::new(HashSet::new()));
    let managed: Arc<Mutex<HashMap<String, Vec<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let service = SyncService::new(Deps {
        versions: versions.clone(),
        instances: instances.clone(),
        feedback: FeedbackService::new(Arc::new(NullSink)),
        config: config.clone(),
        backups: resolver(config, mc.clone()),
        state_dir: state.clone(),
        running: {
            let running = running.clone();
            Arc::new(move |key: &str| running.lock().unwrap().contains(key))
        },
        managed: {
            let managed = managed.clone();
            Arc::new(move |build: &Build, _game: &Path| {
                managed.lock().unwrap().get(&build.key).cloned().unwrap_or_default()
            })
        },
    });
    World { _tmp: tmp, mc, state, versions, instances, running, managed, service }
}

impl World {
    /// A build's key and folder.
    fn build(&self, name: &str) -> (String, PathBuf) {
        let mut build = Build::new(name);
        self.versions.create(&mut build).unwrap();
        (build.key.clone(), game_dir(&build, &self.mc))
    }

    fn group(&self, builds: &[&str], paths: &[&str], mode: SyncMode) -> String {
        let group = SyncGroup {
            id: String::new(),
            name: "Група".into(),
            builds: builds.iter().map(|b| b.to_string()).collect(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            mode,
            start: builds[0].to_string(),
        };
        self.service.save(group).unwrap().group.id
    }
}

/// Writes `relative` in `game`, last changed `ago` before now.
fn put(game: &Path, relative: &str, body: &str, ago: u64) {
    let path = game.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    let at = SystemTime::now() - Duration::from_secs(ago);
    fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
}

fn read(game: &Path, relative: &str) -> Option<String> {
    fs::read_to_string(game.join(relative)).ok()
}

#[test]
fn two_builds_take_the_newer_file() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    put(&ga, "options.txt", "keys 1", 600);
    let id = w.group(&[&a, &b], &["options.txt"], SyncMode::Latest);
    let first = w.service.sync_group(&id, None).unwrap();
    assert_eq!((first.written, read(&gb, "options.txt").as_deref()), (1, Some("keys 1")));
    put(&gb, "options.txt", "keys 2", 60);
    w.service.sync_group(&id, None).unwrap();
    assert_eq!(read(&ga, "options.txt").as_deref(), Some("keys 2"), "the edit made last wins");
    let history = w.service.history(&id).unwrap();
    assert_eq!((history[0].build.as_str(), history[0].reason), (a.as_str(), Reason::Changed));
    assert_eq!(w.service.groups()[0].state, GroupState::Synced);
}

#[test]
fn three_builds_and_a_removal() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    let (c, gc) = w.build("Zeta");
    put(&ga, "config/jei/x.json", "{}", 600);
    let id = w.group(&[&a, &b, &c], &["config"], SyncMode::Latest);
    w.service.sync_group(&id, None).unwrap();
    assert!(read(&gb, "config/jei/x.json").is_some() && read(&gc, "config/jei/x.json").is_some());
    fs::remove_file(gc.join("config/jei/x.json")).unwrap();
    let report = w.service.sync_group(&id, None).unwrap();
    assert_eq!(report.removed, 2);
    assert!(read(&ga, "config/jei/x.json").is_none() && read(&gb, "config/jei/x.json").is_none());
    assert!(w.service.history(&id).unwrap().iter().any(|h| h.reason == Reason::Removed));
}

#[test]
fn a_running_build_is_skipped_then_caught_up() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    let (c, gc) = w.build("Zeta");
    put(&ga, "options.txt", "keys 1", 600);
    let id = w.group(&[&a, &b, &c], &["options.txt"], SyncMode::Latest);
    w.service.sync_group(&id, None).unwrap();
    w.running.lock().unwrap().insert(c.clone());
    put(&ga, "options.txt", "keys 2", 60);
    w.service.sync_group(&id, None).unwrap();
    assert_eq!(read(&gb, "options.txt").as_deref(), Some("keys 2"));
    assert_eq!(read(&gc, "options.txt").as_deref(), Some("keys 1"), "nothing written to a running build");
    w.running.lock().unwrap().clear();
    w.service.sync_group(&id, None).unwrap();
    assert_eq!(read(&gc, "options.txt").as_deref(), Some("keys 2"), "caught up");
}

#[test]
fn a_held_member_is_not_written() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    let (c, gc) = w.build("Zeta");
    put(&ga, "options.txt", "keys", 600);
    let id = w.group(&[&a, &b, &c], &["options.txt"], SyncMode::Latest);
    let lease = w.instances.try_acquire(&gb, "install").unwrap();
    w.service.sync_group(&id, None).unwrap();
    assert!(read(&gb, "options.txt").is_none(), "another operation holds it");
    assert_eq!(read(&gc, "options.txt").as_deref(), Some("keys"));
    drop(lease);
    // The launching build: its lease is the launch's, and the pass writes into it.
    let launch = w.instances.try_acquire(&gb, "launch").unwrap();
    w.service.sync_group(&id, Some(&b)).unwrap();
    assert_eq!(read(&gb, "options.txt").as_deref(), Some("keys"));
    drop(launch);
}

#[test]
fn a_managed_path_is_never_written() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Server");
    put(&ga, "config/jei/x.json", "{}", 600);
    put(&ga, "config/server/a.toml", "mine", 600);
    put(&gb, "config/server/a.toml", "the server's", 600);
    w.managed.lock().unwrap().insert(b.clone(), vec!["config/server".into()]);
    let id = w.group(&[&a, &b], &["config"], SyncMode::Latest);
    w.service.sync_group(&id, None).unwrap();
    assert!(read(&gb, "config/jei/x.json").is_some());
    assert_eq!(read(&gb, "config/server/a.toml").as_deref(), Some("the server's"));
}

#[test]
fn the_base_mode_mirrors_the_base() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    put(&ga, "xaero/minimap.txt", "base", 600);
    put(&gb, "xaero/minimap.txt", "mine", 10);
    put(&gb, "xaero/extra.txt", "only mine", 10);
    let id = w.group(&[&a, &b], &["xaero"], SyncMode::Base);
    w.service.sync_group(&id, None).unwrap();
    assert_eq!(read(&gb, "xaero/minimap.txt").as_deref(), Some("base"), "though older");
    assert!(read(&gb, "xaero/extra.txt").is_none());
    let history = w.service.history(&id).unwrap();
    assert!(history.iter().all(|h| h.reason == Reason::Base && h.build == b) && history.len() == 2);
}

#[test]
fn a_restore_in_base_mode_goes_into_the_base() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    put(&ga, "options.txt", "base", 600);
    put(&gb, "options.txt", "lite's", 10);
    let id = w.group(&[&a, &b], &["options.txt"], SyncMode::Base);
    w.service.sync_group(&id, None).unwrap();
    let kept = w.service.history(&id).unwrap().remove(0);
    w.service.restore(&id, &kept.id).unwrap();
    assert_eq!(read(&ga, "options.txt").as_deref(), Some("lite's"), "into the base");
    assert_eq!(read(&gb, "options.txt").as_deref(), Some("lite's"), "and spread");
}

#[test]
fn the_snapshot_is_untouched_after_a_failed_pass() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Lite");
    let (c, gc) = w.build("Zeta");
    put(&ga, "options.txt", "keys 1", 600);
    let id = w.group(&[&a, &b, &c], &["options.txt"], SyncMode::Latest);
    w.service.sync_group(&id, None).unwrap();
    let snapshot = fs::read(w.state.join("sync").join(format!("{id}.json"))).unwrap();
    put(&ga, "options.txt", "keys 2", 60);
    fs::remove_file(gc.join("options.txt")).unwrap();
    fs::create_dir_all(gc.join("options.txt/in")).unwrap();
    assert!(w.service.sync_group(&id, None).is_err());
    assert_eq!(read(&gb, "options.txt").as_deref(), Some("keys 1"), "put back");
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(w.state.join("sync").join(format!("{id}.json"))).unwrap()).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
    assert_eq!(after["files"], before["files"], "what the builds hold is as it was");
    assert_eq!(w.service.groups()[0].state, GroupState::Failed);
}

#[test]
fn the_tree_is_the_builds_union_with_locks() {
    let w = world();
    let (a, ga) = w.build("Aero");
    let (b, gb) = w.build("Server");
    let (c, _) = w.build("Zeta");
    put(&ga, "options.txt", "a", 1);
    put(&ga, "xaero/minimap.txt", "a", 1);
    put(&ga, "saves/World/level.dat", "a", 1);
    put(&gb, "options.txt", "b", 1);
    put(&gb, "servers.dat", "b", 1);
    put(&gb, "config/server/a.toml", "b", 1);
    put(&gb, "logs/latest.log", "b", 1);
    w.managed.lock().unwrap().insert(b.clone(), vec!["config/server".into()]);
    w.group(&[&b, &c], &["servers.dat"], SyncMode::Latest);
    let builds = vec![a.clone(), b.clone()];
    let root = w.service.tree(&builds, "", None).unwrap();
    let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["config", "xaero", "options.txt", "servers.dat"],
        "folders first; no worlds, no logs"
    );
    let options = root.iter().find(|e| e.name == "options.txt").unwrap();
    assert_eq!((options.present, options.total), (2, 2));
    let servers = root.iter().find(|e| e.name == "servers.dat").unwrap();
    assert_eq!(servers.lock, Some(Lock::Taken { group: "Група".into() }));
    assert_eq!(root.iter().find(|e| e.name == "config").unwrap().lock, None, "partly managed: still chosen");
    let config = w.service.tree(&builds, "config", None).unwrap();
    assert_eq!(config[0].lock, Some(Lock::Managed));
    assert_eq!(config[0].managed_in, vec!["Server".to_string()]);
}
