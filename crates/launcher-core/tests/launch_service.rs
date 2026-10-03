mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use launcher_core::auth::service::AuthService;
use launcher_core::auth::{AuthConfig, NoOpener};
use launcher_core::builds::service::BuildService;
use launcher_core::feedback::{EventSink, FeedbackService, OperationSpec};
use launcher_core::java::gpu::GpuPreferenceStore;
use launcher_core::java::memory::{GIB, MemoryLimits};
use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::launch::hooks::{
    GameWatch, GameWatcher, HookFuture, LaunchContext, LaunchHook, PrepareContext, WatchedGame,
};
use launcher_core::launch::ledger::{Entry, LEDGER_FILE, Ledger, process_started};
use launcher_core::launch::options::game_dir;
use launcher_core::launch::process::{LAUNCH_LOG, SystemSpawner};
use launcher_core::launch::service::{LaunchDeps, LaunchRequest, LaunchService, LaunchTimings};
use launcher_core::loaders::ComponentInstaller;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::VersionStore;
use launcher_shared::branding::MS_CLIENT_ID;
use launcher_shared::recent::Join;
use launcher_shared::{
    ActivityEntry, Alert, AppError, ErrorCode, GameEvent, GameState, LoaderKind, OpsSnapshot, Text, Toast,
};
use serde_json::{Value, json};
use support::fake_files::Served;
use support::fake_mojang::{FakeInstaller, FakeMojang, InstallerStyle};
use support::fake_processors::FakeRunner;

#[derive(Default)]
struct Recorder {
    games: Mutex<Vec<GameState>>,
    alerts: Mutex<Vec<Alert>>,
    toasts: Mutex<Vec<Toast>>,
}

impl EventSink for Recorder {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, toast: &Toast) {
        self.toasts.lock().unwrap().push(toast.clone());
    }
    fn alert(&self, alert: &Alert) {
        self.alerts.lock().unwrap().push(alert.clone());
    }
    fn game(&self, event: &GameEvent) {
        self.games.lock().unwrap().push(event.state.clone());
    }
}

#[derive(Default)]
struct GpuCalls(Mutex<Vec<(PathBuf, String)>>);

impl GpuPreferenceStore for GpuCalls {
    fn set(&self, exe: &Path, value: &str) -> std::io::Result<()> {
        self.0.lock().unwrap().push((exe.to_path_buf(), value.to_string()));
        Ok(())
    }
}

struct World {
    deps: LaunchDeps,
    ledger: PathBuf,
    _tmp: tempfile::TempDir,
    fake: FakeMojang,
    mc: PathBuf,
    auth: Arc<AuthService>,
    versions: Arc<VersionStore>,
    builds: BuildService,
    feedback: Arc<FeedbackService>,
    recorder: Arc<Recorder>,
    gpu: Arc<GpuCalls>,
    launcher: LaunchService,
    neo: FakeInstaller,
    runner: Arc<FakeRunner>,
}

fn fake_game() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-game"))
}

async fn world() -> World {
    world_with_hooks(|_| Vec::new()).await
}

/// A world whose launches run the hooks `hooks` makes (it sees the event recorder).
async fn world_with_hooks(hooks: impl FnOnce(Arc<Recorder>) -> Vec<Arc<dyn LaunchHook>>) -> World {
    world_with(hooks, Vec::new()).await
}

/// A world whose launches run `hooks` and whose games `watchers` watch.
async fn world_with(
    hooks: impl FnOnce(Arc<Recorder>) -> Vec<Arc<dyn LaunchHook>>,
    watchers: Vec<Arc<dyn GameWatcher>>,
) -> World {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let neo = fake.add_installer("neoforge", "1.21.1", "21.1.77", InstallerStyle::Processors);
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
    std::fs::create_dir_all(&state).unwrap();
    let recorder = Arc::new(Recorder::default());
    let sink: Arc<dyn EventSink> = recorder.clone();
    let feedback = FeedbackService::new(sink.clone());
    let config = Arc::new(ConfigStore::open(state.join("config.json")));
    let auth = AuthService::new(
        AuthConfig::new(MS_CLIENT_ID, &state, &tmp.path().join("cache")),
        feedback.clone(),
        sink.clone(),
        Arc::new(NoOpener),
    )
    .unwrap();
    let meta = Arc::new(MetaClient::new(Duration::from_secs(5), Duration::from_secs(3600)).unwrap());
    let downloader = Arc::new(
        Downloader::new(DownloaderConfig {
            retry_delay: Duration::from_millis(1),
            timeout: Duration::from_secs(5),
            ..DownloaderConfig::default()
        })
        .unwrap(),
    );
    let platform = GamePlatform::current();
    let java = Arc::new(JavaRuntimes::new(
        &mc,
        platform.clone(),
        fake.endpoints.clone(),
        meta.clone(),
        downloader.clone(),
    ));
    let installer = Arc::new(MinecraftInstaller::new(
        &mc,
        platform.clone(),
        fake.endpoints.clone(),
        meta.clone(),
        downloader.clone(),
        java,
        Arc::new(Coordinator::shared()),
    ));
    let runner = Arc::new(FakeRunner::default());
    let components = Arc::new(
        ComponentInstaller::new(installer, meta, fake.loader_endpoints(), &mc).with_runner(runner.clone()),
    );
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let builds = BuildService::new(
        versions.clone(),
        components.clone(),
        feedback.clone(),
        config.clone(),
        Arc::new(Coordinator::instances()),
    );
    let gpu = Arc::new(GpuCalls::default());
    let mc_dir = mc.clone();
    let ledger = state.join(LEDGER_FILE);
    let deps = LaunchDeps {
        mc_dir: mc,
        platform,
        versions: versions.clone(),
        components,
        auth: auth.clone(),
        feedback: feedback.clone(),
        config,
        instances: Arc::new(Coordinator::instances()),
        sink,
        spawner: Arc::new(SystemSpawner),
        gpu: gpu.clone(),
        hooks: hooks(recorder.clone()),
        downloader,
        watchers,
        timings: LaunchTimings {
            // Long enough that a slow machine still sees the second launch throttled.
            cooldown: Duration::from_secs(10),
            early_exit: Duration::from_millis(300),
            poll: Duration::from_millis(10),
        },
        ledger: Some(ledger.clone()),
        memory: Arc::new(|| MemoryLimits::from_bytes(Some(32 * GIB), Some(32 * GIB))),
    };
    let launcher = LaunchService::new(deps.clone());
    World {
        deps,
        ledger,
        _tmp: tmp,
        fake,
        mc: mc_dir,
        auth,
        versions,
        builds,
        feedback,
        recorder,
        gpu,
        launcher,
        neo,
        runner,
    }
}

impl World {
    /// The launcher as it starts again: nothing in memory, the same ledger file.
    fn restarted(&self) -> LaunchService {
        LaunchService::new(self.deps.clone())
    }

    /// The key the launch registry knows build `key`'s game folder by.
    fn restarted_key(&self, key: &str) -> String {
        let build = self.versions.get(key).unwrap();
        let dir = game_dir(&build, &self.mc);
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        let text = dir.to_string_lossy().into_owned();
        if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text }
    }

    /// A vanilla build whose "Java" is the fake game, started with `jvm` arguments.
    async fn build(&self, name: &str, jvm: &[&str]) -> String {
        let mut build = self.builds.install_vanilla(name, "1.21.1").await.unwrap();
        build.options.insert("executablePath".into(), json!(fake_game().to_string_lossy()));
        build.options.insert("jvmArguments".into(), json!(jvm));
        self.versions.save(&mut build).unwrap();
        build.key
    }

    fn wait_for(&self, done: impl Fn(&[GameState]) -> bool) -> Vec<GameState> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let games = self.recorder.games.lock().unwrap().clone();
            if done(&games) {
                return games;
            }
            assert!(Instant::now() < deadline, "no such game events: {games:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn request(key: &str) -> LaunchRequest {
    LaunchRequest { build_key: key.into(), ..LaunchRequest::default() }
}

fn finished(games: &[GameState]) -> bool {
    games.iter().any(|g| matches!(g, GameState::Exited { .. } | GameState::Crashed { .. }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_starts_with_its_command_and_reports_its_exit() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=1500"]).await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    assert!(started.pid > 0);
    assert_eq!(started.java, fake_game());
    let games = w.wait_for(finished);
    assert_eq!(
        games,
        [
            GameState::Started { pid: started.pid },
            GameState::Running { close_launcher: false },
            GameState::Exited { code: Some(0) }
        ]
    );
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(started.game_dir.join("fake-game.json")).unwrap())
            .unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    assert!(args[0].starts_with("-Xmx"), "{args:?}");
    assert!(args.contains(&"-Dfake.after=1500".to_string()));
    let main = args.iter().position(|a| a == "net.minecraft.client.main.Main").unwrap();
    let game = &args[main + 1..];
    assert!(game.windows(2).any(|pair| pair == ["--username", "Steve"]), "{game:?}");
    assert!(game.windows(2).any(|pair| pair == ["--accessToken", "offline"]), "{game:?}");
    assert!(game.windows(2).any(|pair| pair == ["--clientId", MS_CLIENT_ID]), "{game:?}");
    let classpath = &args[args.iter().position(|a| a == "-cp").unwrap() + 1];
    assert!(classpath.contains("core-1.0.jar") && classpath.contains("1.21.1.jar"), "{classpath}");
    let log = std::fs::read_to_string(started.game_dir.join("logs").join(LAUNCH_LOG)).unwrap();
    assert!(log.starts_with("Minecraft process diagnostics\nloader=1.21.1\n"), "{log}");
    assert!(log.contains("fake game started"), "{log}");
    let starting = Text::key("version_starting").param("version", "Aero");
    assert!(w.recorder.toasts.lock().unwrap().iter().any(|t| t.title == starting));
    let gpu = w.gpu.0.lock().unwrap().clone();
    if cfg!(windows) {
        assert_eq!(gpu, [(fake_game(), "GpuPreference=2;".to_string())]);
    } else {
        assert!(gpu.is_empty());
    }
    assert!(!w.launcher.is_running(&key));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_early_exit_is_a_crash_with_an_alert() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.exit=1"]).await;
    w.launcher.launch(request(&key)).await.unwrap();
    let games = w.wait_for(finished);
    match &games[..] {
        [GameState::Started { .. }, GameState::Crashed { code: Some(1), early: true, log: Some(log) }] => {
            assert!(log.ends_with(LAUNCH_LOG), "{log}")
        }
        other => panic!("{other:?}"),
    }
    let alerts = w.recorder.alerts.lock().unwrap().clone();
    assert_eq!(alerts.len(), 1);
    assert!(matches!(&alerts[0].message, Text::Key { key, .. } if key == "version_crashed_open_logs"));
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    assert!(!dir.join(".launcher").exists(), "nothing is kept beside the build");
}

/// Sees that a game cannot go on once its launch log says so; remembers the games it watched.
#[derive(Default)]
struct MarkerWatcher {
    watched: Mutex<Vec<WatchedGame>>,
}

struct MarkerWatch(PathBuf);

impl GameWatcher for MarkerWatcher {
    fn watch(&self, game: &WatchedGame) -> Box<dyn GameWatch> {
        self.watched.lock().unwrap().push(game.clone());
        Box::new(MarkerWatch(game.game_dir.join("logs").join(LAUNCH_LOG)))
    }
}

impl GameWatch for MarkerWatch {
    fn cannot_go_on(&mut self) -> bool {
        std::fs::read_to_string(&self.0).is_ok_and(|log| log.contains("this game cannot go on"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_that_sees_the_game_cannot_go_on_stops_it() {
    let watcher = Arc::new(MarkerWatcher::default());
    let w = world_with(|_| Vec::new(), vec![watcher.clone()]).await;
    w.auth.create_offline("Steve").unwrap();
    // Without the launcher it would wait a minute.
    let key = w.build("Aero", &["-Dfake.say=this game cannot go on", "-Dfake.after=60000"]).await;
    let started = Instant::now();
    w.launcher.launch(request(&key)).await.unwrap();
    let games = w.wait_for(finished);
    assert!(started.elapsed() < Duration::from_secs(10), "stopped, not waited out");
    assert!(matches!(games.last(), Some(GameState::Crashed { .. })), "{games:?}");
    assert_eq!(w.recorder.alerts.lock().unwrap().len(), 1, "no watch told of it: the plain alert");
    let watched = watcher.watched.lock().unwrap().clone();
    assert_eq!(watched.len(), 1);
    assert_eq!(
        (watched[0].build_key.as_str(), watched[0].build_name.as_str(), &watched[0].game_dir),
        (key.as_str(), "Aero", &game_dir(&w.versions.get(&key).unwrap(), &w.mc))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_build_is_not_started_twice() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=30000"]).await;
    w.launcher.launch(request(&key)).await.unwrap();
    assert!(w.launcher.is_running(&key));
    let err = w.launcher.launch(request(&key)).await.unwrap_err();
    assert_eq!((err.code, err.params["version"].as_str()), (ErrorCode::VersionRunning, "Aero"));
    let duplicate = LaunchRequest { allow_duplicate: true, ..request(&key) };
    let err = w.launcher.launch(duplicate).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::LaunchThrottled);
    assert!(err.params["seconds"].parse::<u64>().unwrap() >= 1, "{:?}", err.params);
    assert_eq!(w.launcher.terminate(&key), 1);
    let games = w.wait_for(finished);
    assert!(matches!(games.last(), Some(GameState::Exited { .. })), "{games:?}");
    assert!(!w.launcher.is_running(&key));
    assert!(w.recorder.alerts.lock().unwrap().is_empty(), "stopping is not a crash");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_game_that_may_not_fit_in_memory_asks_first() {
    // Two games of 16 GB at once on a 32 GB computer ran out of memory (crash reports of 03.10):
    // with a game running and less memory free than the next one asks, Play asks first.
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let first = w.build("First", &["-Xmx16G", "-Dfake.after=30000"]).await;
    let second = w.build("Second", &["-Xmx16G", "-Dfake.after=30000"]).await;
    let tight = LaunchService::new(LaunchDeps {
        memory: Arc::new(|| MemoryLimits::from_bytes(Some(32 * GIB), Some(10 * GIB))),
        ..w.deps.clone()
    });
    tight.launch(request(&first)).await.unwrap();
    let err = tight.launch(request(&second)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::LowMemory);
    assert_eq!(
        (err.params["version"].as_str(), err.params["heap"].as_str(), err.params["available"].as_str()),
        ("Second", "16", "10")
    );
    tight.launch(LaunchRequest { allow_low_memory: true, ..request(&second) }).await.unwrap();
    assert_eq!(tight.terminate(&first) + tight.terminate(&second), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn problems_before_the_start_free_the_slot() {
    let w = world().await;
    let key = w.build("Aero", &[]).await;
    assert_eq!(w.launcher.launch(request(&key)).await.unwrap_err().code, ErrorCode::NoProfile);
    w.auth.create_offline("Steve").unwrap();
    w.launcher.launch(request(&key)).await.unwrap();
    assert_eq!(w.launcher.launch(request("nobody")).await.unwrap_err().code, ErrorCode::VersionNotFound);
    w.wait_for(finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_version_fails_as_an_integrity_problem() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &[]).await;
    let mut build = w.versions.get(&key).unwrap();
    build.loader = Some("9.9.9".into());
    build.version = Some("9.9.9".into());
    w.versions.save(&mut build).unwrap();
    let err = w.launcher.launch(request(&key)).await.unwrap_err();
    assert_eq!((err.code, err.params["version"].as_str()), (ErrorCode::LaunchFailed, "Aero"));
    assert!(w.recorder.games.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_launcher_refuses_to_launch() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &[]).await;
    let running = w.feedback.begin(OperationSpec::new(Text::key("installation_started"), "install"));
    assert_eq!(w.launcher.launch(request(&key)).await.unwrap_err().code, ErrorCode::Busy);
    running.finish();
    w.launcher.launch(request(&key)).await.unwrap();
    w.wait_for(finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_failure_keeps_its_own_message() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &[]).await;
    std::fs::remove_file(w.mc.join("libraries/com/example/core/1.0/core-1.0.jar")).unwrap();
    w.fake.server.put("libs/core.jar", Served { status: Some(503), ..Served::default() });
    let err = w.launcher.launch(request(&key)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed, "not 'reinstall the build': {}", err.detail);
    assert!(w.recorder.games.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fabric_build_starts_its_loader_and_recovers_its_profile() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let mut build =
        w.builds.install_loader("Fabric Aero", LoaderKind::Fabric, "1.21.1", "0.16.9").await.unwrap();
    build.options.insert("executablePath".into(), json!(fake_game().to_string_lossy()));
    build.options.insert("jvmArguments".into(), json!(["-Dfake.after=1500"]));
    w.versions.save(&mut build).unwrap();
    let id = "fabric-loader-0.16.9-1.21.1";
    std::fs::remove_file(w.mc.join("versions").join(id).join(format!("{id}.json"))).unwrap();
    let started = w.launcher.launch(request(&build.key)).await.unwrap();
    w.wait_for(finished);
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(started.game_dir.join("fake-game.json")).unwrap())
            .unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    assert!(args.iter().any(|a| a == "net.fabricmc.loader.impl.launch.knot.KnotClient"), "{args:?}");
    let classpath = &args[args.iter().position(|a| a == "-cp").unwrap() + 1];
    assert!(
        classpath.contains("fabric-loader-0.16.9.jar") && classpath.contains("1.21.1.jar"),
        "{classpath}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_neoforge_build_starts_bootstrap_and_gets_its_processors_back() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let mut build = w.builds.install_loader("Neo", LoaderKind::NeoForge, "1.21.1", "21.1.77").await.unwrap();
    build.options.insert("executablePath".into(), json!(fake_game().to_string_lossy()));
    build.options.insert("jvmArguments".into(), json!(["-Dfake.after=1500"]));
    w.versions.save(&mut build).unwrap();
    std::fs::remove_file(w.mc.join("libraries").join(&w.neo.patched_path)).unwrap();
    let started = w.launcher.launch(request(&build.key)).await.unwrap();
    w.wait_for(finished);
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(started.game_dir.join("fake-game.json")).unwrap())
            .unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    assert!(args.iter().any(|a| a == "cpw.mods.bootstraplauncher.BootstrapLauncher"), "{args:?}");
    let module_path = &args[args.iter().position(|a| a == "-p").unwrap() + 1];
    assert_eq!(Path::new(module_path), w.mc.join("libraries").join("net/fake/boot/1.0/boot-1.0.jar"));
    assert!(
        w.mc.join("libraries").join(&w.neo.patched_path).is_file(),
        "the processors ran again before the launch"
    );
    assert_eq!(w.runner.count(), 2);
}

/// A mod update that stopped half way in the build's folder: `mods/a.jar` replaced (the old copy
/// in the backup), `mods/b.jar` added.
fn interrupt_a_change(dir: &std::path::Path, backup_there: bool) {
    let id = "0123456789abcdef0123456789abcdef";
    let backup = dir.join(".launcher-sync").join(id).join("backup").join("mods");
    std::fs::create_dir_all(&backup).unwrap();
    if backup_there {
        std::fs::write(backup.join("a.jar"), b"old").unwrap();
    }
    std::fs::create_dir_all(dir.join("mods")).unwrap();
    std::fs::write(dir.join("mods").join("a.jar"), b"new").unwrap();
    std::fs::write(dir.join("mods").join("b.jar"), b"added").unwrap();
    let journal = json!({"schema_version": 2, "status": "applying", "operation": "modrinth-content-install",
        "commit_key": null, "transaction_id": id, "entries": [
            {"path": "mods/a.jar", "kind": "replace", "state": "applied", "had_original": true},
            {"path": "mods/b.jar", "kind": "replace", "state": "applied", "had_original": false}]});
    std::fs::write(dir.join(".launcher-modrinth-sync.json"), journal.to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_file_change_is_undone_before_the_start() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    interrupt_a_change(&dir, true);
    w.launcher.launch(request(&key)).await.unwrap();
    w.wait_for(finished);
    assert_eq!(std::fs::read(dir.join("mods").join("a.jar")).unwrap(), b"old");
    assert!(!dir.join("mods").join("b.jar").exists());
    let journal: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".launcher-modrinth-sync.json")).unwrap())
            .unwrap();
    assert_eq!(journal["status"], "rolled_back");
    assert!(!dir.join(".launcher-sync").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undo_that_fails_warns_but_starts() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    interrupt_a_change(&dir, false);
    let started = w.launcher.launch(request(&key)).await.unwrap();
    assert!(started.pid > 0);
    w.wait_for(finished);
    let warned = w.recorder.alerts.lock().unwrap().iter().any(|a| match &a.message {
        Text::Key { key, params } => {
            key == "content_recovery_failed" && params.get("version").map(String::as_str) == Some("Aero")
        }
        _ => false,
    });
    assert!(warned);
}

/// A hook that notes each launch it steps into and how many game events came before it.
struct Probe {
    seen: Arc<Mutex<Vec<String>>>,
    recorder: Arc<Recorder>,
    fail: bool,
}

impl LaunchHook for Probe {
    fn before_launch<'a>(&'a self, ctx: &'a LaunchContext<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let games = self.recorder.games.lock().unwrap().len();
            let there = ctx.game_dir.is_dir();
            self.seen.lock().unwrap().push(format!("{} games {games} folder {there}", ctx.build.name));
            if self.fail { Err(AppError::new(ErrorCode::Io, "hook broke")) } else { Ok(()) }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_runs_after_the_files_are_checked_and_before_the_game_starts() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let probe = seen.clone();
    let w = world_with_hooks(move |recorder| {
        vec![Arc::new(Probe { seen: probe, recorder, fail: false }) as Arc<dyn LaunchHook>]
    })
    .await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    assert!(started.pid > 0);
    w.wait_for(finished);
    assert_eq!(seen.lock().unwrap().as_slice(), ["Aero games 0 folder true".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_hook_only_warns() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let probe = seen.clone();
    let w = world_with_hooks(move |recorder| {
        vec![Arc::new(Probe { seen: probe, recorder, fail: true }) as Arc<dyn LaunchHook>]
    })
    .await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    assert!(w.launcher.launch(request(&key)).await.unwrap().pid > 0);
    w.wait_for(finished);
    let warned = w.recorder.alerts.lock().unwrap().iter().any(|a| match &a.message {
        Text::Key { key, params } => {
            key == "launch_hook_failed" && params.get("error").is_some_and(|e| e.contains("hook broke"))
        }
        _ => false,
    });
    assert!(warned, "{:?}", w.recorder.alerts.lock().unwrap());
}

/// Before the files are checked, adds a Java argument to the build and saves it (as a server
/// sync changes a build), or refuses the launch.
struct Preparing {
    refuse: bool,
}

impl LaunchHook for Preparing {
    fn before_launch<'a>(&'a self, _: &'a LaunchContext<'a>) -> HookFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_build<'a>(&'a self, ctx: &'a PrepareContext<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            if self.refuse {
                return Err(AppError::new(ErrorCode::Network, "the server is gone"));
            }
            let mut build = ctx.build.clone();
            let mut arguments = build.options.get("jvmArguments").cloned().unwrap_or(json!([]));
            arguments.as_array_mut().unwrap().push(json!("-Dfake.prepared=1"));
            build.options.insert("jvmArguments".into(), arguments);
            ctx.versions.save(&mut build)
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_step_s_saved_change_is_what_launches() {
    let w = world_with_hooks(|_| vec![Arc::new(Preparing { refuse: false }) as Arc<dyn LaunchHook>]).await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    w.wait_for(finished);
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(started.game_dir.join("fake-game.json")).unwrap())
            .unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    assert!(args.contains(&"-Dfake.prepared=1".to_string()), "{args:?}");
    assert!(args.contains(&"-Dfake.after=200".to_string()), "{args:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_step_that_fails_stops_the_launch() {
    let w = world_with_hooks(|_| vec![Arc::new(Preparing { refuse: true }) as Arc<dyn LaunchHook>]).await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=200"]).await;
    let error = w.launcher.launch(request(&key)).await.unwrap_err();
    assert_eq!((error.code, error.detail.as_str()), (ErrorCode::Network, "the server is gone"));
    assert!(w.recorder.games.lock().unwrap().is_empty(), "no game started");
    assert!(!w.launcher.is_running(&key));
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    assert!(!dir.join("fake-game.json").exists());
}

/// Waits until `done` holds (the launcher's threads run on their own).
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_game_started_before_a_restart_is_still_running() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=30000"]).await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    let noted = Ledger::new(w.ledger.clone()).entries();
    assert_eq!(
        noted.iter().map(|e| (e.pid, e.build_key.as_str())).collect::<Vec<_>>(),
        [(started.pid, "aero")]
    );
    let again = w.restarted();
    assert!(!again.is_running(&key), "a new launcher knows nothing yet");
    assert_eq!(again.adopt(), 1);
    assert!(again.is_running(&key), "the game from before the restart runs");
    assert_eq!(again.terminate(&key), 1);
    eventually("the game stops", || process_started(started.pid).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_game_left_by_a_restart_can_be_stopped() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let key = w.build("Aero", &["-Dfake.after=30000"]).await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    let again = w.restarted();
    again.adopt();
    assert_eq!(again.terminate(&key), 1, "«Stop» reaches it");
    eventually("the game stops", || process_started(started.pid).is_none());
    eventually("the restarted launcher sees it gone", || !again.is_running(&key));
    eventually("the ledger forgets it", || Ledger::new(w.ledger.clone()).entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_record_whose_process_is_gone_is_dropped() {
    let w = world().await;
    let gone = Entry {
        key: "k".into(),
        build_key: "aero".into(),
        build_name: "Aero".into(),
        pid: u32::MAX - 7,
        started: "1".into(),
    };
    Ledger::new(w.ledger.clone()).add(gone).unwrap();
    assert_eq!(w.restarted().adopt(), 0);
    assert!(Ledger::new(w.ledger.clone()).entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reused_pid_is_not_the_game() {
    let w = world().await;
    // This process runs, but it is not the game that started with that id long ago.
    let reused = Entry {
        key: "k".into(),
        build_key: "aero".into(),
        build_name: "Aero".into(),
        pid: std::process::id(),
        started: "another time".into(),
    };
    Ledger::new(w.ledger.clone()).add(reused).unwrap();
    let again = w.restarted();
    assert_eq!(again.adopt(), 0);
    assert!(Ledger::new(w.ledger.clone()).entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn both_copies_of_a_build_are_taken_back_after_a_restart() {
    let w = world().await;
    let key = w.build("Aero", &[]).await;
    let dir = w._tmp.path().join("games-of-aero");
    std::fs::create_dir_all(&dir).unwrap();
    // Two games of one build (the player confirmed the second), started before the restart.
    let mut games: Vec<std::process::Child> = (0..2)
        .map(|_| {
            std::process::Command::new(fake_game())
                .arg("-Dfake.after=30000")
                .current_dir(&dir)
                .spawn()
                .unwrap()
        })
        .collect();
    let ledger = Ledger::new(w.ledger.clone());
    let registry_key = w.restarted_key(&key);
    for game in &games {
        let started = process_started(game.id()).unwrap();
        let entry = Entry {
            key: registry_key.clone(),
            build_key: key.clone(),
            build_name: "Aero".into(),
            pid: game.id(),
            started,
        };
        ledger.add(entry).unwrap();
    }
    let again = w.restarted();
    assert_eq!(again.adopt(), 2, "each copy is taken back");
    assert_eq!(again.terminate(&key), 2, "«Stop» reaches both");
    for game in &mut games {
        let _ = game.wait();
    }
    eventually("both are gone from the restarted launcher", || !again.is_running(&key));
}

/// A game the launcher lost track of (its ledger forgot it, or another launcher started it) that
/// still writes the build's log: the build shows running, and «Stop» reaches it.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_game_the_launcher_lost_shows_and_stops() {
    let w = world().await;
    let key = w.build("Aero", &[]).await;
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    std::fs::create_dir_all(&dir).unwrap();
    // Named as a Java runtime is: only Java is taken for a game.
    let java = w._tmp.path().join("runtime/bin/java.exe");
    std::fs::create_dir_all(java.parent().unwrap()).unwrap();
    std::fs::copy(fake_game(), &java).unwrap();
    let mut game = std::process::Command::new(&java)
        .args(["-Dfake.log", "-Dfake.after=30000"])
        .current_dir(&dir)
        .spawn()
        .unwrap();
    eventually("the game writes its log", || dir.join("logs/latest.log").exists());
    assert!(w.launcher.is_running(&key), "the build shows running");
    assert_eq!(w.launcher.terminate(&key), 1, "«Stop» reaches it");
    let _ = game.wait();
    eventually("the build is free", || !w.launcher.is_running(&key));
}

/// Something else that holds the log (not Java) is never stopped.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn only_a_java_game_is_stopped() {
    let w = world().await;
    let key = w.build("Aero", &[]).await;
    let dir = game_dir(&w.versions.get(&key).unwrap(), &w.mc);
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    let held = std::fs::File::create(dir.join("logs/latest.log")).unwrap();
    assert!(w.launcher.is_running(&key));
    assert_eq!(w.launcher.terminate(&key), 0, "this test is not a game");
    drop(held);
    assert!(!w.launcher.is_running(&key));
}

#[test]
fn a_later_game_has_a_later_start() {
    let dir = tempfile::tempdir().unwrap();
    let spawn = || {
        std::process::Command::new(fake_game())
            .arg("-Dfake.after=4000")
            .current_dir(dir.path())
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    std::thread::sleep(Duration::from_millis(1100));
    let mut second = spawn();
    let (a, b) = (process_started(first.id()), process_started(second.id()));
    let _ = (first.kill(), second.kill(), first.wait(), second.wait());
    assert!(a.is_some() && b.is_some());
    assert_ne!(a, b, "a start time tells two processes apart (not a state letter)");
}

/// The `--username` a build's game started with (the fake game records its arguments).
fn username(game_dir: &Path) -> String {
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(game_dir.join("fake-game.json")).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    let at = args.iter().position(|a| a == "--username").unwrap();
    args[at + 1].clone()
}

/// A build of Steve's (the default) and Alex's accounts whose own account is `account`.
async fn build_of(w: &World, account: &str) -> String {
    w.auth.create_offline("Steve").unwrap();
    w.auth.create_offline("Alex").unwrap();
    w.auth.set_default("Steve").unwrap();
    let key = w.build("Aero", &[]).await;
    let mut build = w.versions.get(&key).unwrap();
    build.options.insert(launcher_core::launch::options::PROFILE_OPTION.into(), json!(account));
    w.versions.save(&mut build).unwrap();
    key
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_with_its_own_account_starts_with_it() {
    let w = world().await;
    let key = build_of(&w, "Alex").await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    w.wait_for(finished);
    assert_eq!(username(&started.game_dir), "Alex", "not the default account");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_picked_at_play_wins_over_the_build_s_own() {
    let w = world().await;
    let key = build_of(&w, "Alex").await;
    let started = w
        .launcher
        .launch(LaunchRequest { profile_key: Some("Steve".into()), ..request(&key) })
        .await
        .unwrap();
    w.wait_for(finished);
    assert_eq!(username(&started.game_dir), "Steve");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_whose_account_is_gone_starts_with_the_default() {
    let w = world().await;
    let key = build_of(&w, "Gone").await;
    let started = w.launcher.launch(request(&key)).await.unwrap();
    w.wait_for(finished);
    assert_eq!(username(&started.game_dir), "Steve");
}

/// The game arguments (after the main class) a build's game started with.
fn game_args(game_dir: &Path) -> Vec<String> {
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(game_dir.join("fake-game.json")).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(record["args"].clone()).unwrap();
    let main = args.iter().position(|a| a == "net.minecraft.client.main.Main").unwrap();
    args[main + 1..].to_vec()
}

#[tokio::test(flavor = "multi_thread")]
async fn play_goes_straight_into_the_server_or_world_asked() {
    let w = world().await;
    w.auth.create_offline("Steve").unwrap();
    let launches = [
        (
            Join::Server { host: "other.example".into(), port: 25566 },
            ["--quickPlayMultiplayer", "other.example:25566"],
        ),
        (Join::World { folder: "Мій світ".into() }, ["--quickPlaySingleplayer", "Мій світ"]),
        // A world that is not there (or not a folder name) starts the build as it is.
        (Join::World { folder: "Gone".into() }, ["--quickPlayMultiplayer", "own.example:25570"]),
        (Join::World { folder: "../Мій світ".into() }, ["--quickPlayMultiplayer", "own.example:25570"]),
    ];
    for (round, (join, expected)) in launches.into_iter().enumerate() {
        // A build each: one build is not started twice within moments.
        let key = w.build(&format!("Aero {round}"), &[]).await;
        let mut build = w.versions.get(&key).unwrap();
        build.options.insert("server".into(), json!("own.example:25570"));
        w.versions.save(&mut build).unwrap();
        let game = game_dir(&build, &w.mc);
        std::fs::create_dir_all(game.join("saves").join("Мій світ")).unwrap();
        std::fs::create_dir_all(game.join("Мій світ")).unwrap();
        w.launcher.launch(LaunchRequest { join: Some(join.clone()), ..request(&key) }).await.unwrap();
        w.wait_for(|games| {
            games.iter().filter(|g| matches!(g, GameState::Exited { .. } | GameState::Crashed { .. })).count()
                > round
        });
        let args = game_args(&game);
        assert!(args.ends_with(&expected.map(String::from)), "{join:?}: {args:?}");
        let quick = args.iter().filter(|a| a.starts_with("--quickPlay") || *a == "--server").count();
        assert_eq!(quick, 1, "one place to go: {args:?}");
    }
}
