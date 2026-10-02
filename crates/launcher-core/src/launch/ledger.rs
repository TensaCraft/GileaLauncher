//! The games the launcher started, kept in a file of the state folder so that a launcher which
//! restarts (an update, the setup wizard, a crash) still knows them: each by its process id and
//! the time its process started, which a reused process id does not share.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::storage::atomic::atomic_write;

/// The ledger's file in the state folder.
pub const LEDGER_FILE: &str = "running-games.json";

/// A game the launcher started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Its game folder as the launch registry spells it.
    pub key: String,
    pub build_key: String,
    pub build_name: String,
    pub pid: u32,
    /// When its process started, as the system tells it (`process_started`).
    pub started: String,
}

/// The ledger file, read and written whole under a lock.
pub struct Ledger {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Ledger {
    pub fn new(path: PathBuf) -> Ledger {
        Ledger { path, lock: Mutex::new(()) }
    }

    /// The games in it (none when the file is missing or unreadable).
    pub fn entries(&self) -> Vec<Entry> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read()
    }

    fn read(&self) -> Vec<Entry> {
        fs::read(&self.path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    fn write(&self, entries: &[Entry]) -> io::Result<()> {
        if entries.is_empty() {
            return match fs::remove_file(&self.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let json = serde_json::to_vec_pretty(entries).map_err(io::Error::other)?;
        atomic_write(&self.path, &json)
    }

    /// Adds a game just started.
    pub fn add(&self, entry: Entry) -> io::Result<()> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.read();
        entries.retain(|e| e.pid != entry.pid);
        entries.push(entry);
        self.write(&entries)
    }

    /// Keeps only the games `keep` accepts.
    pub fn retain(&self, keep: impl Fn(&Entry) -> bool) -> io::Result<()> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.read();
        let before = entries.len();
        entries.retain(|e| keep(e));
        if entries.len() == before { Ok(()) } else { self.write(&entries) }
    }

    /// Drops the game with process `pid` (it ended).
    pub fn remove(&self, pid: u32) -> io::Result<()> {
        self.retain(|e| e.pid != pid)
    }
}

/// When process `pid` started, as the system tells it; `None` when there is no such running
/// process (gone, or ended and not yet reaped).
#[cfg(windows)]
pub fn process_started(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    /// `GetExitCodeProcess` of a process that has not ended.
    const STILL_ACTIVE: u32 = 259;
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    let mut code = 0u32;
    // SAFETY: plain Win32 calls on a handle we own and close; the out-parameters are ours.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE;
        CloseHandle(handle);
        (times && running).then(|| {
            ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)).to_string()
        })
    }
}

/// Linux: field 22 of `/proc/<pid>/stat` (clock ticks after boot); a zombie has ended.
#[cfg(target_os = "linux")]
pub fn process_started(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses: the fields follow its last ')'.
    let fields: Vec<&str> = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().collect();
    let (state, started) = (fields.first()?, fields.get(19)?);
    (*state != "Z" && *state != "X").then(|| started.to_string())
}

/// macOS: the process's BSD info (start time to the microsecond); a zombie has ended.
#[cfg(target_os = "macos")]
pub fn process_started(pid: u32) -> Option<String> {
    /// `proc_bsdinfo::pbi_status` of a zombie.
    const SZOMB: u32 = 5;
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: a plain C struct for which zero is valid.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the call writes at most `size` bytes into `info`, which has exactly that size.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (got == size && info.pbi_status != SZOMB)
        .then(|| format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

/// Other Unix systems: `ps` tells the state and the start time, in a fixed locale and zone.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub fn process_started(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .env("TZ", "UTC0")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (state, started) = text.split_once(char::is_whitespace)?;
    (out.status.success() && !state.starts_with('Z')).then(|| started.trim().to_string())
}

/// Stops process `pid` at once.
#[cfg(windows)]
pub fn kill_process(pid: u32) -> io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: plain Win32 calls on a handle we own and close.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let done = TerminateProcess(handle, 1) != 0;
        let error = io::Error::last_os_error();
        CloseHandle(handle);
        if done { Ok(()) } else { Err(error) }
    }
}

/// Stops process `pid` at once.
#[cfg(unix)]
pub fn kill_process(pid: u32) -> io::Result<()> {
    // SAFETY: a plain signal to a process id.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A game a launcher before this one started: known by its process id and start time only.
pub struct AdoptedProcess {
    pub pid: u32,
    pub started: String,
}

impl AdoptedProcess {
    fn alive(&self) -> bool {
        process_started(self.pid).as_deref() == Some(self.started.as_str())
    }
}

impl super::process::GameProcess for AdoptedProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    /// Its exit code is not the launcher's to know.
    fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
        Ok((!self.alive()).then_some(None))
    }

    fn kill(&mut self) -> io::Result<()> {
        // Another process may have the id by now.
        if self.alive() { kill_process(self.pid) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_start_times_are_stable() {
        let me = std::process::id();
        let started = process_started(me).expect("this process runs");
        assert_eq!(process_started(me).as_deref(), Some(started.as_str()), "the same every time");
        assert_eq!(process_started(u32::MAX - 7), None, "no such process");
    }

    #[test]
    fn a_ledger_keeps_the_games_it_is_told_of() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(dir.path().join(LEDGER_FILE));
        let game = |pid| Entry {
            key: format!("k{pid}"),
            build_key: "aero".into(),
            build_name: "Aero".into(),
            pid,
            started: "1".into(),
        };
        ledger.add(game(1)).unwrap();
        ledger.add(game(2)).unwrap();
        assert_eq!(Ledger::new(dir.path().join(LEDGER_FILE)).entries(), [game(1), game(2)], "on disk");
        ledger.remove(1).unwrap();
        assert_eq!(ledger.entries(), [game(2)]);
        ledger.remove(2).unwrap();
        assert!(!dir.path().join(LEDGER_FILE).exists(), "nothing left, no file");
        std::fs::write(dir.path().join(LEDGER_FILE), "{not json").unwrap();
        assert!(ledger.entries().is_empty());
    }
}
