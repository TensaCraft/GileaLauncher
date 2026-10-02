use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::paths::{Os, PathEnv};
use launcher_core::updater::download::sha256_file;
use launcher_core::updater::select::{self, Platform};
use launcher_core::updater::stage::{self, ExecContext};
use launcher_core::updater::{UpdateConfig, UpdateService};
use launcher_shared::{ActivityEntry, Alert, ErrorCode, OpsSnapshot, Text, Toast, UpdateState, UpdateStatus};
use mock_github::{MockHandle, NewRelease, Scenario, add_release};

#[derive(Default)]
struct Rec {
    updates: Mutex<Vec<UpdateStatus>>,
    toasts: Mutex<Vec<Toast>>,
}

impl EventSink for Rec {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, t: &Toast) {
        self.toasts.lock().unwrap().push(t.clone());
    }
    fn alert(&self, _: &Alert) {}
    fn update(&self, s: &UpdateStatus) {
        self.updates.lock().unwrap().push(s.clone());
    }
}

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    exe: PathBuf,
}

const FIXTURE: &str = env!("CARGO_BIN_EXE_update-fixture");

fn world() -> World {
    world_in(&["app"])
}

fn world_in(folders: &[&str]) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let app = folders.iter().fold(tmp.path().to_path_buf(), |dir, f| dir.join(f));
    let exe = if cfg!(target_os = "macos") {
        app.join("Launcher.app").join("Contents").join("MacOS").join("Launcher")
    } else {
        app.join(if cfg!(windows) { "Launcher.exe" } else { "Launcher" })
    };
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"launcher 0.1.0").unwrap();
    World { root: tmp.path().join("mock"), cache: tmp.path().join("cache"), exe, _tmp: tmp }
}

/// The folder an update writes into: the program's, or on macOS the `.app`'s.
#[cfg(unix)]
fn install_folder(w: &World) -> PathBuf {
    let target = w.exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).unwrap_or(&w.exe);
    target.parent().unwrap().to_path_buf()
}

/// Keeps a folder read-only while it lives.
#[cfg(unix)]
struct ReadOnly(PathBuf);

#[cfg(unix)]
fn set_mode(folder: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
impl ReadOnly {
    fn new(folder: PathBuf) -> ReadOnly {
        set_mode(&folder, 0o555);
        ReadOnly(folder)
    }
}

#[cfg(unix)]
impl Drop for ReadOnly {
    fn drop(&mut self) {
        set_mode(&self.0, 0o755);
    }
}

fn publish(w: &World, version: &str) {
    std::fs::create_dir_all(&w.root).unwrap();
    let src = w.root.join(format!("payload-{version}"));
    std::fs::write(&src, format!("launcher {version}").repeat(5000)).unwrap();
    let name = select::preferred_asset_name(&Platform::current());
    let new = NewRelease { version, prerelease: false, notes: "Notes", asset_name: &name, source: &src };
    add_release(&w.root, new).unwrap();
}

async fn serve(w: &World, scenario: Scenario) -> MockHandle {
    mock_github::start(w.root.clone(), scenario, 0).await.unwrap()
}

/// The "launcher" is a real program here, so the helper copied from it can be started.
fn runnable_world() -> World {
    let w = world();
    std::fs::copy(FIXTURE, &w.exe).unwrap();
    w
}

fn service(w: &World, api: &str, repo: &str, dev_mode: bool) -> (Arc<UpdateService>, Arc<Rec>) {
    service_with_env(w, api, repo, dev_mode, PathEnv::default())
}

fn service_with_env(
    w: &World,
    api: &str,
    repo: &str,
    dev_mode: bool,
    env: PathEnv,
) -> (Arc<UpdateService>, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let feedback = FeedbackService::new(rec.clone());
    let exec = ExecContext {
        os: Os::current(),
        current_exe: w.exe.clone(),
        appimage: None,
        pid: std::process::id(),
    };
    let cfg = UpdateConfig {
        api_base: api.into(),
        repo: repo.into(),
        current_version: "0.1.0".into(),
        cache_dir: w.cache.clone(),
        dev_mode,
        platform: Platform::current(),
        exec: Some(exec),
        env,
        updated_from: None,
    };
    let downloader = Downloader::new(DownloaderConfig {
        retry_delay: std::time::Duration::from_millis(1),
        ..DownloaderConfig::default()
    })
    .unwrap();
    (UpdateService::new(cfg, feedback, rec.clone(), Arc::new(downloader)), rec)
}

fn kind(s: &UpdateStatus) -> &'static str {
    match s.state {
        UpdateState::Idle => "idle",
        UpdateState::Checking => "checking",
        UpdateState::UpToDate => "up_to_date",
        UpdateState::Available { .. } => "available",
        UpdateState::Downloading { .. } => "downloading",
        UpdateState::Ready { .. } => "ready",
        UpdateState::Failed { .. } => "failed",
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn finds_downloads_and_stages_an_update() {
    let w = world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::Normal).await;
    let (svc, rec) = service(&w, &srv.base_url, mock_github::REPO, false);
    let status = svc.check(false, false).await;
    match &status.state {
        UpdateState::Available { info } => assert_eq!(info.version, "0.2.0"),
        other => panic!("{other:?}"),
    }
    assert!(status.last_checked_ms.is_some());
    let status = svc.download_and_prepare().await;
    assert_eq!(kind(&status), "ready", "{status:?}");
    let trusted = sha256_file(&w.exe).unwrap();
    assert!(stage::validate_marker(&w.cache, Os::current(), Some(&trusted)).is_ok());
    let kinds: Vec<&str> = rec.updates.lock().unwrap().iter().map(kind).collect();
    assert_eq!(kinds, ["checking", "available", "downloading", "ready"]);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn development_mode_stops_after_the_verified_download() {
    let w = world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::Normal).await;
    let (svc, _) = service(&w, &srv.base_url, mock_github::REPO, true);
    assert!(!svc.status().apply_supported);
    svc.check(false, false).await;
    assert_eq!(kind(&svc.download_and_prepare().await), "ready");
    assert!(!stage::marker_path(&w.cache).exists());
    let asset = select::preferred_asset_name(&Platform::current());
    assert!(stage::download_path(&w.cache, "0.2.0", &asset).is_file());
    assert_eq!(svc.apply().unwrap_err().code, ErrorCode::Unsupported);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_check_without_updates_shows_a_toast() {
    let w = world();
    publish(&w, "0.1.0");
    let srv = serve(&w, Scenario::Normal).await;
    let (svc, rec) = service(&w, &srv.base_url, mock_github::REPO, false);
    assert_eq!(kind(&svc.check(false, true).await), "up_to_date");
    assert_eq!(rec.toasts.lock().unwrap()[0].title, Text::key("update_check_no_updates"));
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_check_is_reported_with_the_reason() {
    let w = world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::RateLimit).await;
    let (svc, rec) = service(&w, &srv.base_url, mock_github::REPO, false);
    match svc.check(false, true).await.state {
        UpdateState::Failed { error } => assert_eq!(error.code, ErrorCode::RateLimited),
        other => panic!("{other:?}"),
    }
    let toast = rec.toasts.lock().unwrap()[0].clone();
    assert_eq!(toast.title, Text::key("launcher_update_status_failed"));
    match toast.message {
        Some(Text::Key { key, params }) => {
            assert_eq!(key, "update_rate_limited");
            assert!(params.contains_key("minutes"));
        }
        other => panic!("{other:?}"),
    }
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_hash_download_fails_cleanly() {
    let w = world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::BadHash).await;
    let (svc, rec) = service(&w, &srv.base_url, mock_github::REPO, false);
    svc.check(false, false).await;
    match svc.download_and_prepare().await.state {
        UpdateState::Failed { error } => assert_eq!(error.code, ErrorCode::IntegrityMismatch),
        other => panic!("{other:?}"),
    }
    assert!(!stage::marker_path(&w.cache).exists());
    assert_eq!(rec.toasts.lock().unwrap()[0].title, Text::key("update_download_failed"));
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_repository_updates_are_off() {
    let w = world();
    let (svc, rec) = service(&w, "https://api.github.com", "", false);
    let status = svc.status();
    assert!(!status.configured && !status.apply_supported);
    assert_eq!(status.source, None);
    assert_eq!(kind(&svc.check(false, true).await), "idle");
    assert!(rec.updates.lock().unwrap().is_empty());
}

async fn ready_service(w: &World, srv: &MockHandle) -> (Arc<UpdateService>, Arc<Rec>) {
    let (svc, rec) = service(w, &srv.base_url, mock_github::REPO, false);
    svc.check(false, false).await;
    assert_eq!(kind(&svc.download_and_prepare().await), "ready");
    (svc, rec)
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_now_starts_a_single_helper() {
    let w = runnable_world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::Normal).await;
    let (svc, _) = ready_service(&w, &srv).await;
    svc.apply().unwrap();
    assert_eq!(svc.apply().unwrap_err().code, ErrorCode::Busy);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_refuses_a_payload_swapped_after_the_download() {
    let w = runnable_world();
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::Normal).await;
    let (svc, _) = ready_service(&w, &srv).await;
    let path = stage::marker_path(&w.cache);
    let mut marker: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let source = PathBuf::from(marker["source"].as_str().unwrap());
    std::fs::write(&source, b"something else").unwrap();
    marker["source_sha256"] = sha256_file(&source).unwrap().into();
    std::fs::write(&path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert_eq!(svc.apply().unwrap_err().code, ErrorCode::IntegrityMismatch);
    assert!(!path.exists(), "a tampered update is discarded");
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn install_folder_the_launcher_cannot_update_is_explained() {
    // Windows cannot update a program under Program Files; other systems, a folder they cannot write.
    let w = world_in(&[if cfg!(windows) { "Program Files" } else { "apps" }, "Launcher"]);
    #[cfg(unix)]
    let _locked = ReadOnly::new(install_folder(&w));
    publish(&w, "0.2.0");
    let srv = serve(&w, Scenario::Normal).await;
    let env = PathEnv { exe_dir: w.exe.parent().map(|p| p.to_path_buf()), ..PathEnv::default() };
    let (svc, rec) = service_with_env(&w, &srv.base_url, mock_github::REPO, false, env);
    svc.check(false, false).await;
    assert_eq!(kind(&svc.download_and_prepare().await), "failed");
    let toast = rec.toasts.lock().unwrap()[0].clone();
    assert_eq!(toast.title, Text::key("update_prepare_failed"));
    match toast.message {
        Some(Text::Key { key, params }) => {
            assert_eq!(key, "update_install_dir_readonly");
            assert!(params.contains_key("path"));
        }
        other => panic!("{other:?}"),
    }
    srv.shutdown().await;
}
