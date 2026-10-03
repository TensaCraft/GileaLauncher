mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::loaders::fabric::FabricMeta;
use launcher_core::loaders::{ComponentInstaller, ComponentSpec};
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_shared::{ErrorCode, LoaderKind};
use serde_json::json;
use support::fake_mojang::FakeMojang;

fn meta() -> Arc<MetaClient> {
    Arc::new(MetaClient::new(Duration::from_secs(5), Duration::from_secs(3600)).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn fabric_meta_lists_games_loaders_and_profiles() {
    let fake = FakeMojang::start().await;
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    fake.add_loader("fabric", "1.21.1", "0.17.0-beta.1", false);
    let fabric = FabricMeta::new(LoaderKind::Fabric, &fake.loader_endpoints().fabric, meta());
    assert_eq!(fabric.games().await.unwrap(), [("1.21.1".to_string(), true)]);
    let loaders = fabric.loaders().await.unwrap();
    let pairs: Vec<(&str, bool)> = loaders.iter().map(|b| (b.version.as_str(), b.stable)).collect();
    assert_eq!(pairs, [("0.17.0-beta.1", false), ("0.16.9", true)]);
    let profile = fabric.profile("1.21.1", "0.16.9").await.unwrap();
    assert_eq!(profile["id"], json!("fabric-loader-0.16.9-1.21.1"));
    let requests = fake.server.total_requests();
    for (mc, lv) in [("../x", "0.16.9"), ("1.21.1", "0.16 9"), ("1.21.1", "")] {
        assert_eq!(fabric.profile(mc, lv).await.unwrap_err().code, ErrorCode::InvalidInput, "{mc} {lv}");
    }
    assert_eq!(fake.server.total_requests(), requests, "bad values never reach the network");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_with_another_id_is_refused() {
    let fake = FakeMojang::start().await;
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    fake.file(
        "fabric/v2/versions/loader/1.21.1/0.16.8/profile/json",
        json!({"id": "fabric-loader-0.16.9-1.21.1", "inheritsFrom": "1.21.1"}).to_string().into_bytes(),
    );
    let fabric = FabricMeta::new(LoaderKind::Fabric, &fake.loader_endpoints().fabric, meta());
    assert_eq!(fabric.profile("1.21.1", "0.16.8").await.unwrap_err().code, ErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn quilt_prereleases_are_unstable() {
    let fake = FakeMojang::start().await;
    fake.add_loader("quilt", "1.21.1", "0.26.4", true);
    fake.add_loader("quilt", "1.21.1", "0.27.0-beta.1", true);
    let quilt = FabricMeta::new(LoaderKind::Quilt, &fake.loader_endpoints().quilt, meta());
    let pairs: Vec<(String, bool)> =
        quilt.loaders().await.unwrap().into_iter().map(|b| (b.version, b.stable)).collect();
    assert_eq!(pairs, [("0.27.0-beta.1".to_string(), false), ("0.26.4".to_string(), true)]);
    assert_eq!(quilt.profile("1.21.1", "0.26.4").await.unwrap()["id"], json!("quilt-loader-0.26.4-1.21.1"));
}

struct Setup {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    minecraft: Arc<MinecraftInstaller>,
    components: ComponentInstaller,
}

fn setup(fake: &FakeMojang) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let mc = tmp.path().join("minecraft");
    let meta = meta();
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
    let components = ComponentInstaller::new(minecraft.clone(), meta, fake.loader_endpoints(), &mc);
    Setup { _tmp: tmp, mc, minecraft, components }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_install_leaves_the_loader_profile_alone() {
    // Another install holds the shared folder: a Fabric install is refused before it fetches and
    // rewrites its profile in versions/.
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let s = setup(&fake);
    let held = s.minecraft.lock("minecraft_install").unwrap();
    let spec = ComponentSpec::loader(LoaderKind::Fabric, "1.21.1", "0.16.9");
    let before = fake.server.total_requests();
    let e = s.components.install(&spec, false, &|_| {}).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::SharedBusy);
    assert!(!s.mc.join("versions").join(spec.component_id()).exists(), "no profile was written");
    assert_eq!(fake.server.total_requests(), before, "nothing was fetched");
    drop(held);
    s.components.install(&spec, false, &|_| {}).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_offers_supported_minecraft_versions_with_loader_builds() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    fake.add_loader("fabric", "1.21.1", "0.17.0-beta.1", false);
    fake.add_loader("fabric", "1.99", "0.16.9", true);
    let s = setup(&fake);
    let stable = s.components.catalog(LoaderKind::Fabric, false).await.unwrap();
    let mcs: Vec<&str> = stable.iter().map(|o| o.mc.as_str()).collect();
    assert_eq!(mcs, ["1.21.1"], "1.20.1 lacks Fabric; 1.99 is unknown to Mojang");
    assert_eq!(
        (stable[0].default_version.as_str(), stable[0].builds.len(), stable[0].snapshot),
        ("0.16.9", 1, false)
    );
    let unstable = s.components.catalog(LoaderKind::Fabric, true).await.unwrap();
    let builds: Vec<&str> = unstable[0].builds.iter().map(|b| b.version.as_str()).collect();
    assert_eq!(builds, ["0.17.0-beta.1", "0.16.9"]);
    for kind in [LoaderKind::Minecraft] {
        assert_eq!(
            s.components.catalog(kind, false).await.unwrap_err().code,
            ErrorCode::InvalidInput,
            "{kind:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fabric_build_installs_its_base_and_libraries_and_repairs_its_profile() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::Fabric, "1.21.1", "0.16.9");
    let installed = s.components.install(&spec, false, &|_| {}).await.unwrap();
    assert_eq!(installed.id, "fabric-loader-0.16.9-1.21.1");
    let profile = s.mc.join("versions").join(&installed.id).join(format!("{}.json", installed.id));
    assert!(profile.is_file());
    assert!(s.mc.join("libraries/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar").is_file());
    assert!(s.mc.join("versions/1.21.1/1.21.1.jar").is_file(), "the base comes along");
    assert!(s.components.minecraft().check(&installed.id).valid);
    std::fs::remove_file(&profile).unwrap();
    let repaired = s.components.ensure(&installed.id, Some(&spec), false, &|_| {}).await.unwrap();
    assert_eq!(repaired.id, installed.id);
    assert!(profile.is_file(), "the profile is fetched again");
    assert!(s.components.minecraft().check(&installed.id).valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn quilt_installs_the_same_way() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_loader("quilt", "1.21.1", "0.26.4", true);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::Quilt, "1.21.1", "0.26.4");
    let installed = s.components.install(&spec, false, &|_| {}).await.unwrap();
    assert_eq!(installed.id, "quilt-loader-0.26.4-1.21.1");
    assert!(s.mc.join("libraries/org/quiltmc/quilt-loader/0.26.4/quilt-loader-0.26.4.jar").is_file());
    assert!(s.components.minecraft().check(&installed.id).valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn fabric_offers_its_whole_newest_family_though_it_marks_one_build_stable() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    // Like meta.fabricmc.net: only the newest build is marked stable.
    fake.add_loader("fabric", "1.21.1", "0.15.11", false);
    fake.add_loader("fabric", "1.21.1", "0.16.8", false);
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let s = setup(&fake);
    let offered = s.components.catalog(LoaderKind::Fabric, false).await.unwrap();
    let builds: Vec<&str> = offered[0].builds.iter().map(|b| b.version.as_str()).collect();
    assert_eq!(builds, ["0.16.9", "0.16.8"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_offers_each_minecraft_its_own_builds_and_the_recommended_default() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    fake.add_vanilla("1.21.1");
    fake.list_forge("1.20.1", "47.4.10");
    fake.list_forge("1.20.1", "47.4.23");
    fake.list_forge("1.19.2", "43.4.0");
    fake.list_forge("1.21.1", "52.1.16");
    let s = setup(&fake);
    let first = s.components.catalog(LoaderKind::Forge, false).await.unwrap();
    assert_eq!(first[1].default_version, "47.4.23", "without recommendations the newest build");
    fake.recommend_forge("1.20.1", "47.4.10");
    let options = s.components.catalog(LoaderKind::Forge, false).await.unwrap();
    let rows: Vec<(&str, Vec<&str>, &str)> = options
        .iter()
        .map(|o| {
            (o.mc.as_str(), o.builds.iter().map(|b| b.version.as_str()).collect(), o.default_version.as_str())
        })
        .collect();
    assert_eq!(
        rows,
        [("1.21.1", vec!["52.1.16"], "52.1.16"), ("1.20.1", vec!["47.4.23", "47.4.10"], "47.4.10")],
        "1.19.2 is unknown to Mojang here"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn neoforge_builds_follow_their_minecraft_and_betas_wait_for_unstable() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    fake.add_vanilla("1.21.4");
    fake.list_neoforge("21.1.77");
    fake.list_neoforge("21.1.78-beta");
    fake.list_neoforge("21.4.0-beta");
    fake.list_neoforge("0.25w14craftmine.3-beta");
    let s = setup(&fake);
    let stable = s.components.catalog(LoaderKind::NeoForge, false).await.unwrap();
    assert_eq!(stable.iter().map(|o| o.mc.as_str()).collect::<Vec<_>>(), ["1.21.1"], "1.21.4 has only betas");
    assert_eq!(stable[0].builds.len(), 1);
    let unstable = s.components.catalog(LoaderKind::NeoForge, true).await.unwrap();
    let rows: Vec<(&str, &str, usize)> =
        unstable.iter().map(|o| (o.mc.as_str(), o.default_version.as_str(), o.builds.len())).collect();
    assert_eq!(rows, [("1.21.4", "21.4.0-beta", 1), ("1.21.1", "21.1.77", 2)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_finds_same_size_damage_and_repair_fixes_it() {
    let fake = FakeMojang::start().await;
    let vanilla = fake.add_vanilla("1.21.1");
    fake.add_loader("fabric", "1.21.1", "0.16.9", true);
    let s = setup(&fake);
    let spec = ComponentSpec::loader(LoaderKind::Fabric, "1.21.1", "0.16.9");
    let id = s.components.install(&spec, false, &|_| {}).await.unwrap().id;
    assert!(s.components.verify(&id, Some(&spec)).await.valid);
    let library = s.mc.join("libraries").join(vanilla.library_path);
    std::fs::write(&library, vec![b'X'; vanilla.library.len()]).unwrap();
    assert!(s.components.minecraft().check(&id).valid, "the quick check sees the right size");
    let check = s.components.verify(&id, Some(&spec)).await;
    assert!(!check.valid && !check.components.libraries, "{:?}", check.issues);
    s.components.repair(&id, Some(&spec), &|_| {}).await.unwrap();
    assert_eq!(std::fs::read(&library).unwrap(), vanilla.library);
    assert!(s.components.verify(&id, Some(&spec)).await.valid);
    s.components.repair("1.21.1", None, &|_| {}).await.unwrap();
}
