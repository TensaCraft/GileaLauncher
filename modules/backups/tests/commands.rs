use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use launcher_core::feedback::{FeedbackService, NullSink};
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::ErrorCode;
use module_backups::backend::service::{BackupsService, Deps};
use module_backups::dto::BackupSettings;

struct World {
    _tmp: tempfile::TempDir,
    config: Arc<ConfigStore>,
    running: Arc<AtomicBool>,
    service: BackupsService,
    key: String,
    game: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("mc"));
    fs::create_dir_all(&state).unwrap();
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let mut build = Build::new("Aero");
    build.version = Some("1.21.1".into());
    versions.create(&mut build).unwrap();
    let game = game_dir(&build, versions.minecraft_dir());
    let config = Arc::new(ConfigStore::open(state.join("config.json")));
    let running = Arc::new(AtomicBool::new(false));
    let seen = running.clone();
    let service = BackupsService::new(Deps {
        config: config.clone(),
        versions,
        instances: Arc::new(Coordinator::instances()),
        feedback: FeedbackService::new(Arc::new(NullSink)),
        running: Arc::new(move |_: &str| seen.load(Ordering::SeqCst)),
    });
    World { _tmp: tmp, config, running, service, key: build.key, game }
}

fn add_world(w: &World, name: &str) {
    let dir = w.game.join("saves").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("level.dat"), b"level").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn worlds_backups_and_actions_go_through_the_service() {
    let w = world();
    add_world(&w, "World");
    let made = w.service.create(&w.key, "World").await.unwrap();
    assert_eq!(w.service.worlds(&w.key).await.unwrap()[0].backups, 1);
    fs::write(w.game.join("saves/World/level.dat"), b"changed").unwrap();
    w.service.restore(&w.key, "World", &made.zip_name).await.unwrap();
    assert_eq!(fs::read(w.game.join("saves/World/level.dat")).unwrap(), b"level");
    w.service.delete(&w.key, "World", &made.zip_name).await.unwrap();
    assert!(w.service.backups(&w.key, "World").await.unwrap().is_empty());
    w.service.create(&w.key, "World").await.unwrap();
    w.service.delete_build(&w.key).await.unwrap();
    assert!(w.service.backups(&w.key, "World").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_touches_the_worlds_of_a_running_game() {
    let w = world();
    add_world(&w, "World");
    let made = w.service.create(&w.key, "World").await.unwrap();
    w.running.store(true, Ordering::SeqCst);
    for result in [
        w.service.create(&w.key, "World").await.map(|_| ()),
        w.service.restore(&w.key, "World", &made.zip_name).await,
        w.service.delete(&w.key, "World", &made.zip_name).await,
    ] {
        assert_eq!(result.unwrap_err().code, ErrorCode::GameRunning);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_build_or_world_is_named() {
    let w = world();
    assert_eq!(w.service.worlds("ghost").await.unwrap_err().code, ErrorCode::VersionNotFound);
    assert!(w.service.create(&w.key, "no such world").await.is_err());
    assert!(w.service.worlds(&w.key).await.unwrap().is_empty());
}

#[test]
fn settings_are_checked_and_the_default_folder_is_not_stored() {
    let w = world();
    let s = w.service.settings();
    assert_eq!((s.enabled, s.keep), (false, 3));
    assert!(w.service.set_settings(BackupSettings { keep: 0, ..s.clone() }).is_err());
    let saved = w
        .service
        .set_settings(BackupSettings { enabled: true, keep: 5, dir: s.default_dir.clone(), ..s })
        .unwrap();
    assert_eq!((saved.enabled, saved.keep), (true, 5));
    assert_eq!(w.config.get("world_backups_dir"), None);
    assert_eq!(w.config.get_str("world_backups_enabled").as_deref(), Some("yes"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_build_s_backups_are_not_deleted() {
    let w = world();
    add_world(&w, "World");
    w.service.create(&w.key, "World").await.unwrap();
    w.running.store(true, Ordering::SeqCst);
    assert_eq!(w.service.delete_build(&w.key).await.unwrap_err().code, ErrorCode::GameRunning);
    w.running.store(false, Ordering::SeqCst);
    assert_eq!(w.service.backups(&w.key, "World").await.unwrap().len(), 1, "they stay while the game runs");
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_builds_backups_touches_only_its_backups() {
    let w = world();
    add_world(&w, "World");
    // The backups folder is a folder of other things too, one of them named as the build is.
    let games = w._tmp.path().join("Games");
    w.config.set("world_backups_dir", serde_json::json!(games.to_string_lossy())).unwrap();
    let made = w.service.create(&w.key, "World").await.unwrap();
    let folders: Vec<PathBuf> = fs::read_dir(&games).unwrap().map(|e| e.unwrap().path()).collect();
    let [build_dir] = folders.as_slice() else { panic!("one build folder: {folders:?}") };
    let world_dir = build_dir.join("World");
    assert!(world_dir.join(&made.zip_name).is_file());
    fs::write(build_dir.join("notes.txt"), b"mine").unwrap();
    fs::create_dir_all(build_dir.join("Other")).unwrap();
    fs::write(build_dir.join("Other/keep.txt"), b"mine").unwrap();
    fs::write(world_dir.join("mine.zip"), b"not a backup").unwrap();
    fs::write(world_dir.join("readme.txt"), b"mine").unwrap();
    fs::write(world_dir.join("theirs.zip"), b"another build's").unwrap();
    let theirs = serde_json::json!({"schema": 1, "kind": "manual", "version_id": "someone-else",
        "world_folder": "World", "zip_name": "theirs.zip", "created_timestamp": 1.0});
    fs::write(world_dir.join("theirs.zip.json"), theirs.to_string()).unwrap();

    w.service.delete_build(&w.key).await.unwrap();

    assert!(!world_dir.join(&made.zip_name).exists(), "its backup is gone");
    assert!(!world_dir.join(format!("{}.json", made.zip_name)).exists(), "with its metadata");
    for kept in ["notes.txt", "Other/keep.txt", "World/mine.zip", "World/readme.txt", "World/theirs.zip"] {
        assert!(build_dir.join(kept).is_file(), "{kept} stays");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_left_empty_by_deleting_a_builds_backups_go() {
    let w = world();
    add_world(&w, "World");
    let games = w._tmp.path().join("Games");
    w.config.set("world_backups_dir", serde_json::json!(games.to_string_lossy())).unwrap();
    w.service.create(&w.key, "World").await.unwrap();
    w.service.delete_build(&w.key).await.unwrap();
    assert_eq!(fs::read_dir(&games).unwrap().count(), 0, "nothing of the build is left");
    assert!(games.is_dir(), "the backups folder itself stays");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_world_open_in_a_game_the_launcher_does_not_know_keeps_its_backups_away() {
    let w = world();
    add_world(&w, "World");
    let made = w.service.create(&w.key, "World").await.unwrap();
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(w.game.join("saves/World/session.lock"))
        .unwrap();
    lock.lock().unwrap();
    fs::write(w.game.join("saves/World/level.dat"), b"played").unwrap();
    let err = w.service.restore(&w.key, "World", &made.zip_name).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::GameRunning);
    assert_eq!(fs::read(w.game.join("saves/World/level.dat")).unwrap(), b"played", "the open world stays");
    drop(lock);
}
