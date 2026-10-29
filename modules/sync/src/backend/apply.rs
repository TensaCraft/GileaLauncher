//! A pass's decisions written into its builds: each build through the core's file transaction,
//! what it overwrites or removes held until the pass ends (no copy is kept after), and the builds
//! already written put back as they were when a later one fails.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use launcher_core::storage::journal::JournalName;
use launcher_core::storage::transaction::{ApplyHooks, TransactionPlan, execute};
use launcher_shared::{AppError, AppResult, ErrorCode};

use super::decide::{Held, Outcome, Remove, Write};
use super::scan::modified_ms;
use super::store::Seen;

/// The journal of a sync pass's change in a build.
pub const SYNC_JOURNAL: JournalName =
    JournalName { file: "build-sync.json", legacy: ".launcher-build-sync.json" };

/// Where a member holds, until its pass ends, the files the pass overwrites or removes there.
pub const UNDO_DIR: &str = ".launcher/sync-undo";

/// A build taking part in a pass.
pub struct Member {
    pub key: String,
    pub game: PathBuf,
}

/// What a pass wrote: each written file as it is now, by build and key.
#[derive(Debug, Default)]
pub struct Applied {
    pub seen: BTreeMap<(String, String), Seen>,
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

/// Copies `from` to `to` (its folder made) with `from`'s modified time.
fn copy_dated(from: &Path, to: &Path) -> AppResult<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    fs::copy(from, to).map_err(|e| io_error(from, e))?;
    let time = fs::metadata(from).and_then(|m| m.modified()).map_err(|e| io_error(from, e))?;
    fs::File::options().write(true).open(to).and_then(|f| f.set_modified(time)).map_err(|e| io_error(to, e))
}

/// Holds `held` in the member's empty undo folder: each a second name for the file (the swap
/// moves the original aside and never writes into it), or a copy where links cannot be made.
fn hold(member: &Member, held: &[&Held]) -> AppResult<PathBuf> {
    let folder = member.game.join(UNDO_DIR);
    let _ = fs::remove_dir_all(&folder);
    for file in held {
        let (from, to) = (member.game.join(&file.path), folder.join(&file.path));
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        if fs::hard_link(&from, &to).is_err() {
            copy_dated(&from, &to)?;
        }
    }
    Ok(folder)
}

/// A member written, kept to be put back when a later member fails.
struct Done<'a> {
    member: &'a Member,
    /// Its undo folder, when the pass overwrote or removed a file there.
    folder: Option<PathBuf>,
    writes: Vec<&'a Write>,
    removes: Vec<&'a Remove>,
    held: Vec<&'a Held>,
}

/// Puts the members in `done` back as they were (best effort): a file it overwrote or removed
/// from its undo folder, a file it added removed.
fn undo(done: &[Done<'_>]) {
    for d in done.iter().rev() {
        let copy = |key: &str| {
            let file = d.held.iter().find(|h| h.key == key)?;
            Some((d.folder.as_ref()?.join(&file.path), d.member.game.join(&file.path)))
        };
        for write in &d.writes {
            let target = d.member.game.join(&write.path);
            let result = if d.held.iter().any(|h| h.key == write.key) {
                let _ = fs::remove_file(&target);
                copy(&write.key).map_or(Ok(()), |(from, to)| copy_dated(&from, &to))
            } else {
                fs::remove_file(&target).map_err(|e| io_error(&target, e))
            };
            if let Err(e) = result {
                tracing::warn!("Unable to put {} back in {}: {}", write.path, d.member.key, e.detail);
            }
        }
        for removed in &d.removes {
            if let Some((from, to)) = copy(&removed.key)
                && let Err(e) = copy_dated(&from, &to)
            {
                tracing::warn!("Unable to put {} back in {}: {}", removed.path, d.member.key, e.detail);
            }
        }
    }
}

/// Removes the members' undo folders once the pass is over.
fn release(members: &[Member]) {
    for member in members {
        let folder = member.game.join(UNDO_DIR);
        match fs::remove_dir_all(&folder) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                tracing::warn!("Unable to remove {}: {e}", folder.display());
            }
            _ => {}
        }
    }
}

/// Writes `outcome` into `members`, one after another, each through its own transaction; what a
/// member's change overwrites or removes is held until the pass ends. When a member fails, the
/// ones before are put back and the error comes back.
pub fn apply(members: &[Member], outcome: &Outcome) -> AppResult<Applied> {
    let result = write_all(members, outcome);
    release(members);
    result
}

fn write_all(members: &[Member], outcome: &Outcome) -> AppResult<Applied> {
    let mut applied = Applied::default();
    let mut done: Vec<Done<'_>> = Vec::new();
    for member in members {
        let writes: Vec<&Write> = outcome.writes.iter().filter(|w| w.build == member.key).collect();
        let removes: Vec<&Remove> = outcome.removes.iter().filter(|r| r.build == member.key).collect();
        if writes.is_empty() && removes.is_empty() {
            continue;
        }
        let held: Vec<&Held> = outcome.held.iter().filter(|h| h.build == member.key).collect();
        let source_of = |write: &Write| -> AppResult<PathBuf> {
            members.iter().find(|m| m.key == write.from).map(|m| m.game.join(&write.path)).ok_or_else(|| {
                AppError::internal(format!(
                    "{} comes from {}, which does not take part",
                    write.path, write.from
                ))
            })
        };
        let result = (|| -> AppResult<Option<PathBuf>> {
            let folder = if held.is_empty() { None } else { Some(hold(member, &held)?) };
            let mut plan = TransactionPlan::new("build-sync");
            plan.replacements = writes.iter().map(|w| w.path.clone()).collect();
            plan.stale = removes.iter().map(|r| r.path.clone()).collect();
            plan.staged_bytes = writes
                .iter()
                .filter_map(|w| source_of(w).ok().and_then(|p| fs::metadata(p).ok()).map(|m| m.len()))
                .sum();
            execute(
                &member.game,
                SYNC_JOURNAL,
                plan,
                |transaction| {
                    for write in &writes {
                        copy_dated(&source_of(write)?, &transaction.stage_path(&write.path)?)?;
                    }
                    Ok(())
                },
                ApplyHooks::default(),
            )?;
            Ok(folder)
        })();
        match result {
            Ok(folder) => {
                for write in &writes {
                    let path = member.game.join(&write.path);
                    let hash =
                        outcome.agreed.get(&write.key).cloned().flatten().map(|v| v.hash).unwrap_or_default();
                    if let Ok(meta) = fs::metadata(&path) {
                        let seen = Seen { hash, size: meta.len(), modified_ms: modified_ms(&meta) };
                        applied.seen.insert((member.key.clone(), write.key.clone()), seen);
                    }
                }
                done.push(Done { member, folder, writes, removes, held });
            }
            Err(e) => {
                undo(&done);
                return Err(e);
            }
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::backend::decide::{Held, Outcome, Remove, Write};

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn member(tmp: &Path, key: &str) -> Member {
        let game = tmp.join("games").join(key);
        fs::create_dir_all(&game).unwrap();
        Member { key: key.into(), game }
    }

    fn write(build: &str, path: &str) -> Write {
        Write { build: build.into(), key: path.into(), path: path.into(), from: "a".into() }
    }

    fn held(build: &str, path: &str) -> Held {
        Held { build: build.into(), key: path.into(), path: path.into() }
    }

    fn remove(build: &str, path: &str) -> Remove {
        Remove { build: build.into(), key: path.into(), path: path.into() }
    }

    #[test]
    fn writes_and_removes_keep_no_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (member(tmp.path(), "a"), member(tmp.path(), "b"));
        put(&a.game, "options.txt", b"new keys");
        let edited = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(a.game.join("options.txt"))
            .unwrap()
            .set_modified(edited)
            .unwrap();
        put(&b.game, "options.txt", b"old");
        put(&b.game, "servers.dat", b"servers");
        let outcome = Outcome {
            writes: vec![write("b", "options.txt")],
            removes: vec![remove("b", "servers.dat")],
            held: vec![held("b", "options.txt"), held("b", "servers.dat")],
            ..Outcome::default()
        };
        let applied = apply(&[a, b], &outcome).unwrap();
        let b = member(tmp.path(), "b");
        assert_eq!(fs::read(b.game.join("options.txt")).unwrap(), b"new keys");
        let time = fs::metadata(b.game.join("options.txt")).unwrap().modified().unwrap();
        assert_eq!(
            time.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            edited.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            "the source's time"
        );
        assert!(!b.game.join("servers.dat").exists());
        assert!(!b.game.join(UNDO_DIR).exists(), "nothing is left held");
        assert_eq!(applied.seen.get(&("b".to_string(), "options.txt".to_string())).map(|s| s.size), Some(8));
    }

    #[test]
    fn a_failure_in_a_later_member_puts_the_earlier_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (member(tmp.path(), "a"), member(tmp.path(), "b"), member(tmp.path(), "c"));
        put(&a.game, "xaero/map.txt", b"new");
        put(&b.game, "xaero/map.txt", b"b's map");
        put(&b.game, "xaero/gone.txt", b"b's file");
        // In c the file's place is a folder: writing it fails.
        put(&c.game, "xaero/map.txt/inside.txt", b"a folder");
        let outcome = Outcome {
            writes: vec![
                write("b", "xaero/map.txt"),
                write("b", "xaero/added.txt"),
                write("c", "xaero/map.txt"),
            ],
            removes: vec![remove("b", "xaero/gone.txt")],
            held: vec![held("b", "xaero/map.txt"), held("b", "xaero/gone.txt")],
            ..Outcome::default()
        };
        put(&a.game, "xaero/added.txt", b"added");
        assert!(apply(&[a, b, c], &outcome).is_err());
        let b = member(tmp.path(), "b");
        assert_eq!(fs::read(b.game.join("xaero/map.txt")).unwrap(), b"b's map", "put back");
        assert_eq!(fs::read(b.game.join("xaero/gone.txt")).unwrap(), b"b's file", "put back");
        assert!(!b.game.join("xaero/added.txt").exists(), "what it added is taken away");
        assert!(!b.game.join(UNDO_DIR).exists(), "the undo folder is gone");
        let c = member(tmp.path(), "c");
        assert!(c.game.join("xaero/map.txt/inside.txt").is_file());
    }
}
