use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use launcher_core::paths::{Os, PathEnv};
use launcher_core::updater::apply::{self, EXIT_OK, HelperArgs};
use launcher_core::updater::download::sha256_file;
use launcher_core::updater::stage::{self, ExecContext};

const FIXTURE: &str = env!("CARGO_BIN_EXE_update-fixture");

fn read_output(dir: &Path, timeout: Duration) -> Option<String> {
    let path = dir.join("fixture-out.txt");
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(text) = fs::read_to_string(&path)
            && !text.is_empty()
        {
            return Some(text);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// A process to wait for, reaped in a thread so it never lingers as a zombie on Unix.
fn waited_process(ms: u64) -> (u32, std::thread::JoinHandle<()>) {
    let mut child = Command::new(FIXTURE).args(["--sleep-ms", &ms.to_string()]).spawn().unwrap();
    let pid = child.id();
    (
        pid,
        std::thread::spawn(move || {
            let _ = child.wait();
        }),
    )
}

#[cfg(not(target_os = "macos"))]
mod plain {
    use super::*;
    use launcher_core::updater::apply::{EXIT_BUSY, EXIT_INVALID, EXIT_TIMEOUT};
    use launcher_core::updater::stage::ResumeOutcome;

    fn tagged_copy(dest: &Path, tag: &str) {
        let mut bytes = fs::read(FIXTURE).unwrap();
        bytes.extend_from_slice(format!("\nUAFIXTURE:{tag}\n").as_bytes());
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(dest, bytes).unwrap();
        stage::set_mode(dest, 0o755).unwrap();
    }

    fn tag_of(path: &Path) -> String {
        let text = String::from_utf8_lossy(&fs::read(path).unwrap()).into_owned();
        text.rsplit("UAFIXTURE:").next().unwrap().trim().to_string()
    }

    struct Install {
        _tmp: tempfile::TempDir,
        cache: PathBuf,
        target: PathBuf,
        ctx: ExecContext,
    }

    /// A tagged "v1" launcher in a folder with Cyrillic, spaces, `&` and `%`, and a staged "v2".
    fn install() -> Install {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Іван & 100% Co").join("app").join(if cfg!(windows) {
            "Launcher.exe"
        } else {
            "Launcher"
        });
        tagged_copy(&target, "v1");
        let payload = tmp.path().join("download").join("Launcher-v2");
        tagged_copy(&payload, "v2");
        let ctx = ExecContext {
            os: Os::current(),
            current_exe: target.clone(),
            appimage: None,
            pid: std::process::id(),
        };
        let cache = tmp.path().join("Іван & 100% Co").join("cache");
        let sha = sha256_file(&payload).unwrap();
        stage::prepare(&cache, &ctx, &PathEnv::default(), &payload, "0.2.0", &sha).unwrap();
        Install { _tmp: tmp, cache, target, ctx }
    }

    #[test]
    fn helper_replaces_the_program_and_starts_the_new_version() {
        let i = install();
        let (pid, reaper) = waited_process(700);
        let args = HelperArgs {
            marker: stage::marker_path(&i.cache),
            wait_pid: pid,
            wait_timeout: Duration::from_secs(20),
        };
        let code = apply::run_helper(&args);
        reaper.join().unwrap();
        assert_eq!(code, EXIT_OK);
        assert_eq!(tag_of(&i.target), "v2");
        assert_eq!(read_output(i.target.parent().unwrap(), Duration::from_secs(15)).as_deref(), Some("v2"));
        assert!(!stage::marker_path(&i.cache).exists());
        let mut bak = i.target.as_os_str().to_os_string();
        bak.push(".bak");
        assert!(!PathBuf::from(bak).exists());
    }

    #[test]
    fn helper_leaves_everything_in_place_if_the_launcher_does_not_exit() {
        let i = install();
        let (pid, reaper) = waited_process(4000);
        let args = HelperArgs {
            marker: stage::marker_path(&i.cache),
            wait_pid: pid,
            wait_timeout: Duration::from_millis(300),
        };
        let started = stage::staging_dir(&i.cache).join(stage::HELPER_STARTED);
        fs::write(&started, b"").unwrap();
        assert_eq!(apply::run_helper(&args), EXIT_TIMEOUT);
        assert_eq!(tag_of(&i.target), "v1");
        assert!(stage::marker_path(&i.cache).exists(), "the update stays pending");
        assert!(!started.exists(), "the next start must try again");
        reaper.join().unwrap();
    }

    #[test]
    fn helper_refuses_a_tampered_payload() {
        let i = install();
        let marker = stage::validate_marker(&i.cache, Os::current(), None).unwrap();
        fs::write(&marker.source, b"not what was downloaded").unwrap();
        let args = HelperArgs {
            marker: stage::marker_path(&i.cache),
            wait_pid: std::process::id(),
            wait_timeout: Duration::from_secs(20),
        };
        assert_eq!(apply::run_helper(&args), EXIT_INVALID);
        assert_eq!(tag_of(&i.target), "v1");
        assert!(!stage::marker_path(&i.cache).exists());
    }

    #[test]
    fn resume_starts_the_helper_for_a_valid_marker() {
        let i = install();
        assert_eq!(stage::resume_pending(&i.cache, &i.ctx), ResumeOutcome::Launched);
        // The helper is a copy of the "v1" fixture: it runs and writes its tag next to itself.
        let staged = stage::staging_dir(&i.cache);
        assert_eq!(read_output(&staged, Duration::from_secs(10)).as_deref(), Some("v1"));
        assert!(staged.join(stage::HELPER_STARTED).exists());
    }

    /// Holds the lock a running helper takes.
    fn hold_helper_lock(cache: &Path) -> fs::File {
        let path = stage::pending_dir(cache).join(stage::HELPER_LOCK);
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).unwrap();
        file.try_lock().unwrap();
        file
    }

    #[test]
    fn helper_does_nothing_while_another_helper_is_installing() {
        let i = install();
        let _held = hold_helper_lock(&i.cache);
        let (pid, reaper) = waited_process(10);
        reaper.join().unwrap();
        let args = HelperArgs {
            marker: stage::marker_path(&i.cache),
            wait_pid: pid,
            wait_timeout: Duration::from_secs(5),
        };
        assert_eq!(apply::run_helper(&args), EXIT_BUSY);
        assert_eq!(tag_of(&i.target), "v1");
        assert!(stage::marker_path(&i.cache).exists());
    }

    #[test]
    fn resume_waits_while_a_helper_is_installing() {
        let i = install();
        let _held = hold_helper_lock(&i.cache);
        assert_eq!(stage::resume_pending(&i.cache, &i.ctx), ResumeOutcome::InProgress);
        assert!(stage::marker_path(&i.cache).exists());
    }

    #[test]
    fn resume_gives_up_when_the_previous_helper_never_finished() {
        let i = install();
        fs::write(stage::staging_dir(&i.cache).join(stage::HELPER_STARTED), b"").unwrap();
        match stage::resume_pending(&i.cache, &i.ctx) {
            ResumeOutcome::Discarded(reason) => assert!(reason.contains("did not finish"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(!stage::marker_path(&i.cache).exists());
        assert_eq!(tag_of(&i.target), "v1");
    }

    #[test]
    fn resume_rejects_a_marker_prepared_for_another_program() {
        let i = install();
        let other =
            i.target.parent().unwrap().parent().unwrap().join("other").join(i.target.file_name().unwrap());
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::copy(&i.target, &other).unwrap();
        let ctx = ExecContext { current_exe: other, ..i.ctx.clone() };
        assert!(matches!(stage::resume_pending(&i.cache, &ctx), ResumeOutcome::Discarded(_)));
        assert!(!stage::marker_path(&i.cache).exists());
    }
}

#[cfg(target_os = "macos")]
mod bundle {
    use super::*;

    const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Launcher</string>
<key>CFBundleIdentifier</key><string>app.launcher.fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
"#;

    fn app_bundle(root: &Path, tag: &str) -> PathBuf {
        let app = root.join("Launcher.app");
        let contents = app.join("Contents");
        fs::create_dir_all(contents.join("MacOS")).unwrap();
        fs::create_dir_all(contents.join("Resources")).unwrap();
        let exe = contents.join("MacOS").join("Launcher");
        fs::copy(FIXTURE, &exe).unwrap();
        stage::set_mode(&exe, 0o755).unwrap();
        fs::write(contents.join("Resources").join("fixture-tag.txt"), tag).unwrap();
        fs::write(contents.join("Info.plist"), INFO_PLIST).unwrap();
        app
    }

    #[test]
    fn helper_installs_an_app_from_a_disk_image() {
        let tmp = tempfile::tempdir().unwrap();
        let target = app_bundle(&tmp.path().join("Іван & 100% Co"), "v1");
        let src = tmp.path().join("dmg-src");
        app_bundle(&src, "v2");
        let dmg = tmp.path().join("Launcher.dmg");
        let status = Command::new("hdiutil")
            .args(["create", "-volname", "Launcher", "-ov", "-format", "UDZO", "-srcfolder"])
            .arg(&src)
            .arg(&dmg)
            .status()
            .unwrap();
        assert!(status.success());
        let cache = tmp.path().join("cache");
        let exe = target.join("Contents").join("MacOS").join("Launcher");
        let ctx = ExecContext { os: Os::MacOs, current_exe: exe, appimage: None, pid: std::process::id() };
        let sha = sha256_file(&dmg).unwrap();
        stage::prepare(&cache, &ctx, &PathEnv::default(), &dmg, "0.2.0", &sha).unwrap();
        let (pid, reaper) = waited_process(500);
        let args = HelperArgs {
            marker: stage::marker_path(&cache),
            wait_pid: pid,
            wait_timeout: Duration::from_secs(20),
        };
        let code = apply::run_helper(&args);
        reaper.join().unwrap();
        assert_eq!(code, EXIT_OK);
        let tag =
            fs::read_to_string(target.join("Contents").join("Resources").join("fixture-tag.txt")).unwrap();
        assert_eq!(tag, "v2");
        let started = read_output(&target.join("Contents").join("MacOS"), Duration::from_secs(20));
        assert_eq!(started.as_deref(), Some("v2"));
    }
}
