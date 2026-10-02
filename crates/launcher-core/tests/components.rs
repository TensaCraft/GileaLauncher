mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::builds::components::ComponentManager;
use launcher_core::feedback::{FeedbackService, NullSink, OperationSpec};
use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::loaders::processors::LOADER_MARKER;
use launcher_core::loaders::{ComponentInstaller, ComponentSpec};
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{ErrorCode, LoaderKind, Text, VerifyOutcome};
use support::fake_mojang::{FakeMojang, InstallerStyle, PATCHED_BYTES};
use support::fake_processors::FakeRunner;

struct World {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    versions: Arc<VersionStore>,
    feedback: Arc<FeedbackService>,
    components: Arc<ComponentInstaller>,
    manager: ComponentManager,
}

fn world(fake: &FakeMojang) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
    std::fs::create_dir_all(&state).unwrap();
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
    let components = Arc::new(
        ComponentInstaller::new(minecraft, meta, fake.loader_endpoints(), &mc)
            .with_runner(Arc::new(FakeRunner::default())),
    );
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let feedback = FeedbackService::new(Arc::new(NullSink));
    let manager = ComponentManager::new(versions.clone(), components.clone(), feedback.clone());
    World { _tmp: tmp, mc, versions, feedback, components, manager }
}

const FABRIC: &str = "fabric-loader-0.16.9-1.21.1";

#[tokio::test(flavor = "multi_thread")]
async fn installed_components_are_listed_verified_and_repaired() {
    let fake = FakeMojang::start().await;
    let vanilla = fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let w = world(&fake);
    assert_eq!(
        w.manager.install(LoaderKind::Fabric, "1.21.1", None).await.unwrap_err().code,
        ErrorCode::InvalidInput,
        "a loader needs its build"
    );
    w.manager.install(LoaderKind::Fabric, "1.21.1", Some("0.16.9")).await.unwrap();
    let list = w.manager.list().await.components;
    let ids: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["1.21.1", FABRIC]);
    assert_eq!(list[0].base_for, [FABRIC]);
    assert_eq!(w.manager.verify(FABRIC).await.unwrap(), VerifyOutcome::Intact);
    let library = w.mc.join("libraries").join(vanilla.library_path);
    std::fs::write(&library, vec![b'X'; vanilla.library.len()]).unwrap();
    assert_eq!(w.manager.verify("1.21.1").await.unwrap(), VerifyOutcome::Repaired, "same size, wrong bytes");
    assert_eq!(std::fs::read(&library).unwrap(), vanilla.library);
    let jar = w.mc.join("versions").join("1.21.1").join("1.21.1.jar");
    std::fs::remove_file(&jar).unwrap();
    w.manager.reinstall("1.21.1").await.unwrap();
    assert!(jar.is_file());
    assert!(!w.feedback.is_busy());
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_components_verify_their_installer_steps() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    let forge = fake.add_installer("forge", "1.20.1", "47.4.10", InstallerStyle::Processors);
    let w = world(&fake);
    w.manager.install(LoaderKind::Forge, "1.20.1", Some("47.4.10")).await.unwrap();
    let marker = w.mc.join("versions").join(&forge.id).join(LOADER_MARKER);
    std::fs::remove_file(&marker).unwrap();
    assert_eq!(w.manager.verify(&forge.id).await.unwrap(), VerifyOutcome::Repaired);
    assert!(marker.is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_needs_a_safe_installed_id_and_no_running_build() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let w = world(&fake);
    w.manager.install(LoaderKind::Fabric, "1.21.1", Some("0.16.9")).await.unwrap();
    for bad in ["../x", "..", "C:\\evil", "", "versions/1.21.1"] {
        assert_eq!(w.manager.delete(bad, |_| false).unwrap_err().code, ErrorCode::InvalidInput, "{bad:?}");
    }
    assert_eq!(w.manager.delete("1.12.2", |_| false).unwrap_err().code, ErrorCode::VersionNotFound);
    assert_eq!(w.manager.dir("1.12.2").unwrap_err().code, ErrorCode::VersionNotFound);
    let mut build = Build::new("Aero");
    build.version = Some("1.21.1".into());
    build.loader = Some(FABRIC.into());
    w.versions.create(&mut build).unwrap();
    let key = build.key.clone();
    assert_eq!(w.manager.delete(FABRIC, |running| running == key).unwrap_err().code, ErrorCode::BuildRunning);
    assert!(w.mc.join("versions").join(FABRIC).is_dir());
    w.manager.delete("1.21.1", |_| false).unwrap();
    assert!(!w.mc.join("versions").join("1.21.1").exists(), "a base with dependents goes after the warning");
    assert!(w.mc.join("libraries").is_dir() && w.mc.join("assets").is_dir(), "shared files stay");
    let ids: Vec<String> = w.manager.list().await.components.into_iter().map(|c| c.id).collect();
    assert_eq!(ids, [FABRIC]);
    let spec = ComponentSpec::loader(LoaderKind::Fabric, "1.21.1", "0.16.9");
    w.components.ensure(FABRIC, Some(&spec), false, &|_| {}).await.unwrap();
    assert!(
        w.mc.join("versions").join("1.21.1").join("1.21.1.json").is_file(),
        "the next launch brings the base back"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn component_operations_wait_for_each_other() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    w.manager.install(LoaderKind::Minecraft, "1.21.1", None).await.unwrap();
    let op = w.feedback.begin(OperationSpec::new(Text::key("installation_started"), "install"));
    assert_eq!(w.manager.verify("1.21.1").await.unwrap_err().code, ErrorCode::Busy);
    assert_eq!(w.manager.reinstall("1.21.1").await.unwrap_err().code, ErrorCode::Busy);
    assert_eq!(
        w.manager.install(LoaderKind::Minecraft, "1.21.1", None).await.unwrap_err().code,
        ErrorCode::Busy
    );
    assert_eq!(w.manager.delete("1.21.1", |_| false).unwrap_err().code, ErrorCode::Busy);
    drop(op);
    assert!(w.mc.join("versions").join("1.21.1").is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_verify_checks_what_the_installer_made() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    fake.add_vanilla("1.12.2");
    let forge = fake.add_installer("forge", "1.20.1", "47.4.10", InstallerStyle::Processors);
    let old = fake.add_installer("forge", "1.12.2", "14.23.5.2859", InstallerStyle::MavenOnly);
    let w = world(&fake);
    w.manager.install(LoaderKind::Forge, "1.20.1", Some("47.4.10")).await.unwrap();
    w.manager.install(LoaderKind::Forge, "1.12.2", Some("14.23.5.2859")).await.unwrap();
    let patched = w.mc.join("libraries").join(&forge.patched_path);
    std::fs::write(&patched, vec![b'X'; PATCHED_BYTES.len()]).unwrap();
    assert_eq!(w.manager.verify(&forge.id).await.unwrap(), VerifyOutcome::Repaired, "same size, wrong bytes");
    assert_eq!(std::fs::read(&patched).unwrap(), PATCHED_BYTES);
    let shipped = w.mc.join("libraries").join(&old.loader_path);
    std::fs::remove_file(&shipped).unwrap();
    assert_eq!(
        w.manager.verify(&old.id).await.unwrap(),
        VerifyOutcome::Repaired,
        "a jar the installer shipped"
    );
    assert!(shipped.is_file());
    assert_eq!(w.manager.verify(&old.id).await.unwrap(), VerifyOutcome::Intact);
}
