mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::builds::service::{BuildService, GPU_MODE_DEFAULT_KEY};
use launcher_core::feedback::{FeedbackService, NullSink, OperationSpec};
use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::loaders::ComponentInstaller;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{BuildSettingsUpdate, ErrorCode, LoaderKind, Text};
use serde_json::json;
use support::fake_mojang::{FakeMojang, InstallerStyle};
use support::fake_processors::FakeRunner;

struct World {
    _tmp: tempfile::TempDir,
    games: PathBuf,
    versions: Arc<VersionStore>,
    feedback: Arc<FeedbackService>,
    config: Arc<ConfigStore>,
    instances: Arc<Coordinator>,
    service: BuildService,
}

fn world(fake: &FakeMojang) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
    std::fs::create_dir_all(&state).unwrap();
    let games = mc.join("games");
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
        platform,
        fake.endpoints.clone(),
        meta.clone(),
        downloader,
        java,
        Arc::new(Coordinator::shared()),
    ));
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let feedback = FeedbackService::new(Arc::new(NullSink));
    let config = Arc::new(ConfigStore::open(state.join("config.json")));
    let instances = Arc::new(Coordinator::instances());
    let components = Arc::new(
        ComponentInstaller::new(installer, meta, fake.loader_endpoints(), &mc)
            .with_runner(Arc::new(FakeRunner::default())),
    );
    let service =
        BuildService::new(versions.clone(), components, feedback.clone(), config.clone(), instances.clone());
    World { _tmp: tmp, games, versions, feedback, config, instances, service }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vanilla_build_is_installed_and_registered() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.config.set(GPU_MODE_DEFAULT_KEY, json!("igpu")).unwrap();
    let build = w.service.install_vanilla("  Моя збірка ", "1.21.1").await.unwrap();
    assert_eq!((build.key.as_str(), build.name.as_str()), ("moja_zbirka", "Моя збірка"));
    assert_eq!(
        (build.version.as_deref(), build.loader.as_deref(), build.client.as_deref()),
        (Some("1.21.1"), Some("1.21.1"), Some("Minecraft"))
    );
    let game_dir = w.games.join("moja_zbirka");
    assert!(game_dir.is_dir());
    assert_eq!(build.path.as_deref().map(PathBuf::from), Some(game_dir));
    assert_eq!(build.options["gpuMode"], json!("igpu"), "the configured default GPU mode");
    if GamePlatform::current().java_runtime_key().is_some() {
        let java = build.options["executablePath"].as_str().unwrap();
        assert!(PathBuf::from(java).is_file(), "{java}");
    }
    assert_eq!(w.versions.get("moja_zbirka"), Some(build));
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_install_registers_nothing() {
    let fake = FakeMojang::start().await;
    let w = world(&fake);
    let err = w.service.install_vanilla("Ghost", "9.9.9").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionNotFound);
    assert!(w.versions.list().is_empty());
    assert!(!w.games.join("ghost").exists());
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn names_are_checked_before_any_download() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.versions.create(&mut Build::new("Aero")).unwrap();
    assert_eq!(
        w.service.install_vanilla(" aero ", "1.21.1").await.unwrap_err().code,
        ErrorCode::VersionExists
    );
    assert_eq!(
        w.service.install_vanilla("   ", "1.21.1").await.unwrap_err().code,
        ErrorCode::VersionNameEmpty
    );
    assert_eq!(fake.server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_launcher_refuses_a_second_install() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let running = w.feedback.begin(OperationSpec::new(Text::key("installation_started"), "install"));
    assert_eq!(w.service.install_vanilla("Second", "1.21.1").await.unwrap_err().code, ErrorCode::Busy);
    running.finish();
    assert_eq!(fake.server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_build_skips_a_leftover_folder() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let leftover = w.games.join("moja_zbirka");
    std::fs::create_dir_all(leftover.join("saves")).unwrap();
    std::fs::write(leftover.join("saves").join("keep.txt"), b"old world").unwrap();
    let build = w.service.install_vanilla("Моя збірка", "1.21.1").await.unwrap();
    assert_eq!(build.key, "moja_zbirka_2");
    assert_eq!(std::fs::read(leftover.join("saves").join("keep.txt")).unwrap(), b"old world");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_lists_builds_by_name_with_their_state() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("zeta", "1.21.1").await.unwrap();
    w.service.install_vanilla("Alpha", "1.21.1").await.unwrap();
    let snapshot = w.service.snapshot(|build| build.key == "zeta");
    let names: Vec<(&str, bool)> = snapshot.builds.iter().map(|b| (b.name.as_str(), b.running)).collect();
    assert_eq!(names, [("Alpha", false), ("zeta", true)]);
    let alpha = &snapshot.builds[0];
    assert_eq!((alpha.version.as_deref(), alpha.client.as_deref()), (Some("1.21.1"), Some("Minecraft")));
    assert_eq!(PathBuf::from(&alpha.game_dir), w.games.join("alpha"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_gets_its_own_folder_and_settings() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let mut source = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    source.options.insert("jvmArguments".into(), json!("-Xmx6G"));
    source.description = "Мій світ".into();
    w.versions.save(&mut source).unwrap();
    let from = w.games.join("aero");
    std::fs::create_dir_all(from.join("saves").join("World")).unwrap();
    std::fs::write(from.join("saves").join("World").join("level.dat"), b"level").unwrap();
    std::fs::write(from.join("options.txt"), b"fov:90").unwrap();

    let copy = w.service.copy("aero", "  Aero copy ", || false).await.unwrap();
    assert_eq!((copy.key.as_str(), copy.name.as_str()), ("aero_copy", "Aero copy"));
    let to = w.games.join("aero_copy");
    assert_eq!(copy.path.as_deref().map(PathBuf::from), Some(to.clone()));
    assert_eq!(std::fs::read(to.join("saves").join("World").join("level.dat")).unwrap(), b"level");
    assert_eq!(std::fs::read(to.join("options.txt")).unwrap(), b"fov:90");
    assert_eq!(
        (copy.version.as_deref(), copy.loader.as_deref(), copy.client.as_deref()),
        (Some("1.21.1"), Some("1.21.1"), Some("Minecraft"))
    );
    assert_eq!(copy.options, source.options);
    assert_eq!(copy.description, "Мій світ");
    assert_eq!(w.versions.get("aero_copy"), Some(copy));
    assert_eq!(std::fs::read(from.join("options.txt")).unwrap(), b"fov:90", "the source is untouched");
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_a_managed_build_is_the_players_own() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let mut source = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    source.client = Some("TensaCraft".into());
    source.force_update = true;
    source.options.insert("managedByApi".into(), json!(true));
    source.options.insert("tensacraftPackId".into(), json!("aero"));
    w.versions.save(&mut source).unwrap();
    let copy = w.service.copy("aero", "Aero copy", || false).await.unwrap();
    assert_eq!(copy.options.get("managedByApi"), Some(&json!(false)));
    assert_eq!(copy.options.get("syncMode"), Some(&json!("manual")));
    assert!(!copy.force_update);
    assert_eq!(copy.client.as_deref(), Some("Minecraft"), "it names what it runs");
    let source = w.versions.get("aero").unwrap();
    assert_eq!(source.options.get("managedByApi"), Some(&json!(true)), "the original stays managed");
}

#[tokio::test(flavor = "multi_thread")]
async fn copying_never_touches_an_existing_folder() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    let leftover = w.games.join("aero_copy");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("keep.txt"), b"someone else's").unwrap();
    let copy = w.service.copy("aero", "Aero copy", || false).await.unwrap();
    assert_eq!(copy.key, "aero_copy_2");
    assert!(w.games.join("aero_copy_2").is_dir());
    assert_eq!(std::fs::read(leftover.join("keep.txt")).unwrap(), b"someone else's");
    assert_eq!(std::fs::read_dir(&leftover).unwrap().count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_is_refused_for_taken_names_busy_folders_and_unknown_builds() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    assert_eq!(w.service.copy("aero", " AERO ", || false).await.unwrap_err().code, ErrorCode::VersionExists);
    assert_eq!(w.service.copy("aero", "  ", || false).await.unwrap_err().code, ErrorCode::VersionNameEmpty);
    assert_eq!(
        w.service.copy("ghost", "Ghost copy", || false).await.unwrap_err().code,
        ErrorCode::VersionNotFound
    );
    let lease = w.instances.try_acquire(&w.games.join("aero"), "launch").unwrap();
    assert_eq!(
        w.service.copy("aero", "Aero copy", || false).await.unwrap_err().code,
        ErrorCode::InstanceBusy
    );
    drop(lease);
    assert_eq!(w.versions.list().len(), 1);
    assert!(!w.games.join("aero_copy").exists());
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_respects_running_games_and_locks() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    w.service.install_vanilla("Beta", "1.21.1").await.unwrap();
    let folder = w.games.join("aero");
    std::fs::write(folder.join("options.txt"), b"fov:90").unwrap();

    assert_eq!(w.service.delete("aero", true, || true).unwrap_err().code, ErrorCode::BuildRunning);
    assert!(w.versions.get("aero").is_some() && folder.join("options.txt").is_file());

    let lease = w.instances.try_acquire(&folder, "launch").unwrap();
    assert_eq!(w.service.delete("aero", true, || false).unwrap_err().code, ErrorCode::InstanceBusy);
    drop(lease);
    w.service.delete("aero", true, || false).unwrap();
    assert!(!folder.exists());
    let left: Vec<String> = w.versions.list().into_iter().map(|b| b.key).collect();
    assert_eq!(left, ["beta"], "only the build's own folder goes");
    assert!(w.games.join("beta").join("version.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_build_can_be_forgotten_without_its_files() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    w.service.delete("aero", false, || true).unwrap();
    assert!(w.versions.get("aero").is_none());
    assert!(w.games.join("aero").is_dir());
    assert!(!w.games.join("aero").join("version.json").exists(), "only the record goes");
    assert_eq!(w.service.delete("aero", true, || false).unwrap_err().code, ErrorCode::VersionNotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fabric_build_is_installed_and_registered() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let w = world(&fake);
    let build =
        w.service.install_loader(" Fabric 1.21.1 ", LoaderKind::Fabric, "1.21.1", "0.16.9").await.unwrap();
    assert_eq!(
        (
            build.name.as_str(),
            build.version.as_deref(),
            build.client.as_deref(),
            build.loader_version.as_deref()
        ),
        ("Fabric 1.21.1", Some("1.21.1"), Some("Fabric"), Some("0.16.9"))
    );
    assert_eq!(build.loader.as_deref(), Some("fabric-loader-0.16.9-1.21.1"));
    assert_eq!(w.versions.get(&build.key), Some(build));
    assert_eq!(
        w.service.install_loader("Other", LoaderKind::Forge, "1.21.1", "52.0.1").await.unwrap_err().code,
        ErrorCode::DownloadFailed,
        "a Forge build the Maven does not have"
    );
    assert!(w.versions.get_by_name("Other").is_none());
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forge_build_is_installed_and_registered() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    fake.add_installer("forge", "1.20.1", "47.4.10", InstallerStyle::Processors);
    let w = world(&fake);
    let build =
        w.service.install_loader("Forge 1.20.1", LoaderKind::Forge, "1.20.1", "47.4.10").await.unwrap();
    assert_eq!(
        (
            build.client.as_deref(),
            build.loader.as_deref(),
            build.loader_version.as_deref(),
            build.version.as_deref()
        ),
        (Some("Forge"), Some("1.20.1-forge-47.4.10"), Some("47.4.10"), Some("1.20.1"))
    );
    assert!(build.options.contains_key("executablePath"), "the base's managed Java");
    assert!(!w.feedback.is_busy());
}

fn update_from(settings: &launcher_shared::BuildSettingsDto) -> BuildSettingsUpdate {
    BuildSettingsUpdate {
        name: settings.name.clone(),
        component: settings.component.clone(),
        java_path: settings.java_path.clone(),
        gpu_mode: settings.gpu_mode.clone(),
        max_ram_gb: settings.max_ram_gb,
        jvm_arguments: settings.jvm_arguments.clone(),
        server_host: settings.server_host.clone(),
        server_port: settings.server_port.map(|p| p.to_string()).unwrap_or_default(),
        image_path: None,
        remove_image: false,
        profile: settings.profile.clone(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_read_and_saved() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let build = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    let before = w.service.settings(&build.key).unwrap();
    assert_eq!(
        (before.component.as_deref(), before.java_path.as_deref()),
        (Some("1.21.1"), None),
        "the launcher's own Java"
    );
    if GamePlatform::current().java_runtime_key().is_some() {
        assert!(before.auto_java.as_deref().is_some_and(|p| p.contains("runtime")));
    }
    assert_eq!((before.gpu_mode.as_str(), before.max_ram_gb, before.image.as_deref()), ("dgpu", None, None));
    let java = w.games.parent().unwrap().join("jdk").join("bin").join(if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    });
    std::fs::create_dir_all(java.parent().unwrap()).unwrap();
    std::fs::write(&java, b"java").unwrap();
    let icon = w.games.parent().unwrap().join("icon.png");
    std::fs::write(&icon, b"\x89PNG\r\n\x1a\nicon").unwrap();
    let mut update = update_from(&before);
    update.name = " Aero 2 ".into();
    update.java_path = Some(java.to_string_lossy().into_owned());
    update.gpu_mode = "igpu".into();
    update.max_ram_gb = Some(4);
    update.jvm_arguments = vec!["-XX:+UseG1GC".into()];
    update.server_host = "play.example".into();
    update.server_port = "25570".into();
    update.image_path = Some(icon.to_string_lossy().into_owned());
    let after = w.service.update_settings(&build.key, update).unwrap();
    assert_eq!((after.name.as_str(), after.gpu_mode.as_str(), after.max_ram_gb), ("Aero 2", "igpu", Some(4)));
    assert!(
        after.java_path.is_some()
            && after.image.as_deref().is_some_and(|i| i.starts_with("data:image/png;base64,"))
    );
    let record = w.versions.get(&build.key).unwrap();
    assert_eq!(record.options["jvmArguments"], json!(["-Xmx4G", "-XX:+UseG1GC"]));
    assert_eq!(record.options["server"], json!({"host": "play.example", "port": 25570}));
    assert_eq!(record.key, build.key, "the key and the folder stay");
    let mut back = update_from(&after);
    back.java_path = None;
    back.remove_image = true;
    let reset = w.service.update_settings(&build.key, back).unwrap();
    assert_eq!((reset.java_path, reset.image), (None, None));
    assert!(!w.versions.get(&build.key).unwrap().options.contains_key("executablePath"));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_settings_change_nothing() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let build = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    w.service.install_vanilla("Other", "1.21.1").await.unwrap();
    let before = w.versions.get(&build.key).unwrap();
    let base = update_from(&w.service.settings(&build.key).unwrap());
    let not_image = w.games.parent().unwrap().join("icon.txt");
    std::fs::write(&not_image, b"text").unwrap();
    let cases: Vec<(BuildSettingsUpdate, ErrorCode)> = vec![
        (
            BuildSettingsUpdate { server_host: "a".into(), server_port: "abc".into(), ..base.clone() },
            ErrorCode::InvalidInput,
        ),
        (
            BuildSettingsUpdate { server_host: "a".into(), server_port: "70000".into(), ..base.clone() },
            ErrorCode::InvalidInput,
        ),
        (BuildSettingsUpdate { name: "  ".into(), ..base.clone() }, ErrorCode::VersionNameEmpty),
        (BuildSettingsUpdate { name: "OTHER".into(), ..base.clone() }, ErrorCode::VersionExists),
        (
            BuildSettingsUpdate { java_path: Some("C:/nope/java.exe".into()), ..base.clone() },
            ErrorCode::InvalidJavaExecutable,
        ),
        (
            BuildSettingsUpdate { component: Some("1.12.2".into()), ..base.clone() },
            ErrorCode::VersionNotFound,
        ),
        (BuildSettingsUpdate { component: Some("../x".into()), ..base.clone() }, ErrorCode::InvalidInput),
        (
            BuildSettingsUpdate {
                image_path: Some(not_image.to_string_lossy().into_owned()),
                ..base.clone()
            },
            ErrorCode::InvalidInput,
        ),
    ];
    for (update, code) in cases {
        assert_eq!(
            w.service.update_settings(&build.key, update.clone()).unwrap_err().code,
            code,
            "{update:?}"
        );
        assert_eq!(w.versions.get(&build.key).unwrap(), before, "nothing is written");
    }
    assert_eq!(w.service.settings("ghost").unwrap_err().code, ErrorCode::VersionNotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_changes_its_component_only_after_the_install() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let w = world(&fake);
    let build = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    assert_eq!(
        w.service.change_component(&build.key, LoaderKind::Fabric, "1.21.1", None).await.unwrap_err().code,
        ErrorCode::InvalidInput,
        "a loader needs its build"
    );
    let failed = w.service.change_component(&build.key, LoaderKind::Fabric, "1.21.1", Some("9.9.9")).await;
    assert!(failed.is_err());
    assert_eq!(
        w.versions.get(&build.key).unwrap().loader.as_deref(),
        Some("1.21.1"),
        "still on the old component"
    );
    let fabric =
        w.service.change_component(&build.key, LoaderKind::Fabric, "1.21.1", Some("0.16.9")).await.unwrap();
    assert_eq!(fabric.component.as_deref(), Some("fabric-loader-0.16.9-1.21.1"));
    let record = w.versions.get(&build.key).unwrap();
    assert_eq!(
        (record.client.as_deref(), record.loader_version.as_deref(), record.version.as_deref()),
        (Some("Fabric"), Some("0.16.9"), Some("1.21.1"))
    );
    let mut back = update_from(&fabric);
    back.component = Some("1.21.1".into());
    w.service.update_settings(&build.key, back).unwrap();
    let record = w.versions.get(&build.key).unwrap();
    assert_eq!(
        (record.loader.as_deref(), record.client.as_deref(), record.loader_version.as_deref()),
        (Some("1.21.1"), Some("Minecraft"), None)
    );
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_component_brings_its_own_java() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let w = world(&fake);
    let build = w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    let jdk8 = w.games.parent().unwrap().join("jdk8").join("bin").join(if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    });
    std::fs::create_dir_all(jdk8.parent().unwrap()).unwrap();
    std::fs::write(&jdk8, b"java").unwrap();
    let mut pinned = update_from(&w.service.settings(&build.key).unwrap());
    pinned.java_path = Some(jdk8.to_string_lossy().into_owned());
    assert!(w.service.update_settings(&build.key, pinned).unwrap().java_path.is_some());
    let changed =
        w.service.change_component(&build.key, LoaderKind::Fabric, "1.21.1", Some("0.16.9")).await.unwrap();
    if GamePlatform::current().java_runtime_key().is_some() {
        assert_eq!(changed.java_path, None, "the new component's managed Java, shown as automatic");
        let java =
            w.versions.get(&build.key).unwrap().options["executablePath"].as_str().unwrap().to_string();
        assert!(java.contains("runtime") && PathBuf::from(&java).is_file(), "{java}");
    }
}

fn names(w: &World) -> Vec<String> {
    w.service.snapshot(|_| false).builds.into_iter().map(|b| b.name).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn builds_keep_the_order_the_user_set() {
    let fake = FakeMojang::start().await;
    let w = world(&fake);
    let mut keys = Vec::new();
    for name in ["Бета", "Альфа", "Гамма", "Дельта"] {
        let mut b = Build::new(name);
        w.versions.create(&mut b).unwrap();
        keys.push(b.key);
    }
    assert_eq!(names(&w), ["Альфа", "Бета", "Гамма", "Дельта"], "by name until the user orders them");
    // Гамма first, then Бета; the others follow by name; a key no build has is ignored.
    w.config.set("build_order", json!([keys[2], "gone", keys[0]])).unwrap();
    assert_eq!(names(&w), ["Гамма", "Бета", "Альфа", "Дельта"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn reordering_saves_the_order() {
    let fake = FakeMojang::start().await;
    let w = world(&fake);
    let mut keys = Vec::new();
    for name in ["A", "B", "C"] {
        let mut b = Build::new(name);
        w.versions.create(&mut b).unwrap();
        keys.push(b.key);
    }
    let order = vec![keys[2].clone(), keys[0].clone(), keys[1].clone()];
    w.service.reorder(&order).unwrap();
    assert_eq!(w.config.get("build_order"), Some(json!(order)));
    assert_eq!(names(&w), ["C", "A", "B"]);
}

/// Holds world `world` of `folder` open as a game would: its `session.lock` locked.
fn open_world(folder: &std::path::Path, world: &str) -> std::fs::File {
    let dir = folder.join("saves").join(world);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("level.dat"), b"level").unwrap();
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("session.lock"))
        .unwrap();
    lock.lock().unwrap();
    lock
}

#[tokio::test(flavor = "multi_thread")]
async fn a_world_open_in_a_game_blocks_deleting_the_build() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    let folder = w.games.join("aero");
    // A game the launcher started before it restarted: the registry does not know it.
    let game = open_world(&folder, "World");
    let err = w.service.delete("aero", true, || false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BuildRunning);
    assert!(w.versions.get("aero").is_some() && folder.join("saves/World/level.dat").is_file());
    drop(game);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_world_does_not_block() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    let folder = w.games.join("aero");
    drop(open_world(&folder, "World"));
    w.service.copy("aero", "Aero copy", || false).await.unwrap();
    w.service.delete("aero", true, || false).unwrap();
    assert!(!folder.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn copying_a_build_with_an_open_world_is_refused() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.service.install_vanilla("Aero", "1.21.1").await.unwrap();
    assert_eq!(w.service.copy("aero", "Aero copy", || true).await.unwrap_err().code, ErrorCode::BuildRunning);
    let game = open_world(&w.games.join("aero"), "World");
    assert_eq!(
        w.service.copy("aero", "Aero copy", || false).await.unwrap_err().code,
        ErrorCode::BuildRunning
    );
    drop(game);
    assert_eq!(w.versions.list().len(), 1);
    assert!(!w.games.join("aero_copy").exists() && !w.feedback.is_busy());
}
