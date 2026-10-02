mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::lock::{Coordinator, Lease};
use launcher_core::minecraft::platform::{GameArch, GamePlatform};
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::MetaClient;
use launcher_core::paths::Os;
use launcher_shared::ErrorCode;
use support::fake_files::Served;
use support::fake_mojang::{FakeMojang, JAVA_BYTES, JAVA_COMPONENT, RuntimeEntry, java_file};

struct Setup {
    _tmp: tempfile::TempDir,
    mc: PathBuf,
    java: JavaRuntimes,
    _shared: Coordinator,
    lease: Lease,
}

/// Runtimes in `mc` with a fresh metadata cache.
fn runtimes(fake: &FakeMojang, mc: &Path, platform: GamePlatform) -> JavaRuntimes {
    let meta = Arc::new(MetaClient::new(Duration::from_secs(5), Duration::from_secs(3600)).unwrap());
    let downloader = Arc::new(
        Downloader::new(DownloaderConfig {
            retry_delay: Duration::from_millis(1),
            timeout: Duration::from_secs(5),
            ..DownloaderConfig::default()
        })
        .unwrap(),
    );
    JavaRuntimes::new(mc, platform, fake.endpoints.clone(), meta, downloader)
}

fn setup(fake: &FakeMojang, platform: GamePlatform) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let mc = tmp.path().join("minecraft");
    let java = runtimes(fake, &mc, platform);
    let shared = Coordinator::shared();
    let lease = shared.try_acquire(&mc, "java_runtime").unwrap();
    Setup { _tmp: tmp, mc, java, _shared: shared, lease }
}

fn supported() -> bool {
    GamePlatform::current().java_runtime_key().is_some()
}

fn home(mc: &Path, component: &str) -> PathBuf {
    let key = GamePlatform::current().java_runtime_key().unwrap();
    mc.join("runtime").join(component).join(key).join(component)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_a_runtime_like_the_original() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let s = setup(&fake, GamePlatform::current());
    let exe = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap().expect("a runtime");
    let home = home(&s.mc, JAVA_COMPONENT);
    assert_eq!(exe, home.join(java_file()));
    assert_eq!(fs::read(&exe).unwrap(), JAVA_BYTES, "unpacked from lzma");
    assert_eq!(fs::read(home.join("lib").join("modules")).unwrap(), vec![7u8; 3000]);
    let platform_dir = home.parent().unwrap();
    assert_eq!(fs::read_to_string(platform_dir.join(".version")).unwrap(), "21.0.3");
    let records = fs::read_to_string(platform_dir.join(format!("{JAVA_COMPONENT}.sha1"))).unwrap();
    assert_eq!(records.lines().count(), 3, "{records}");
    for line in records.lines() {
        let (path, rest) = line.split_once(" /#// ").unwrap();
        let mut rest = rest.split(' ');
        assert!(home.join(path).is_file(), "{line}");
        assert_eq!(rest.next().unwrap().len(), 40, "{line}");
        assert!(rest.next().unwrap().parse::<u128>().is_ok(), "{line}");
    }
    let leftovers: Vec<_> =
        files_under(&home).into_iter().filter(|p| p.to_string_lossy().contains(".launcher.")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0);
        assert_eq!(fs::read_link(home.join("bin").join("java-link")).unwrap(), PathBuf::from("java"));
    }
    assert!(s.java.is_complete(JAVA_COMPONENT, true));
    assert_eq!(s.java.executable(JAVA_COMPONENT), Some(exe));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_complete_runtime_is_not_downloaded_again() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let s = setup(&fake, GamePlatform::current());
    s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap();
    let before = fake.server.total_requests();
    s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap();
    assert_eq!(fake.server.total_requests(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn damaged_files_are_found_and_replaced() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let s = setup(&fake, GamePlatform::current());
    let exe = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap().unwrap();
    let modules = home(&s.mc, JAVA_COMPONENT).join("lib").join("modules");
    fs::write(&modules, vec![8u8; 3000]).unwrap();
    assert!(!s.java.is_complete(JAVA_COMPONENT, true), "a deep check hashes every file");
    s.java.repair(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap();
    assert_eq!(fs::read(&modules).unwrap(), vec![7u8; 3000]);
    fs::write(&exe, b"short").unwrap();
    assert!(!s.java.is_complete(JAVA_COMPONENT, false), "a size change is seen without hashing");
    s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap();
    assert_eq!(fs::read(&exe).unwrap(), JAVA_BYTES);
}

#[cfg(windows)]
fn hold(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    // No sharing: nobody may replace or delete the file while it is open, like a running java.exe.
    fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap()
}

#[cfg(unix)]
struct ReadOnly(PathBuf);

#[cfg(unix)]
impl Drop for ReadOnly {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[cfg(unix)]
fn hold(path: &Path) -> ReadOnly {
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    ReadOnly(dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_runtime_is_installed_to_a_new_generation() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let s = setup(&fake, GamePlatform::current());
    let exe = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap().unwrap();
    fs::write(&exe, b"stale java").unwrap();
    let guard = hold(&exe);
    let fresh = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap().unwrap();
    drop(guard);
    let generations = s.mc.join("runtime").join(".generations").join(JAVA_COMPONENT);
    assert!(fresh.starts_with(&generations), "{fresh:?}");
    assert_eq!(fs::read(&fresh).unwrap(), JAVA_BYTES);
    assert!(s.mc.join("runtime").join(".active").join(format!("{JAVA_COMPONENT}.txt")).is_file());
    assert_eq!(s.java.executable(JAVA_COMPONENT), Some(fresh));
}

#[tokio::test(flavor = "multi_thread")]
async fn platforms_without_a_mojang_runtime_get_none() {
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let arm_linux = GamePlatform { os: Os::Linux, arch: GameArch::Arm64, os_version: String::new() };
    let s = setup(&fake, arm_linux);
    assert_eq!(s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap(), None);
    assert!(s.java.unavailable(JAVA_COMPONENT));
    assert_eq!(fake.server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_builds_is_remembered() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_runtime_without_builds("jre-legacy");
    let s = setup(&fake, GamePlatform::current());
    assert_eq!(s.java.ensure("jre-legacy", &s.lease, &|_| {}).await.unwrap(), None);
    // A new launcher run: no metadata cache, only what is on disk.
    let again = runtimes(&fake, &s.mc, GamePlatform::current());
    assert!(again.unavailable("jre-legacy"));
    let before = fake.server.total_requests();
    assert_eq!(again.ensure("jre-legacy", &s.lease, &|_| {}).await.unwrap(), None);
    assert_eq!(fake.server.total_requests(), before, "no second question to Mojang");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_or_hostile_runtimes_fail_clearly() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    // Published before the first request: the runtime list is cached for an hour.
    let file = || RuntimeEntry::File { bytes: b"x".to_vec(), executable: false, lzma: false };
    fake.add_runtime("evil-path", "1", &[("../../escape.txt", file())]);
    fake.add_runtime(
        "evil-link",
        "1",
        &[("bin/java", file()), ("bin/out", RuntimeEntry::Link("../../../../../outside"))],
    );
    let s = setup(&fake, GamePlatform::current());
    let err = s.java.ensure("java-runtime-nope", &s.lease, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::JavaRuntimeFailed);
    assert_eq!(err.params["component"], "java-runtime-nope");
    let err = s.java.ensure("evil-path", &s.lease, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::JavaRuntimeFailed);
    assert!(err.detail.contains("unsafe path"), "{}", err.detail);
    assert!(!s.mc.join("runtime").join("evil-path").join("escape.txt").exists());
    let err = s.java.ensure("evil-link", &s.lease, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::JavaRuntimeFailed);
    assert!(err.detail.contains("unsafe link"), "{}", err.detail);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_failure_keeps_the_runtime_in_place() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    fake.add_default_runtime(JAVA_COMPONENT);
    let s = setup(&fake, GamePlatform::current());
    let exe = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap().unwrap();
    let home = home(&s.mc, JAVA_COMPONENT);
    fs::remove_file(home.join("release")).unwrap();
    fake.server.put(
        &format!("java/{JAVA_COMPONENT}/lzma/release"),
        Served { status: Some(503), ..Served::default() },
    );
    let err = s.java.ensure(JAVA_COMPONENT, &s.lease, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed);
    assert_eq!(fs::read(&exe).unwrap(), JAVA_BYTES, "a network failure erases nothing");
    assert!(home.join("lib").join("modules").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_java_is_an_error() {
    if !supported() {
        return;
    }
    let fake = FakeMojang::start().await;
    let release = RuntimeEntry::File { bytes: b"x".to_vec(), executable: false, lzma: false };
    fake.add_runtime("no-java", "1", &[("release", release)]);
    let s = setup(&fake, GamePlatform::current());
    let err = s.java.ensure("no-java", &s.lease, &|_| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::JavaRuntimeFailed);
    assert!(err.detail.contains("no java"), "{}", err.detail);
}
