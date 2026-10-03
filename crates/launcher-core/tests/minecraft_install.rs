mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::integrity::{INSTALLED_MARKER, INSTALLING_MARKER, check_version};
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_shared::ErrorCode;
use serde_json::json;
use support::fake_files::Served;
use support::fake_mojang::{FakeMojang, JAVA_BYTES, JAVA_COMPONENT};

struct Setup {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    installer: MinecraftInstaller,
    java: Arc<JavaRuntimes>,
    shared: Arc<Coordinator>,
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
    let shared = Arc::new(Coordinator::shared());
    let installer = MinecraftInstaller::new(
        &mc,
        platform,
        fake.endpoints.clone(),
        meta,
        downloader,
        java.clone(),
        shared.clone(),
    );
    Setup { _tmp: tmp, mc, installer, java, shared }
}

fn java_supported() -> bool {
    GamePlatform::current().java_runtime_key().is_some()
}

fn object(mc: &Path, hash: &str) -> PathBuf {
    mc.join("assets").join("objects").join(&hash[..2]).join(hash)
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_a_vanilla_version() {
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    let installed = s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let dir = s.mc.join("versions").join("1.21.1");
    assert_eq!(installed.id, "1.21.1");
    assert_eq!(fs::read(dir.join("1.21.1.jar")).unwrap(), v.client);
    assert!(dir.join("1.21.1.json").is_file());
    assert_eq!(fs::read(s.mc.join("libraries").join(v.library_path)).unwrap(), v.library);
    assert!(!s.mc.join("libraries").join(v.skipped_library_path).exists(), "its rules never match");
    assert_eq!(fs::read_to_string(dir.join("natives").join("lwjgl64.dll")).unwrap(), "native code");
    assert!(!dir.join("natives").join("META-INF").exists());
    for (hash, bytes) in &v.objects {
        assert_eq!(&fs::read(object(&s.mc, hash)).unwrap(), bytes);
    }
    assert!(s.mc.join("assets").join("indexes").join("1.21.1.json").is_file());
    let log = s.mc.join("assets").join("log_configs").join("client-1.12.xml");
    assert_eq!(fs::read(log).unwrap(), v.log_config);
    assert!(dir.join(INSTALLED_MARKER).is_file() && !dir.join(INSTALLING_MARKER).exists());
    if java_supported() {
        let java = installed.java.clone().expect("managed Java");
        assert_eq!(fs::read(&java).unwrap(), JAVA_BYTES);
        assert_eq!(s.java.executable(JAVA_COMPONENT), Some(java));
    }
    let check = check_version(&s.mc, "1.21.1", &GamePlatform::current(), Some(&s.java), false);
    assert!(check.valid, "{:?}", check.issues);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_install_downloads_nothing() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let before = fake.server.total_requests();
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    assert_eq!(fake.server.total_requests(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn ensure_installed_repairs_missing_files() {
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let before = fake.server.total_requests();
    let ready = s.installer.ensure_installed("1.21.1", false, &|_| {}).await.unwrap();
    assert_eq!(fake.server.total_requests(), before, "a valid version is left alone");
    assert_eq!(ready.java, s.java.executable(JAVA_COMPONENT));
    fs::remove_file(s.mc.join("libraries").join(v.library_path)).unwrap();
    fs::remove_file(object(&s.mc, &v.objects[1].0)).unwrap();
    assert!(!s.installer.check("1.21.1").valid);
    s.installer.ensure_installed("1.21.1", false, &|_| {}).await.unwrap();
    assert_eq!(fs::read(s.mc.join("libraries").join(v.library_path)).unwrap(), v.library);
    assert!(object(&s.mc, &v.objects[1].0).is_file());
    assert_eq!(fake.server.total_requests(), before + 2, "only the two missing files");
    assert!(s.installer.check("1.21.1").valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_repair_reads_only_what_is_missing() {
    // One missing library before a launch: the files of the right size are trusted, not hashed
    // again one by one (thousands of assets); the Components page's Verify hashes them.
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let kept = object(&s.mc, &v.objects[0].0);
    let len = fs::metadata(&kept).unwrap().len() as usize;
    fs::write(&kept, vec![b'#'; len]).unwrap();
    fs::remove_file(s.mc.join("libraries").join(v.library_path)).unwrap();
    s.installer.ensure_installed("1.21.1", false, &|_| {}).await.unwrap();
    assert_eq!(fs::read(s.mc.join("libraries").join(v.library_path)).unwrap(), v.library);
    assert_eq!(fs::read(&kept).unwrap(), vec![b'#'; len], "a file of the right size was not hashed");
    s.installer.ensure_installed("1.21.1", true, &|_| {}).await.unwrap();
    assert_ne!(fs::read(&kept).unwrap(), vec![b'#'; len], "a forced check hashes every file");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_install_is_repaired() {
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let dir = s.mc.join("versions").join("1.21.1");
    fs::remove_file(dir.join(INSTALLED_MARKER)).unwrap();
    fs::write(dir.join(INSTALLING_MARKER), b"").unwrap();
    fs::write(dir.join("1.21.1.jar"), b"half").unwrap();
    assert!(!s.installer.check("1.21.1").valid);
    s.installer.ensure_installed("1.21.1", false, &|_| {}).await.unwrap();
    assert_eq!(fs::read(dir.join("1.21.1.jar")).unwrap(), v.client);
    assert!(dir.join(INSTALLED_MARKER).is_file() && !dir.join(INSTALLING_MARKER).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_java_makes_the_version_incomplete() {
    if !java_supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    fs::remove_dir_all(s.mc.join("runtime").join(JAVA_COMPONENT)).unwrap();
    let check = s.installer.check("1.21.1");
    assert!(!check.valid && !check.components.java, "{check:?}");
    s.installer.ensure_installed("1.21.1", false, &|_| {}).await.unwrap();
    assert!(s.installer.check("1.21.1").valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_versions_are_not_found() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    let err = s.installer.install("9.9.9", false, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionNotFound);
    assert!(!s.mc.join("versions").join("9.9.9").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_install_at_once_is_busy() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    let _held = s.shared.try_acquire(&s.mc, "java_runtime").unwrap();
    let err = s.installer.install("1.21.1", false, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::SharedBusy);
    assert_eq!(fake.server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_metadata_is_refused() {
    let fake = FakeMojang::start().await;
    let mut evil = fake.entry("libs/evil.jar", b"evil");
    evil["path"] = json!("../../../evil.jar");
    fake.add_version_json(&json!({"id": "evil", "mainClass": "M",
        "libraries": [{"name": "a:evil:1", "downloads": {"artifact": evil}}]}));
    let s = setup(&fake);
    let err = s.installer.install("evil", false, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(!s.mc.parent().unwrap().join("evil.jar").exists());
    assert!(!s.mc.join("evil.jar").exists());
    assert_eq!(fake.server.seen("libs/evil.jar").len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_version_file_is_fetched_again() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    let path = s.mc.join("versions").join("1.21.1").join("1.21.1.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ broken").unwrap();
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["id"], json!("1.21.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn network_failures_retry_the_whole_install() {
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    fake.server
        .put("clients/1.21.1.jar", Served { body: v.client.clone(), fail_times: 4, ..Served::default() });
    let s = setup(&fake);
    s.installer.install("1.21.1", false, &|_| {}).await.unwrap();
    let jar = s.mc.join("versions").join("1.21.1").join("1.21.1.jar");
    assert_eq!(fs::read(jar).unwrap(), v.client);
    assert_eq!(
        fake.server.seen("clients/1.21.1.jar").len(),
        5,
        "3 tries in the first attempt, 2 in the second"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inheritance_loops_are_refused() {
    let fake = FakeMojang::start().await;
    let s = setup(&fake);
    for (id, parent) in [("loop-a", "loop-b"), ("loop-b", "loop-a"), ("self-loop", "self-loop")] {
        let dir = s.mc.join("versions").join(id);
        fs::create_dir_all(&dir).unwrap();
        let json = json!({"id": id, "inheritsFrom": parent, "mainClass": "M"});
        fs::write(dir.join(format!("{id}.json")), json.to_string()).unwrap();
    }
    for id in ["loop-a", "self-loop"] {
        let err = s.installer.install(id, false, &|_| {}).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{id}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_version_installs_its_parent_first() {
    let fake = FakeMojang::start().await;
    let v = fake.add_vanilla("1.21.1");
    let loader = b"loader library".to_vec();
    let mut library = fake.entry("libs/loader.jar", &loader);
    library["path"] = json!("net/example/loader/1.0/loader-1.0.jar");
    fake.add_version_json(
        &json!({"id": "loader-1.0-1.21.1", "inheritsFrom": "1.21.1", "mainClass": "net.example.Knot",
        "libraries": [{"name": "net.example:loader:1.0", "downloads": {"artifact": library}}]}),
    );
    let s = setup(&fake);
    s.installer.install("loader-1.0-1.21.1", false, &|_| {}).await.unwrap();
    let parent = s.mc.join("versions").join("1.21.1");
    assert!(parent.join(INSTALLED_MARKER).is_file());
    assert_eq!(fs::read(parent.join("1.21.1.jar")).unwrap(), v.client);
    let child = s.mc.join("versions").join("loader-1.0-1.21.1");
    assert_eq!(
        fs::read(child.join("loader-1.0-1.21.1.jar")).unwrap(),
        v.client,
        "the client jar lands in the child too"
    );
    assert_eq!(fs::read(s.mc.join("libraries/net/example/loader/1.0/loader-1.0.jar")).unwrap(), loader);
    assert!(s.installer.check("loader-1.0-1.21.1").valid);
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_parts_do_not_force_repairs() {
    let fake = FakeMojang::start().await;
    fake.add_default_runtime("jre-legacy");
    fake.file("maven/org/example/empty/1/empty-1.jar", Vec::new());
    fake.add_version_json(&json!({"id": "bare", "mainClass": "M", "assets": "legacy-custom",
        "libraries": [{"name": "org.example:empty:1"}]}));
    let s = setup(&fake);
    s.installer.install("bare", false, &|_| {}).await.unwrap();
    let check = s.installer.check("bare");
    assert!(check.valid, "{:?}", check.issues);
    let before = fake.server.total_requests();
    s.installer.ensure_installed("bare", false, &|_| {}).await.unwrap();
    assert_eq!(fake.server.total_requests(), before, "nothing to repair");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_lists_manifest_versions_newest_first() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.20.1");
    fake.add_vanilla("1.21.1");
    let s = setup(&fake);
    let catalog = s.installer.catalog(false).await.unwrap();
    let ids: Vec<(&str, &str)> = catalog.iter().map(|v| (v.id.as_str(), v.kind.as_str())).collect();
    assert_eq!(ids, [("1.21.1", "release"), ("1.20.1", "release")]);
    assert!(!s.mc.join("versions").exists(), "the catalog installs nothing");
}
