//! The launcher's own crashes (Rust panics): each is kept in a small file under the cache folder
//! (`crashes/`) and offered as a report at the next start. Nothing is ever sent by itself.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use launcher_shared::branding::VERSION;
use serde::{Deserialize, Serialize};

/// The folder of the kept crashes, in the cache folder.
pub const CRASH_DIR: &str = "crashes";
/// How many crashes of one place in the code a run keeps (one stuck in a loop writes no more).
const PER_PLACE: u32 = 3;
/// How many crashes wait for a start, and how long.
const KEPT: usize = 5;
const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A crash as kept: its text and place in the code (the home folder and tokens taken out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crash {
    pub version: String,
    /// When it happened (RFC 3339).
    pub at: String,
    pub message: String,
    pub location: String,
    pub thread: String,
}

impl Crash {
    /// A crash of this launcher now, `home` taken out of its text.
    pub fn now(message: &str, location: &str, thread: &str, home: Option<&str>) -> Crash {
        let clean = |text: &str| crate::logging::redact(text, home);
        Crash {
            version: VERSION.to_string(),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            message: clean(message),
            location: clean(location),
            thread: thread.to_string(),
        }
    }
}

/// Keeps `crash` in `dir`.
pub fn write(dir: &Path, crash: &Crash) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    let path = dir.join(format!("crash-{millis}-{}.json", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, serde_json::to_vec(crash).map_err(io::Error::other)?)?;
    Ok(path)
}

/// The kept crash files, oldest first.
fn files(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let mut kept: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
                .collect()
        })
        .unwrap_or_default();
    kept.sort();
    kept
}

/// The newest crash waiting to be reported. Crashes older than a month, all but the newest few
/// and files that cannot be read go.
pub fn pending(dir: &Path) -> Option<Crash> {
    let now = SystemTime::now();
    let mut kept: Vec<(PathBuf, Crash)> = Vec::new();
    for (at, path) in files(dir) {
        let old = now.duration_since(at).is_ok_and(|age| age > KEPT_FOR);
        match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Crash>(&b).ok()) {
            Some(crash) if !old => kept.push((path, crash)),
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    let too_many = kept.len().saturating_sub(KEPT);
    for (path, _) in kept.drain(..too_many) {
        let _ = std::fs::remove_file(path);
    }
    kept.pop().map(|(_, crash)| crash)
}

/// Forgets every kept crash (reported, or the user chose not to).
pub fn clear(dir: &Path) {
    for (_, path) in files(dir) {
        let _ = std::fs::remove_file(path);
    }
}

/// The crashes kept so far this run, by place.
#[derive(Debug, Default)]
pub struct PlaceLimit {
    seen: HashMap<String, u32>,
}

impl PlaceLimit {
    /// One more crash at `place` may be kept.
    pub fn allow(&mut self, place: &str) -> bool {
        let count = self.seen.entry(place.to_string()).or_default();
        *count += 1;
        *count <= PER_PLACE
    }
}

/// Keeps every crash of the launcher in `dir` from now on (once; later calls do nothing), then
/// lets the panic go on as before.
pub fn install_hook(dir: PathBuf, home: Option<String>) {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let before = std::panic::take_hook();
    let limit = Mutex::new(PlaceLimit::default());
    std::panic::set_hook(Box::new(move |info| {
        let place =
            info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        // A crash inside the hook's own lock is not kept rather than waited for.
        if limit.try_lock().is_ok_and(|mut l| l.allow(&place)) {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let _ = write(&dir, &Crash::now(&message, &place, &thread, home.as_deref()));
        }
        before(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crash(message: &str) -> Crash {
        Crash::now(message, "crates/x/src/a.rs:1:1", "main", None)
    }

    #[test]
    fn a_kept_crash_waits_for_the_next_start_without_the_home_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(pending(dir.path()), None);
        let home = r"C:\Users\Olena";
        let kept = Crash::now(r"cannot read C:\Users\Olena\x.json", "src/a.rs:1:1", "main", Some(home));
        write(dir.path(), &kept).unwrap();
        let found = pending(dir.path()).unwrap();
        assert!(!found.message.contains("Olena"), "{}", found.message);
        assert_eq!((found.version.as_str(), found.thread.as_str()), (VERSION, "main"));
        assert_eq!(pending(dir.path()), Some(found), "it waits until reported or dismissed");
        clear(dir.path());
        assert_eq!(pending(dir.path()), None);
    }

    #[test]
    fn crashes_are_kept_few_and_not_for_long() {
        let dir = tempfile::tempdir().unwrap();
        let mut limit = PlaceLimit::default();
        assert!((0..3).all(|_| limit.allow("src/x.rs:1:1")));
        assert!(!limit.allow("src/x.rs:1:1"), "the same place again and again keeps three");
        assert!(limit.allow("src/y.rs:2:2"));
        for n in 0..8 {
            let path = write(dir.path(), &crash(&format!("crash {n}"))).unwrap();
            let age = SystemTime::now() - Duration::from_secs(60 * 60 * (8 - n));
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(age).unwrap();
        }
        let old = write(dir.path(), &crash("old")).unwrap();
        let month = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 31);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(month).unwrap();
        std::fs::write(dir.path().join("broken.json"), "{").unwrap();
        assert_eq!(pending(dir.path()).map(|c| c.message), Some("crash 7".to_string()), "the newest");
        assert_eq!(files(dir.path()).len(), KEPT, "the five newest stay, the rest go");
        assert!(!old.exists() && !dir.path().join("broken.json").exists());
    }
}
