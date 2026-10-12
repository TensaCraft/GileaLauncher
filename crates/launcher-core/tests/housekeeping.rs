//! The storage service: what the «Сховище» tab shows and clears, and the once-a-day clean.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use launcher_core::backups::{BuildBackups, resolver};
use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::housekeeping::{StorageDeps, StorageService};
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{
    ActivityEntry, Alert, ErrorCode, OpsSnapshot, StorageLine, StorageRule, StorageSnapshot, Text, Toast,
};
use serde_json::json;

const DAY: Duration = Duration::from_secs(24 * 3600);

#[derive(Default)]
struct Rec {
    activity: Mutex<Vec<ActivityEntry>>,
}

impl EventSink for Rec {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, entry: &ActivityEntry) {
        self.activity.lock().unwrap().push(entry.clone());
    }
    fn toast(&self, _: &Toast) {}
    fn alert(&self, _: &Alert) {}
}

impl Rec {
    fn keys(&self) -> Vec<String> {
        let entries = self.activity.lock().unwrap();
        entries
            .iter()
            .filter_map(|e| match &e.message {
                Text::Key { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }
}

struct World {
    tmp: tempfile::TempDir,
    mc: PathBuf,
    cache: PathBuf,
    config: Arc<ConfigStore>,
    versions: Arc<VersionStore>,
    instances: Arc<Coordinator>,
    rec: Arc<Rec>,
    running: Arc<Mutex<HashSet<String>>>,
    service: StorageService,
}

fn world() -> World {
    world_with_config(|_| {})
}

fn world_with_config(prepare: impl FnOnce(&Path)) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let mc = tmp.path().join("minecraft");
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&mc).unwrap();
    prepare(&tmp.path().join("config.json"));
    let config = Arc::new(ConfigStore::open(tmp.path().join("config.json")));
    let versions = Arc::new(VersionStore::open(&tmp.path().join("state"), &mc));
    let instances = Arc::new(Coordinator::instances());
    let rec = Arc::new(Rec::default());
    let running = Arc::new(Mutex::new(HashSet::new()));
    let service = StorageService::new(StorageDeps {
        config: config.clone(),
        versions: versions.clone(),
        instances: instances.clone(),
        feedback: FeedbackService::new(rec.clone()),
        backups: resolver(config.clone(), mc.clone()),
        cache_dir: cache.clone(),
        running: {
            let running = running.clone();
            Arc::new(move |key: &str| running.lock().unwrap().contains(key))
        },
        clear_updates: Arc::new(|| Ok(())),
        lang: Arc::new(|| "uk_UA".to_string()),
    });
    World { tmp, mc, cache, config, versions, instances, rec, running, service }
}

/// A build: modded (Fabric) or vanilla. Its key and game folder.
fn build(w: &World, name: &str, modded: bool) -> (String, PathBuf) {
    let mut build = Build::new(name);
    build.version = Some("1.21.1".into());
    if modded {
        build.loader = Some("fabric-loader-0.16.9-1.21.1".into());
    }
    w.versions.create(&mut build).unwrap();
    let game = game_dir(&build, &w.mc);
    (build.key.clone(), game)
}

fn shelf(w: &World, key: &str) -> BuildBackups {
    BuildBackups::of_build(&w.mc.join("backups"), &w.versions.get(key).unwrap())
}

fn put(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

fn aged(path: &Path, age: Duration) {
    fs::File::options().write(true).open(path).unwrap().set_modified(SystemTime::now() - age).unwrap();
}

fn line(snapshot: &StorageSnapshot, key: &str, line: StorageLine) -> u64 {
    let build = snapshot.builds.iter().find(|b| b.key == key).unwrap();
    build.lines.iter().find(|l| l.line == line).unwrap().bytes
}

fn launcher_line(snapshot: &StorageSnapshot, line: StorageLine) -> u64 {
    snapshot.launcher.iter().find(|l| l.line == line).unwrap().bytes
}

fn world_backup(w: &World, key: &str) -> [PathBuf; 2] {
    let worlds = shelf(w, key).worlds();
    let meta = json!({"world_folder": "World"}).to_string();
    [put(&worlds, "World/a.zip", b"zip"), put(&worlds, "World/a.zip.json", meta.as_bytes())]
}

#[test]
fn the_snapshot_measures_each_build_and_the_launcher() {
    let w = world();
    let (aero, game) = build(&w, "Aero", true);
    let (plain, _) = build(&w, "Plain", false);
    put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"12345");
    put(&game, "crash-reports/crash.txt", b"abc");
    put(&w.cache, "thumbs/ab.jpg", b"1234");
    let snapshot = w.service.snapshot(false);
    assert_eq!(line(&snapshot, &aero, StorageLine::ModBackups), 5);
    assert_eq!(line(&snapshot, &aero, StorageLine::CrashReports), 3);
    assert_eq!(launcher_line(&snapshot, StorageLine::Thumbnails), 4);
    let vanilla = |key: &str| snapshot.builds.iter().find(|b| b.key == key).unwrap().vanilla;
    assert!(!vanilla(&aero) && vanilla(&plain));
    assert_eq!(snapshot.rules.len(), StorageRule::ALL.len());
    assert!(snapshot.rules.iter().all(|r| r.on), "every rule is on at first");
    assert!(snapshot.free.is_some());

    put(&game, "crash-reports/more.txt", b"more");
    assert_eq!(line(&w.service.snapshot(false), &aero, StorageLine::CrashReports), 3, "kept until a refresh");
    assert_eq!(line(&w.service.snapshot(true), &aero, StorageLine::CrashReports), 7);
}

#[test]
fn clearing_a_line_frees_it_and_notes_it() {
    let w = world();
    let (aero, _) = build(&w, "Aero", true);
    let copy = put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"12345");
    // Another size than the copy's: the same note twice in a row is shown once.
    let thumb = put(&w.cache, "thumbs/ab.jpg", &[0; 3000]);
    w.service.snapshot(false);
    let after = w.service.clear(Some(&aero), StorageLine::ModBackups).unwrap();
    assert!(!copy.exists());
    assert_eq!(line(&after, &aero, StorageLine::ModBackups), 0);
    let after = w.service.clear(None, StorageLine::Thumbnails).unwrap();
    assert!(!thumb.exists());
    assert_eq!(launcher_line(&after, StorageLine::Thumbnails), 0);
    assert_eq!(w.rec.keys(), vec!["storage_cleaned", "storage_cleaned"]);
}

#[test]
fn clearing_everything_unneeded_keeps_world_backups_and_journals() {
    let w = world();
    let (aero, game) = build(&w, "Aero", true);
    let kept = world_backup(&w, &aero);
    let journal =
        put(&game, ".launcher/journal/tensa.json", json!({"status": "complete"}).to_string().as_bytes());
    let note = put(&game, ".launcher/tensa-checked.json", b"{}");
    let gone = [
        put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"old"),
        put(&game, ".launcher/launch.log", b"started"),
        put(&game, "logs/old.log.gz", b"gz"),
        put(&w.cache, "thumbs/ab.jpg", b"jpg"),
    ];
    let temp = put(&game, "mods/x.jar.part.json", b"{}");
    aged(&temp, 2 * DAY);
    let after = w.service.clear_unneeded().unwrap();
    for path in kept.iter().chain([&journal, &note]) {
        assert!(path.is_file(), "{} stays", path.display());
    }
    for path in gone.iter().chain([&temp]) {
        assert!(!path.exists(), "{} goes", path.display());
    }
    assert!(after.unneeded().is_empty());
    assert_eq!(w.rec.keys(), vec!["storage_cleaned"], "one note for the whole");
}

#[test]
fn a_running_build_is_neither_cleared_nor_cleaned() {
    let w = world();
    let (aero, game) = build(&w, "Aero", true);
    let (zeta, zeta_game) = build(&w, "Zeta", true);
    let copy = put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"old");
    aged(&copy, 40 * DAY);
    let busy_copy = put(&shelf(&w, &zeta).mods(), "sodium.jar.backup", b"old");
    aged(&busy_copy, 40 * DAY);
    w.running.lock().unwrap().insert(aero.clone());
    let lease = w.instances.try_acquire(&zeta_game, "launch").unwrap();
    let refused = w.service.clear(Some(&aero), StorageLine::ModBackups).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InstanceBusy);
    assert_eq!(
        w.service.clear(Some(&zeta), StorageLine::ModBackups).unwrap_err().code,
        ErrorCode::InstanceBusy
    );
    w.service.clear_unneeded().unwrap();
    assert_eq!(w.service.auto_clean(SystemTime::now()), 0);
    assert!(copy.is_file() && busy_copy.is_file());
    assert!(w.service.snapshot(false).builds.iter().find(|b| b.key == aero).unwrap().running);
    drop(lease);
    let _ = game;
}

#[test]
fn auto_cleaning_runs_once_a_day_and_follows_the_switches() {
    let w = world();
    let (aero, game) = build(&w, "Aero", true);
    let copy = put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"old");
    let dump = put(&game, "hs_err_pid1.log", b"dump");
    aged(&copy, 31 * DAY);
    aged(&dump, 8 * DAY);
    w.service.set_rule(StorageRule::JavaDumps, false).unwrap();
    assert!(w.service.snapshot(false).rules.iter().any(|r| r.rule == StorageRule::JavaDumps && !r.on));
    let now = SystemTime::now();
    assert_eq!(w.service.auto_clean(now), 3);
    assert!(!copy.exists() && dump.is_file(), "a rule that is off keeps its files");
    assert_eq!(w.rec.keys(), vec!["storage_cleaned_auto"]);
    let late = put(&shelf(&w, &aero).mods(), "iris.jar.backup", b"old");
    aged(&late, 31 * DAY);
    assert_eq!(w.service.auto_clean(now + Duration::from_secs(3600)), 0, "once a day");
    assert_eq!(w.service.auto_clean(now + DAY + Duration::from_secs(60)), 3);
    assert!(!late.exists());
    let at = w.config.get_u64("storage_cleaned_at").unwrap();
    let expected =
        (now + DAY + Duration::from_secs(60)).duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    assert_eq!(at, expected);
}

#[test]
fn a_damaged_config_is_never_rewritten_by_cleaning() {
    let w = world_with_config(|path| fs::write(path, b"{ broken").unwrap());
    let (aero, _) = build(&w, "Aero", true);
    let copy = put(&shelf(&w, &aero).mods(), "sodium.jar.backup", b"old");
    aged(&copy, 31 * DAY);
    assert_eq!(w.service.auto_clean(SystemTime::now()), 0);
    assert_eq!(fs::read(w.tmp.path().join("config.json")).unwrap(), b"{ broken");
    assert!(copy.is_file());
    assert!(w.service.snapshot(false).rules.iter().all(|r| r.on));
}
