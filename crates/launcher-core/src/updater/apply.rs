//! `--apply-update` helper mode. Runs from the staged copy of the launcher after the
//! launcher itself has exited: swaps the program, rolls back on failure, starts the result.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::stage::{self, Marker};
use crate::paths::Os;

pub const EXIT_OK: i32 = 0;
pub const EXIT_INVALID: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_FAILED: i32 = 4;
/// Another helper is already installing this update.
pub const EXIT_BUSY: i32 = 5;
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Antivirus scanners and Explorer briefly lock freshly written executables on Windows.
const RENAME_ATTEMPTS: u32 = if cfg!(windows) { 30 } else { 3 };
const RENAME_PAUSE: Duration = Duration::from_secs(1);

pub struct HelperArgs {
    pub marker: PathBuf,
    pub wait_pid: u32,
    pub wait_timeout: Duration,
}

struct HelperLog(Option<fs::File>);

impl HelperLog {
    fn open(path: &Path) -> Self {
        Self(OpenOptions::new().create(true).append(true).open(path).ok())
    }

    fn line(&mut self, message: impl AsRef<str>) {
        if let Some(file) = &mut self.0 {
            let _ =
                writeln!(file, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), message.as_ref());
        }
    }
}

/// No console, no inherited handles, not killed together with the parent.
pub fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// Starts the (updated) launcher; `.app` bundles go through `open`.
pub fn relaunch(target: &Path) -> io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut open = Command::new("open");
        open.arg("-n").arg(target);
        open
    } else {
        Command::new(target)
    };
    if let Some(dir) = target.parent() {
        cmd.current_dir(dir);
    }
    detach(&mut cmd);
    cmd.spawn().map(|_| ())
}

#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain Win32 calls on a handle we own and close.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true; // no such process: it has already exited
        }
        let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        let result = WaitForSingleObject(handle, ms);
        CloseHandle(handle);
        result == WAIT_OBJECT_0
    }
}

#[cfg(unix)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // SAFETY: signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn retry<T>(attempts: u32, pause: Duration, mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let attempts = attempts.max(1);
    let mut last = None;
    for attempt in 1..=attempts {
        match op() {
            Ok(value) => return Ok(value),
            Err(e) => {
                last = Some(e);
                if attempt < attempts {
                    std::thread::sleep(pause);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no attempts")))
}

/// `target` → `bak`, `new` → `target`; if the second move fails the original is put back.
pub fn swap_in(
    target: &Path,
    new: &Path,
    bak: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    rename(target, bak)?;
    if let Err(e) = rename(new, target) {
        let _ = rename(bak, target);
        return Err(e);
    }
    Ok(())
}

fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    target.with_file_name(name)
}

fn remove_any(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

#[cfg(not(target_os = "macos"))]
fn stage_new_copy(source: &Path, new: &Path) -> io::Result<()> {
    fs::copy(source, new)?;
    stage::set_mode(new, 0o755)
}

#[cfg(target_os = "macos")]
fn stage_new_copy(dmg: &Path, new_app: &Path) -> io::Result<()> {
    fn run(cmd: &mut Command) -> io::Result<()> {
        let status = cmd.status()?;
        if status.success() { Ok(()) } else { Err(io::Error::other(format!("{cmd:?} failed: {status}"))) }
    }
    let mount = dmg.with_extension("mnt");
    remove_any(&mount);
    fs::create_dir_all(&mount)?;
    run(Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(dmg))?;
    let copied = (|| {
        let app = fs::read_dir(&mount)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| io::Error::other("the disk image contains no .app"))?;
        run(Command::new("ditto").arg(&app).arg(new_app))
    })();
    let _ = run(Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount));
    let _ = fs::remove_dir(&mount);
    copied
}

fn install(marker: &Marker) -> io::Result<()> {
    let target = &marker.target;
    let new = sibling(target, "new");
    let bak = sibling(target, "bak");
    remove_any(&new);
    remove_any(&bak);
    stage_new_copy(&marker.source, &new)?;
    let mut rename = |from: &Path, to: &Path| retry(RENAME_ATTEMPTS, RENAME_PAUSE, || fs::rename(from, to));
    swap_in(target, &new, &bak, &mut rename).inspect_err(|_| remove_any(&new))
}

/// Entry point of `launcher-app --apply-update <marker> --wait-pid <pid>`.
/// `Err` while another helper holds the lock; `Ok(None)` when locking is not possible here.
fn lock_helper(path: &Path) -> Result<Option<fs::File>, ()> {
    let Ok(file) = OpenOptions::new().create(true).truncate(false).write(true).open(path) else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Err(()),
        Err(fs::TryLockError::Error(_)) => Ok(None),
    }
}

pub fn run_helper(args: &HelperArgs) -> i32 {
    let Some(pending) = args.marker.parent() else { return EXIT_INVALID };
    let Some(cache) = pending.parent() else { return EXIT_INVALID };
    let mut log = HelperLog::open(&pending.join(stage::APPLY_LOG));
    log.line(format!("apply-update: marker={} wait_pid={}", args.marker.display(), args.wait_pid));
    if args.wait_pid == 0 || args.marker != stage::marker_path(cache) {
        log.line("rejected: bad arguments");
        return EXIT_INVALID;
    }
    // Held until this process ends: a second helper (a double click, a second launcher start)
    // must not swap the same files concurrently.
    let Ok(_lock) = lock_helper(&stage::helper_lock_path(cache)) else {
        log.line("another helper is already installing this update");
        return EXIT_BUSY;
    };
    let marker = match stage::validate_marker(cache, Os::current(), None) {
        Ok(marker) => marker,
        Err(e) => {
            log.line(format!("rejected: {}", e.detail));
            let _ = fs::remove_file(&args.marker);
            return EXIT_INVALID;
        }
    };
    if !wait_for_exit(args.wait_pid, args.wait_timeout) {
        log.line("the launcher did not exit in time; the update stays pending");
        let _ = fs::remove_file(stage::staging_dir(cache).join(stage::HELPER_STARTED));
        return EXIT_TIMEOUT;
    }
    let code = match install(&marker) {
        Ok(()) => {
            let _ = fs::remove_file(&args.marker);
            remove_any(&sibling(&marker.target, "bak"));
            let _ = fs::remove_file(&marker.source);
            log.line(format!("installed {} into {}", marker.version, marker.target.display()));
            EXIT_OK
        }
        Err(e) => {
            let _ = fs::remove_file(&args.marker);
            log.line(format!("install failed, the previous version was kept: {e}"));
            EXIT_FAILED
        }
    };
    if let Err(e) = relaunch(&marker.target) {
        log.line(format!("relaunch failed: {e}"));
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;

    fn trio(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (dir.join("app"), dir.join("app.new"), dir.join("app.bak"))
    }

    #[test]
    fn swap_moves_the_new_file_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let (target, new, bak) = trio(dir.path());
        fs::write(&target, "v1").unwrap();
        fs::write(&new, "v2").unwrap();
        swap_in(&target, &new, &bak, &mut |a, b| fs::rename(a, b)).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "v2");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "v1");
        assert!(!new.exists());
    }

    #[test]
    fn swap_rolls_back_when_the_new_file_cannot_be_moved() {
        let dir = tempfile::tempdir().unwrap();
        let (target, new, bak) = trio(dir.path());
        fs::write(&target, "v1").unwrap();
        fs::write(&new, "v2").unwrap();
        let blocked = new.clone();
        let err = swap_in(&target, &new, &bak, &mut |a, b| {
            if a == blocked { Err(io::Error::other("locked by antivirus")) } else { fs::rename(a, b) }
        });
        assert!(err.is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "v1");
        assert!(!bak.exists());
    }

    #[test]
    fn retry_recovers_from_transient_failures() {
        let calls = Cell::new(0);
        let value = retry(5, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 { Err(io::Error::other("busy")) } else { Ok(7) }
        });
        assert_eq!(value.unwrap(), 7);
        assert_eq!(calls.get(), 3);
        calls.set(0);
        let failed = retry(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::Error::other("busy"))
        });
        assert!(failed.is_err());
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn a_finished_process_counts_as_exited() {
        let mut child = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 0"]).spawn()
        } else {
            Command::new("true").spawn()
        }
        .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let start = Instant::now();
        assert!(wait_for_exit(pid, Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn bad_arguments_are_rejected_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cache").join(stage::PENDING_DIR).join(stage::MARKER_FILE);
        let start = Instant::now();
        let zero = HelperArgs { marker: marker.clone(), wait_pid: 0, wait_timeout: Duration::from_secs(30) };
        assert_eq!(run_helper(&zero), EXIT_INVALID);
        let missing =
            HelperArgs { marker, wait_pid: std::process::id(), wait_timeout: Duration::from_secs(30) };
        assert_eq!(run_helper(&missing), EXIT_INVALID);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
