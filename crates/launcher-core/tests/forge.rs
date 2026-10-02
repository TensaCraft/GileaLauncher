mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::loaders::processors::LOADER_MARKER;
use launcher_core::loaders::{ComponentInstaller, ComponentSpec};
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_shared::{ErrorCode, LoaderKind};
use serde_json::Value;
use support::fake_mojang::{FakeMojang, InstallerStyle, PATCHED_BYTES, PROCESSOR_MAIN};
use support::fake_processors::FakeRunner;

struct Setup {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    runner: Arc<FakeRunner>,
    components: ComponentInstaller,
}

fn setup(fake: &FakeMojang) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let mc = tmp.path().join("minecraft");
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
    let minecraft = Arc::new(MinecraftInstaller::new(
        &mc,
        platform,
        fake.endpoints.clone(),
        meta.clone(),
        downloader,
        java,
        Arc::new(Coordinator::shared()),
    ));
    let runner = Arc::new(FakeRunner::default());
    let components =
        ComponentInstaller::new(minecraft, meta, fake.loader_endpoints(), &mc).with_runner(runner.clone());
    Setup { _tmp: tmp, mc, runner, components }
}

fn version_json(mc: &Path, id: &str) -> PathBuf {
    mc.join("versions").join(id).join(format!("{id}.json"))
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_installs_through_its_installer_and_processors() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    let forge = fake.add_installer("forge", "1.20.1", "47.4.10", InstallerStyle::Processors);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::Forge, "1.20.1", "47.4.10");
    let installed = s.components.install(&spec, false, &|_| {}).await.unwrap();
    assert_eq!(installed.id, "1.20.1-forge-47.4.10");
    let json: Value =
        serde_json::from_slice(&std::fs::read(version_json(&s.mc, &installed.id)).unwrap()).unwrap();
    assert_eq!(
        (json["id"].as_str(), json["inheritsFrom"].as_str()),
        (Some(installed.id.as_str()), Some("1.20.1"))
    );
    let calls = s.runner.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the server-only step is skipped");
    assert_eq!(calls[0].main_class, PROCESSOR_MAIN);
    assert_eq!(calls[0].args[1], s.mc.join("versions").join("1.20.1").join("1.20.1.jar").to_string_lossy());
    assert_eq!(std::fs::read(s.mc.join("libraries").join(&forge.patched_path)).unwrap(), PATCHED_BYTES);
    let dir = s.mc.join("versions").join(&installed.id);
    assert!(dir.join(LOADER_MARKER).is_file());
    assert!(!dir.join(".installer").exists(), "the unpacked installer data is cleaned up");
    assert!(s.components.minecraft().check(&installed.id).valid);
    s.components.install(&spec, false, &|_| {}).await.unwrap();
    assert_eq!(s.runner.count(), 1, "outputs that check out are not made again");
}

#[tokio::test(flavor = "multi_thread")]
async fn neoforge_installs_the_same_way() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let neo = fake.add_installer("neoforge", "1.21.1", "21.1.77", InstallerStyle::Processors);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::NeoForge, "1.21.1", "21.1.77");
    let installed = s.components.install(&spec, false, &|_| {}).await.unwrap();
    assert_eq!(installed.id, "neoforge-21.1.77");
    assert_eq!(
        installed.java.as_deref().map(Path::is_file),
        Some(true),
        "the base's Java runs the processors"
    );
    assert!(s.mc.join("libraries").join(&neo.patched_path).is_file());
    assert!(
        s.mc.join("libraries")
            .join("net/neoforged/neoforge/21.1.77/neoforge-21.1.77-installer.jar")
            .is_file(),
        "the installer is kept for repairs"
    );
    assert!(s.components.minecraft().check(&installed.id).valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn old_forge_installers_bring_their_own_jars() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.12.2");
    fake.add_vanilla("1.7.10");
    let maven_only = fake.add_installer("forge", "1.12.2", "14.23.5.2859", InstallerStyle::MavenOnly);
    let legacy = fake.add_installer("forge", "1.7.10", "10.13.4.1614-1.7.10", InstallerStyle::Legacy);
    let s = setup(&fake);
    for (mc, lv, installer) in
        [("1.12.2", "14.23.5.2859", &maven_only), ("1.7.10", "10.13.4.1614-1.7.10", &legacy)]
    {
        let installed = s
            .components
            .install(&ComponentSpec::loader(LoaderKind::Forge, mc, lv), false, &|_| {})
            .await
            .unwrap();
        assert_eq!(installed.id, installer.id);
        assert!(s.mc.join("libraries").join(&installer.loader_path).is_file(), "{mc}");
        assert!(s.components.minecraft().check(&installed.id).valid, "{mc}");
    }
    assert_eq!(s.runner.count(), 0, "no processors");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_or_wrong_processor_is_run_again_next_time() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    let forge = fake.add_installer("forge", "1.20.1", "47.4.10", InstallerStyle::Processors);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::Forge, "1.20.1", "47.4.10");
    let marker = s.mc.join("versions").join(&forge.id).join(LOADER_MARKER);
    s.runner.fail.store(true, Ordering::SeqCst);
    let error = s.components.install(&spec, false, &|_| {}).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::LoaderInstallFailed);
    assert!(error.detail.contains("processor exploded"), "{}", error.detail);
    assert!(!marker.exists());
    s.runner.fail.store(false, Ordering::SeqCst);
    s.runner.wrong_output.store(true, Ordering::SeqCst);
    assert_eq!(
        s.components.install(&spec, false, &|_| {}).await.unwrap_err().code,
        ErrorCode::LoaderInstallFailed
    );
    assert!(!marker.exists());
    s.runner.wrong_output.store(false, Ordering::SeqCst);
    s.components.ensure(&forge.id, Some(&spec), false, &|_| {}).await.unwrap();
    assert!(marker.is_file());
    assert_eq!(std::fs::read(s.mc.join("libraries").join(&forge.patched_path)).unwrap(), PATCHED_BYTES);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ready_build_needs_no_network_and_a_broken_one_is_repaired() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let neo = fake.add_installer("neoforge", "1.21.1", "21.1.77", InstallerStyle::Processors);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::NeoForge, "1.21.1", "21.1.77");
    s.components.install(&spec, false, &|_| {}).await.unwrap();
    let requests = fake.server.total_requests();
    s.components.ensure(&neo.id, Some(&spec), false, &|_| {}).await.unwrap();
    assert_eq!(fake.server.total_requests(), requests, "nothing is fetched for a ready build");
    assert_eq!(s.runner.count(), 1);
    std::fs::remove_file(s.mc.join("libraries").join(&neo.patched_path)).unwrap();
    std::fs::remove_file(version_json(&s.mc, &neo.id)).unwrap();
    s.components.ensure(&neo.id, Some(&spec), false, &|_| {}).await.unwrap();
    assert_eq!(s.runner.count(), 2, "the processors ran again");
    assert!(s.mc.join("libraries").join(&neo.patched_path).is_file());
    assert!(s.components.minecraft().check(&neo.id).valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn installers_for_another_minecraft_or_with_a_bad_checksum_are_refused() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    let s = setup(&fake);
    let requests = fake.server.total_requests();
    for (mc, lv) in [("1.20.1", "../x"), ("1.20 1", "47.4.10"), ("1.20.1", "")] {
        let spec = ComponentSpec::loader(LoaderKind::Forge, mc, lv);
        assert_eq!(
            s.components.install(&spec, false, &|_| {}).await.unwrap_err().code,
            ErrorCode::InvalidInput,
            "{mc} {lv}"
        );
    }
    assert_eq!(fake.server.total_requests(), requests, "bad values never reach the network");
    let other =
        fake.add_installer_claiming("forge", "1.20.1", "47.4.10", InstallerStyle::Processors, "1.19.2");
    let spec = ComponentSpec::loader(LoaderKind::Forge, "1.20.1", "47.4.10");
    assert_eq!(s.components.install(&spec, false, &|_| {}).await.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(!version_json(&s.mc, &other.id).exists(), "no version JSON is written");
    let tampered = fake.add_installer("forge", "1.20.1", "47.4.23", InstallerStyle::Processors);
    fake.file(&tampered.sidecar, "0".repeat(40).into_bytes());
    let spec = ComponentSpec::loader(LoaderKind::Forge, "1.20.1", "47.4.23");
    assert_eq!(
        s.components.install(&spec, false, &|_| {}).await.unwrap_err().code,
        ErrorCode::DownloadFailed
    );
}
