//! A pass's decisions written into its builds: each build through the core's file transaction,
//! what it overwrites or removes kept first in its history, and the builds already written put
//! back as they were when a later one fails.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::backups::BuildBackups;
use launcher_core::storage::journal::JournalName;
use launcher_core::storage::json::write_json_file;
use launcher_core::storage::transaction::{ApplyHooks, TransactionPlan, execute};
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::json;

use super::decide::{Outcome, Remove, Saved, Write};
use super::scan::modified_ms;
use super::store::Seen;
use crate::dto::SyncGroup;

/// The journal of a sync pass's change in a build.
pub const SYNC_JOURNAL: JournalName =
    JournalName { file: "build-sync.json", legacy: ".launcher-build-sync.json" };

/// A build taking part in a pass.
pub struct Member {
    pub key: String,
    pub game: PathBuf,
    pub shelf: BuildBackups,
}

/// What a pass wrote: each written file as it is now (by build and key), and the history folders.
#[derive(Debug, Default)]
pub struct Applied {
    pub seen: BTreeMap<(String, String), Seen>,
    pub folders: Vec<PathBuf>,
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
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

/// The folder for a pass's copies in `shelf`: its time (UTC), then the first free number.
pub fn pass_folder(shelf: &BuildBackups, at: SystemTime) -> PathBuf {
    let stamp = chrono::DateTime::<chrono::Utc>::from(at).format("%Y-%m-%d_%H-%M-%S").to_string();
    (1..)
        .map(|n| shelf.sync().join(format!("{stamp}-{n}")))
        .find(|f| !f.exists())
        .unwrap_or_else(|| shelf.sync())
}

/// Keeps `saved` (the member's versions about to go) in a new pass folder with a `change.json`.
pub(crate) fn keep(
    member: &Member,
    saved: &[&Saved],
    group: &SyncGroup,
    at: SystemTime,
) -> AppResult<PathBuf> {
    let folder = pass_folder(&member.shelf, at);
    for version in saved {
        copy_dated(&member.game.join(&version.path), &folder.join("files").join(&version.path))?;
    }
    let change: Vec<_> = saved
        .iter()
        .map(|s| {
            json!({"group": group.id, "group_name": group.name, "path": s.path, "from": s.from,
                   "at_ms": millis(at), "reason": s.reason})
        })
        .collect();
    let note = folder.join("change.json");
    write_json_file(&note, &json!(change), 2).map_err(|e| io_error(&note, e))?;
    Ok(folder)
}

/// A member written, kept to be put back when a later member fails.
struct Done<'a> {
    member: &'a Member,
    folder: Option<PathBuf>,
    writes: Vec<&'a Write>,
    removes: Vec<&'a Remove>,
    saved: Vec<&'a Saved>,
}

/// Puts the members in `done` back as they were (best effort): a file it overwrote or removed
/// from its copy, a file it added removed.
fn undo(done: &[Done<'_>]) {
    for d in done.iter().rev() {
        let copy = |path: &str| d.folder.as_ref().map(|f| f.join("files").join(path));
        for write in &d.writes {
            let target = d.member.game.join(&write.path);
            let result = match d.saved.iter().find(|s| s.key == write.key) {
                Some(saved) => {
                    let _ = fs::remove_file(&target);
                    copy(&saved.path)
                        .map_or(Ok(()), |from| copy_dated(&from, &d.member.game.join(&saved.path)))
                }
                None => fs::remove_file(&target).map_err(|e| io_error(&target, e)),
            };
            if let Err(e) = result {
                tracing::warn!("Unable to put {} back in {}: {}", write.path, d.member.key, e.detail);
            }
        }
        for removed in &d.removes {
            if let Some(from) = copy(&removed.path)
                && let Err(e) = copy_dated(&from, &d.member.game.join(&removed.path))
            {
                tracing::warn!("Unable to put {} back in {}: {}", removed.path, d.member.key, e.detail);
            }
        }
    }
}

/// Writes `outcome` into `members`, one after another, each through its own transaction; what a
/// member's change overwrites or removes is kept first in its history. When a member fails, the
/// ones before are put back and the error comes back.
pub fn apply(members: &[Member], outcome: &Outcome, group: &SyncGroup, at: SystemTime) -> AppResult<Applied> {
    let mut applied = Applied::default();
    let mut done: Vec<Done<'_>> = Vec::new();
    for member in members {
        let writes: Vec<&Write> = outcome.writes.iter().filter(|w| w.build == member.key).collect();
        let removes: Vec<&Remove> = outcome.removes.iter().filter(|r| r.build == member.key).collect();
        if writes.is_empty() && removes.is_empty() {
            continue;
        }
        let saved: Vec<&Saved> = outcome.history.iter().filter(|s| s.build == member.key).collect();
        let source_of = |write: &Write| -> AppResult<PathBuf> {
            members.iter().find(|m| m.key == write.from).map(|m| m.game.join(&write.path)).ok_or_else(|| {
                AppError::internal(format!(
                    "{} comes from {}, which does not take part",
                    write.path, write.from
                ))
            })
        };
        let result = (|| -> AppResult<Option<PathBuf>> {
            let folder = if saved.is_empty() { None } else { Some(keep(member, &saved, group, at)?) };
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
                applied.folders.extend(folder.clone());
                done.push(Done { member, folder, writes, removes, saved });
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

    use launcher_core::backups::BuildBackups;
    use serde_json::Value;

    use super::*;
    use crate::backend::decide::{Outcome, Remove, Saved, Write};
    use crate::dto::{Reason, SyncGroup};

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn member(tmp: &Path, key: &str) -> Member {
        let game = tmp.join("games").join(key);
        fs::create_dir_all(&game).unwrap();
        Member { key: key.into(), game, shelf: BuildBackups::of(&tmp.join("backups"), key) }
    }

    fn group() -> SyncGroup {
        SyncGroup {
            id: "g1".into(), name: "Налаштування".into(), start: "a".into(), ..SyncGroup::default()
        }
    }

    fn write(build: &str, path: &str) -> Write {
        Write { build: build.into(), key: path.into(), path: path.into(), from: "a".into() }
    }

    fn saved(build: &str, path: &str, reason: Reason) -> Saved {
        let from = (reason != Reason::Removed).then(|| "a".to_string());
        Saved { build: build.into(), key: path.into(), path: path.into(), from, reason }
    }

    #[test]
    fn writes_and_removes_keep_their_history() {
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
            removes: vec![Remove {
                build: "b".into(),
                key: "servers.dat".into(),
                path: "servers.dat".into(),
            }],
            history: vec![
                saved("b", "options.txt", Reason::Changed),
                saved("b", "servers.dat", Reason::Removed),
            ],
            ..Outcome::default()
        };
        let at = UNIX_EPOCH + Duration::from_secs(1_791_800_000);
        let applied = apply(&[a, b], &outcome, &group(), at).unwrap();
        let b = member(tmp.path(), "b");
        assert_eq!(fs::read(b.game.join("options.txt")).unwrap(), b"new keys");
        let time = fs::metadata(b.game.join("options.txt")).unwrap().modified().unwrap();
        assert_eq!(
            time.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            edited.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            "the source's time"
        );
        assert!(!b.game.join("servers.dat").exists());
        let pass = b.shelf.sync().join("2026-10-12_10-13-20-1");
        assert_eq!(fs::read(pass.join("files/options.txt")).unwrap(), b"old");
        assert_eq!(fs::read(pass.join("files/servers.dat")).unwrap(), b"servers");
        let change: Value = serde_json::from_slice(&fs::read(pass.join("change.json")).unwrap()).unwrap();
        assert_eq!(change[0]["group"], "g1");
        assert_eq!(change[0]["reason"], "changed");
        assert_eq!(change[1]["reason"], "removed");
        assert_eq!(applied.seen.get(&("b".to_string(), "options.txt".to_string())).map(|s| s.size), Some(8));
    }

    #[test]
    fn a_failure_in_the_second_member_restores_the_first() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (member(tmp.path(), "a"), member(tmp.path(), "b"), member(tmp.path(), "c"));
        put(&a.game, "options.txt", b"new");
        put(&b.game, "options.txt", b"old b");
        // In c the file's place is a folder: writing it fails.
        put(&c.game, "options.txt/inside.txt", b"a folder");
        let outcome = Outcome {
            writes: vec![write("b", "options.txt"), write("c", "options.txt")],
            history: vec![saved("b", "options.txt", Reason::Changed)],
            ..Outcome::default()
        };
        assert!(apply(&[a, b, c], &outcome, &group(), SystemTime::now()).is_err());
        let b = member(tmp.path(), "b");
        assert_eq!(fs::read(b.game.join("options.txt")).unwrap(), b"old b", "put back as it was");
        let c = member(tmp.path(), "c");
        assert!(c.game.join("options.txt/inside.txt").is_file());
    }
}
