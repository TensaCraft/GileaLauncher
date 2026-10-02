//! Old build caches. Cargo keeps a separate set of artifacts for every feature set, profile, crate
//! version and toolchain, and never removes the ones no longer built: in a few weeks `target/` grew
//! to hundreds of gigabytes. The sweep removes the cache entries not written for a while.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The cache folders of a profile folder (`target/debug`, `target/<triple>/release`, …).
const CACHES: [&str; 4] = ["incremental", "deps", "build", ".fingerprint"];
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an artifact stays unwritten before it goes: a dependency built once and used since is
/// rebuilt after this (a few minutes, once in a while).
const KEEP: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// Incremental caches serve only the latest build of a crate's configuration.
const KEEP_INCREMENTAL: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// When the target folder was last swept.
const STAMP: &str = ".sweep-stamp";
/// How deep profile folders lie (`target/hooks/<triple>/debug`).
const DEPTH: usize = 4;

/// What the sweep removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub entries: usize,
    pub bytes: u64,
}

/// Removes, from every cache folder under `target`, the entries last written before `now - keep`
/// (`keep_incremental` for `incremental`, which only ever serves the latest build of a crate).
/// An entry is a file, or a folder judged by the newest file in it.
pub fn sweep(target: &Path, keep: Duration, keep_incremental: Duration, now: SystemTime) -> Swept {
    let mut swept = Swept::default();
    visit(target, 0, &mut |cache, name| {
        let keep = if name == "incremental" { keep_incremental } else { keep };
        if let Some(cutoff) = now.checked_sub(keep) {
            sweep_cache(cache, cutoff, &mut swept);
        }
    });
    swept
}

/// Calls `found` with every cache folder under `dir` and its name.
fn visit(dir: &Path, depth: usize, found: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        match entry.file_name().to_str() {
            Some(name) if CACHES.contains(&name) => found(&path, name),
            _ if depth < DEPTH => visit(&path, depth + 1, found),
            _ => {}
        }
    }
}

fn sweep_cache(cache: &Path, cutoff: SystemTime, swept: &mut Swept) {
    let Ok(entries) = fs::read_dir(cache) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if newest(&path).is_none_or(|written| written >= cutoff) {
            continue;
        }
        let size = size(&path);
        let removed = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        // A file in use (another build runs) stays for the next sweep.
        if removed.is_ok() {
            swept.entries += 1;
            swept.bytes += size;
        }
    }
}

/// When `path` was last written: a file's own time, a folder's newest file.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return meta.modified().ok();
    }
    let inside = fs::read_dir(path).ok()?.flatten().filter_map(|e| newest(&e.path())).max();
    inside.or_else(|| meta.modified().ok())
}

fn size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path).map_or(0, |entries| entries.flatten().map(|e| size(&e.path())).sum())
}

/// The folder Cargo builds into.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| crate::cmd::root().join("target"), PathBuf::from)
}

/// Sweeps `target` at most once a day, before a command that builds; says what it removed. Never in
/// CI: a cache restored there keeps its files' old times, and the sweep would empty it every run.
pub fn daily(target: &Path) {
    if !in_ci(|key| std::env::var(key).ok()) {
        once_a_day(target);
    }
}

/// Whether this runs in CI (`CI` or `GITHUB_ACTIONS` set, as GitHub Actions sets them).
fn in_ci(var: impl Fn(&str) -> Option<String>) -> bool {
    ["CI", "GITHUB_ACTIONS"].iter().any(|key| var(key).is_some_and(|v| !v.is_empty() && v != "false"))
}

fn once_a_day(target: &Path) {
    let stamp = target.join(STAMP);
    let now = SystemTime::now();
    let fresh = fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age < DAY));
    if fresh || !target.is_dir() {
        return;
    }
    let swept = sweep(target, KEEP, KEEP_INCREMENTAL, now);
    let _ = fs::write(&stamp, b"");
    if swept.entries > 0 {
        println!(
            "Removed {} old build caches ({:.1} GB) from {}",
            swept.entries,
            swept.bytes as f64 / 1e9,
            target.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::PathBuf;

    use super::*;

    /// Writes `path` (and its folders) as last written `days` ago.
    fn file(path: PathBuf, days: u64, now: SystemTime) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"artifact").unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(now - DAY * days as u32).unwrap();
        path
    }

    #[test]
    fn old_caches_go_and_recent_ones_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let now = SystemTime::now();
        let old_session = file(t.join("debug/incremental/old_core-1abc/s-1/query.bin"), 5, now);
        let new_session = file(t.join("debug/incremental/launcher_core-2def/s-2/query.bin"), 1, now);
        let old_test = file(t.join("debug/deps/launcher_core-1111.exe"), 20, now);
        let old_rlib = file(t.join("debug/deps/libserde-2222.rlib"), 20, now);
        let new_rlib = file(t.join("debug/deps/libserde-3333.rlib"), 2, now);
        // A folder with one recent file stays whole.
        let mixed_old = file(t.join("debug/build/ring-4444/out/old.o"), 30, now);
        let mixed_new = file(t.join("debug/build/ring-4444/output"), 1, now);
        let old_print = file(t.join("debug/.fingerprint/old_app-5555/lib-old_app"), 30, now);
        // Every profile folder: other targets, release, the hooks' own target folder.
        let wasm = file(t.join("wasm32-unknown-unknown/debug/deps/launcher_ui-6666.wasm"), 20, now);
        let hooks = file(t.join("hooks/debug/incremental/xtask-7777/s-3/dep-graph.bin"), 9, now);
        // Outside the caches nothing is touched, however old.
        let app = file(t.join("debug/launcher_app.exe"), 60, now);
        let bundle = file(t.join("release/bundle/nsis/Launcher-Setup.exe"), 60, now);

        let swept = sweep(t, DAY * 14, DAY * 3, now);

        for gone in [&old_session, &old_test, &old_rlib, &old_print, &wasm, &hooks] {
            assert!(!gone.exists(), "{} goes", gone.display());
        }
        assert!(!t.join("debug/incremental/old_core-1abc").exists(), "the whole crate folder goes");
        for kept in [&new_session, &new_rlib, &mixed_old, &mixed_new, &app, &bundle] {
            assert!(kept.exists(), "{} stays", kept.display());
        }
        assert_eq!(swept.entries, 6);
        assert_eq!(swept.bytes, 6 * b"artifact".len() as u64);
    }

    #[test]
    fn the_sweep_runs_once_a_day() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        once_a_day(tmp.path());
        let old = file(tmp.path().join("debug/deps/libold-1.rlib"), 40, now);
        once_a_day(tmp.path());
        assert!(old.exists(), "swept today already");
        File::options()
            .write(true)
            .open(tmp.path().join(STAMP))
            .unwrap()
            .set_modified(now - DAY * 2)
            .unwrap();
        once_a_day(tmp.path());
        assert!(!old.exists(), "a day later the sweep runs again");
    }

    #[test]
    fn a_ci_run_never_sweeps_the_cache_it_restored() {
        // A restored cache keeps its files' old times: the sweep would empty it on every run.
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
        };
        assert!(in_ci(env(&[("CI", "true")])));
        assert!(in_ci(env(&[("GITHUB_ACTIONS", "true")])));
        assert!(!in_ci(env(&[])));
        assert!(!in_ci(env(&[("CI", "false")])));
    }

    #[test]
    fn no_target_folder_is_nothing_to_sweep() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(sweep(&tmp.path().join("target"), DAY, DAY, SystemTime::now()), Swept::default());
    }
}
