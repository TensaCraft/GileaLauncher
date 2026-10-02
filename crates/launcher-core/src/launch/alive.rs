//! A game the launcher did not start, or started before it restarted, still shows: Minecraft
//! holds `saves/<world>/session.lock` locked while a world is open (1.16+), and on Windows its
//! log `logs/latest.log` cannot be shared for writing while any game since 1.7 runs.
//! The same signs name the game's process, so that a game the launcher lost can still be stopped.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// A game has the game folder `game` open: its log is being written (Windows) or a world of it is
/// open (its `session.lock` is locked). On Linux and macOS a game before 1.16, or one in its main
/// menu, shows neither: only the launcher's own registry knows it.
pub fn game_open(game: &Path) -> bool {
    if log_written(&game.join("logs").join("latest.log")) {
        return true;
    }
    let Ok(worlds) = fs::read_dir(game.join("saves")) else { return false };
    worlds.flatten().any(|world| held(&world.path().join("session.lock")))
}

/// The processes that have the game folder `game` open as a running game does: those with its log
/// open (Windows) and those holding a world's `session.lock` (Linux, macOS).
pub fn game_holders(game: &Path) -> Vec<u32> {
    let mut pids = log_holders(&game.join("logs").join("latest.log"));
    if let Ok(worlds) = fs::read_dir(game.join("saves")) {
        for world in worlds.flatten() {
            if let Ok(file) = File::open(world.path().join("session.lock"))
                && let Some(pid) = record_holder(&file)
                && pid != 0
                && !pids.contains(&pid)
            {
                pids.push(pid);
            }
        }
    }
    pids
}

/// The games running in the game folder `game`: the processes holding it that are Java.
pub fn game_processes(game: &Path) -> Vec<u32> {
    game_holders(game).into_iter().filter(|pid| image_of(*pid).is_some_and(|image| is_java(&image))).collect()
}

/// `image` is a Java runtime's executable (`java`, `javaw`), whichever system's path it is.
pub fn is_java(image: &Path) -> bool {
    let path = image.to_string_lossy();
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default().to_ascii_lowercase();
    matches!(name.strip_suffix(".exe").unwrap_or(&name), "java" | "javaw")
}

/// The processes that have `file` open, as the system's Restart Manager tells.
#[cfg(windows)]
fn log_holders(file: &Path) -> Vec<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
    };
    const ERROR_MORE_DATA: u32 = 234;
    if !file.is_file() {
        return Vec::new();
    }
    let wide: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: plain Win32 calls; every pointer is to a live buffer of the size given, and the
    // session is ended before returning.
    unsafe {
        if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
            return Vec::new();
        }
        let files = [wide.as_ptr()];
        let mut pids = Vec::new();
        if RmRegisterResources(session, 1, files.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
            let mut infos: Vec<RM_PROCESS_INFO> = vec![std::mem::zeroed(); 8];
            loop {
                let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
                let got = RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons);
                if got == ERROR_MORE_DATA && needed as usize > infos.len() {
                    infos = vec![std::mem::zeroed(); needed as usize];
                    continue;
                }
                if got == 0 {
                    pids = infos[..count as usize].iter().map(|i| i.Process.dwProcessId).collect();
                }
                break;
            }
        }
        RmEndSession(session);
        pids
    }
}

/// Elsewhere the log tells nothing.
#[cfg(not(windows))]
fn log_holders(_: &Path) -> Vec<u32> {
    Vec::new()
}

/// The executable process `pid` runs.
#[cfg(windows)]
fn image_of(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    // SAFETY: plain Win32 calls on a handle we own and close; `size` is the buffer's length.
    let got = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let got = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size) != 0;
        CloseHandle(handle);
        got
    };
    got.then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..size as usize])))
}

#[cfg(target_os = "linux")]
fn image_of(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
fn image_of(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the call writes at most `buffer.len()` bytes into `buffer`.
    let len =
        unsafe { libc::proc_pidpath(pid as libc::c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (len > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..len as usize])))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn image_of(_: u32) -> Option<PathBuf> {
    None
}

/// Another process has `file` open to write: opening it while sharing only reading fails then.
#[cfg(windows)]
fn log_written(file: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    match File::options().read(true).share_mode(FILE_SHARE_READ).open(file) {
        Ok(_) => false,
        Err(e) => sharing_violation(&e),
    }
}

/// Elsewhere nothing refuses to share a file.
#[cfg(not(windows))]
fn log_written(_: &Path) -> bool {
    false
}

/// Someone holds a lock on `file`. Taking a shared lock ourselves for a moment is how to ask; the
/// file is only read.
fn held(file: &Path) -> bool {
    let opened = match File::open(file) {
        Ok(opened) => opened,
        // Opened by a game that shares it with no one.
        Err(e) => return sharing_violation(&e),
    };
    if matches!(opened.try_lock_shared(), Err(TryLockError::WouldBlock)) {
        return true;
    }
    record_holder(&opened).is_some()
}

#[cfg(windows)]
fn sharing_violation(e: &io::Error) -> bool {
    e.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32)
}

#[cfg(not(windows))]
fn sharing_violation(_: &io::Error) -> bool {
    false
}

/// Java locks with `fcntl` record locks, which the `flock` above does not see on Linux; asking
/// for a read lock shows a write lock someone holds, and whose it is (0: an open-file-description
/// lock, which has no owner).
#[cfg(unix)]
// The lock constants are `c_int` on Linux and `c_short` on macOS: the casts are needed on one.
#[allow(clippy::unnecessary_cast)]
fn record_holder(file: &File) -> Option<u32> {
    use std::os::fd::AsRawFd;
    // SAFETY: a plain C struct; zero is a valid value for each of its fields.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_RDLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    // An open-file-description query sees locks this process holds too (Linux only).
    #[cfg(target_os = "linux")]
    let query = libc::F_OFD_GETLK;
    #[cfg(not(target_os = "linux"))]
    let query = libc::F_GETLK;
    // SAFETY: `file` is open for the call and `lock` is a valid `flock` it may write to.
    let answered = unsafe { libc::fcntl(file.as_raw_fd(), query, &mut lock) } == 0;
    (answered && i32::from(lock.l_type) != libc::F_UNLCK as i32)
        .then(|| u32::try_from(lock.l_pid).unwrap_or(0))
}

/// On Windows the shared lock above already meets the lock Java takes.
#[cfg(not(unix))]
fn record_holder(_: &File) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};

    use super::*;

    fn lock_file(game: &Path, world: &str) -> std::path::PathBuf {
        let dir = game.join("saves").join(world);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.lock");
        fs::write(&path, "\u{2603}").unwrap();
        path
    }

    #[test]
    fn a_locked_session_lock_is_an_open_world() {
        let tmp = tempfile::tempdir().unwrap();
        lock_file(tmp.path(), "Closed");
        let held = File::options().read(true).write(true).open(lock_file(tmp.path(), "Open")).unwrap();
        held.lock().unwrap();
        assert!(game_open(tmp.path()));
        held.unlock().unwrap();
        assert!(!game_open(tmp.path()), "a world whose lock is free is closed");
    }

    #[test]
    fn no_saves_or_no_lock_is_no_open_world() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!game_open(tmp.path()));
        fs::create_dir_all(tmp.path().join("saves/New")).unwrap();
        assert!(!game_open(tmp.path()));
        lock_file(tmp.path(), "Old");
        assert!(!game_open(tmp.path()), "an old game's lock file left behind is no open world");
    }

    /// Any game since 1.7 keeps its log open to write from start to exit, main menu included.
    #[cfg(windows)]
    #[test]
    fn a_game_writing_its_log_is_open() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("logs")).unwrap();
        let log = File::create(tmp.path().join("logs/latest.log")).unwrap();
        assert!(game_open(tmp.path()), "a 1.12.2 game in its menu");
        drop(log);
        assert!(!game_open(tmp.path()), "a log left behind is no game");
    }

    /// The process writing a game's log is found: the game to stop.
    #[cfg(windows)]
    #[test]
    fn the_process_writing_the_log_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(game_holders(tmp.path()).is_empty(), "no log");
        fs::create_dir_all(tmp.path().join("logs")).unwrap();
        let log = File::create(tmp.path().join("logs/latest.log")).unwrap();
        assert_eq!(game_holders(tmp.path()), [std::process::id()]);
        drop(log);
        assert!(game_holders(tmp.path()).is_empty(), "a log left behind");
    }

    /// Only a Java runtime is taken for the game.
    #[test]
    fn only_java_is_a_game() {
        for (path, java) in [
            (r"C:\Games\runtime\bin\javaw.exe", true),
            (r"C:\Games\runtime\bin\JAVA.EXE", true),
            ("/usr/lib/jvm/bin/java", true),
            (r"C:\Program Files\Notepad++\notepad++.exe", false),
            (r"C:\Tools\javascript.exe", false),
            ("/usr/bin/tail", false),
        ] {
            assert_eq!(is_java(Path::new(path)), java, "{path}");
        }
    }

    /// Java locks with `fcntl` record locks, which `flock` does not see on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_world_locked_as_java_locks_it_is_open() {
        use std::os::fd::AsRawFd;
        let tmp = tempfile::tempdir().unwrap();
        let held = File::options().read(true).write(true).open(lock_file(tmp.path(), "W")).unwrap();
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        // Closing any descriptor of the file drops this process's own record locks, so each
        // question below closes the lock this test holds; a game holds its lock in its own process.
        let lock_it = || assert_eq!(unsafe { libc::fcntl(held.as_raw_fd(), libc::F_SETLK, &lock) }, 0);
        lock_it();
        assert!(game_open(tmp.path()));
        lock_it();
        assert_eq!(game_holders(tmp.path()), [std::process::id()], "the lock names its owner");
    }
}
