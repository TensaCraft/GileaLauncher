//! Manual check with the real Minecraft (tests never run it):
//! `cargo run -p launcher-core --bin launch -- <minecraft dir> <state dir> <version> <player> [--seconds N]`.
//! Installs a vanilla build named after the version when there is none, launches it with an
//! offline profile, lets it run for N seconds (40 by default), stops it and checks that the game
//! logged the player in. Writes no GPU preferences.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::auth::service::AuthService;
use launcher_core::auth::{AuthConfig, NoOpener};
use launcher_core::builds::service::BuildService;
use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_core::java::gpu::NoGpuPreferences;
use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::launch::options::game_dir;
use launcher_core::launch::process::SystemSpawner;
use launcher_core::launch::service::{LaunchDeps, LaunchRequest, LaunchService, LaunchTimings};
use launcher_core::loaders::ComponentInstaller;
use launcher_core::loaders::LoaderEndpoints;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::manifest::MojangEndpoints;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::{META_TTL, MetaClient};
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::VersionStore;
use launcher_shared::branding::MS_CLIENT_ID;
use launcher_shared::{ActivityEntry, Alert, GameEvent, OpsSnapshot, Toast};

struct Print;

impl EventSink for Print {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, toast: &Toast) {
        println!("toast: {:?}", toast.title);
    }
    fn alert(&self, alert: &Alert) {
        println!("alert: {:?}", alert.message);
    }
    fn game(&self, event: &GameEvent) {
        println!("game: {:?}", event.state);
    }
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("failed: {message}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(mc), Some(state), Some(version), Some(player)) =
        (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        eprintln!("usage: launch <minecraft dir> <state dir> <version> <player> [--seconds N]");
        std::process::exit(2);
    };
    let seconds = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let (mc, state) = (PathBuf::from(mc), PathBuf::from(state));
    std::fs::create_dir_all(&state).unwrap_or_else(|e| fail(e));
    let sink: Arc<dyn EventSink> = Arc::new(Print);
    let feedback = FeedbackService::new(sink.clone());
    let config = Arc::new(ConfigStore::open(state.join("config.json")));
    let auth = AuthService::new(
        AuthConfig::new(MS_CLIENT_ID, &state, &state.join("cache")),
        feedback.clone(),
        sink.clone(),
        Arc::new(NoOpener),
    )
    .unwrap_or_else(|e| fail(e));
    if !auth.snapshot().profiles.iter().any(|p| p.key == *player) {
        auth.create_offline(player).unwrap_or_else(|e| fail(e));
    }
    let platform = GamePlatform::current();
    let endpoints = MojangEndpoints::default();
    let meta = Arc::new(MetaClient::new(Duration::from_secs(30), META_TTL).unwrap_or_else(|e| fail(e)));
    let files = Arc::new(Downloader::new(DownloaderConfig::default()).unwrap_or_else(|e| fail(e)));
    let java =
        Arc::new(JavaRuntimes::new(&mc, platform.clone(), endpoints.clone(), meta.clone(), files.clone()));
    let installer = Arc::new(MinecraftInstaller::new(
        &mc,
        platform.clone(),
        endpoints,
        meta.clone(),
        files.clone(),
        java,
        Arc::new(Coordinator::shared()),
    ));
    let components = Arc::new(ComponentInstaller::new(installer, meta, LoaderEndpoints::default(), &mc));
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let instances = Arc::new(Coordinator::instances());
    let builds = BuildService::new(
        versions.clone(),
        components.clone(),
        feedback.clone(),
        config.clone(),
        instances.clone(),
    );
    let build = match versions.get_by_name(version) {
        Some(build) => build,
        None => builds.install_vanilla(version, version).await.unwrap_or_else(|e| fail(e)),
    };
    let launcher = LaunchService::new(LaunchDeps {
        mc_dir: mc.clone(),
        platform,
        versions: versions.clone(),
        components,
        auth,
        feedback,
        config,
        instances,
        sink,
        spawner: Arc::new(SystemSpawner),
        gpu: Arc::new(NoGpuPreferences),
        hooks: Vec::new(),
        downloader: files.clone(),
        watchers: Vec::new(),
        timings: LaunchTimings::default(),
        ledger: None,
    });
    let request = LaunchRequest {
        build_key: build.key.clone(),
        profile_key: Some(player.clone()),
        allow_duplicate: false,
    };
    let started = launcher.launch(request).await.unwrap_or_else(|e| fail(e));
    println!("started pid {} with {}", started.pid, started.java.display());
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    println!("stopping: {} game(s)", launcher.terminate(&build.key));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let latest = game_dir(&build, &mc).join("logs").join("latest.log");
    let log = std::fs::read_to_string(&latest).unwrap_or_default();
    let needle = format!("Setting user: {player}");
    println!(
        "{}: {}",
        latest.display(),
        if log.contains(&needle) { "the player is logged in" } else { "no login line" }
    );
    if !log.contains(&needle) {
        std::process::exit(1);
    }
}
