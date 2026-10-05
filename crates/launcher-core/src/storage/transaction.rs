//! A transactional change of a build's files: stage every new file, check it,
//! then swap it in with a backup of what it replaces — undone as a whole on any failure, and after
//! a crash by the next transaction of the same journal or the next launch.

use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};

use super::journal::{SyncJournal, contained, normalized_path};
use crate::net::preflight::{SpaceRequest, preflight};

/// What a transaction changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPlan {
    /// Its name in the journal (`modrinth-content-install`, …).
    pub operation: String,
    /// Files staged and then swapped in, relative to the root.
    pub replacements: Vec<String>,
    /// Files deleted.
    pub stale: Vec<String>,
    /// The bytes the staged files take.
    pub staged_bytes: u64,
    /// For a transaction with a commit step: what identifies it when resuming after a crash.
    pub commit_key: Option<String>,
}

impl TransactionPlan {
    pub fn new(operation: &str) -> TransactionPlan {
        TransactionPlan {
            operation: operation.to_string(),
            replacements: Vec::new(),
            stale: Vec::new(),
            staged_bytes: 0,
            commit_key: None,
        }
    }
}

/// Steps around the swap: `validate` before and after `before_activate`, `commit` after it.
#[derive(Default, Clone, Copy)]
pub struct ApplyHooks<'a> {
    pub validate: Option<&'a dyn Fn() -> AppResult<()>>,
    pub before_activate: Option<&'a dyn Fn() -> AppResult<()>>,
    pub commit: Option<&'a dyn Fn() -> AppResult<()>>,
}

fn commit_mismatch() -> AppError {
    AppError::new(ErrorCode::InvalidInput, "a commit step needs a commit key, and a commit key a commit step")
}

/// The bytes of the present files a plan replaces or deletes (their backups).
fn rollback_bytes(root: &Path, plan: &TransactionPlan) -> u64 {
    plan.replacements
        .iter()
        .chain(&plan.stale)
        .filter_map(|raw| normalized_path(raw).ok())
        .filter_map(|relative| contained(root, &relative).ok())
        .filter_map(|path| fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len()))
        .sum()
}

/// An open transaction: its new files go to `stage_path`, then `apply` swaps them in.
#[derive(Debug)]
pub struct FileTransaction {
    plan: TransactionPlan,
    journal: SyncJournal,
    /// The transaction's stage folder (from `begin`).
    stage: PathBuf,
}

impl FileTransaction {
    /// Checks there is room for the staged files and the backups, finishes what an earlier
    /// transaction of `journal_file` left (`commit_recovery` resumes its commit), and opens this one.
    pub fn begin(
        root: &Path,
        journal_file: &str,
        plan: TransactionPlan,
        commit_recovery: Option<&dyn Fn() -> AppResult<()>>,
    ) -> AppResult<FileTransaction> {
        if plan.operation.trim().is_empty() {
            return Err(AppError::new(ErrorCode::InvalidInput, "a file transaction needs an operation"));
        }
        let bytes = plan.staged_bytes.saturating_add(rollback_bytes(root, &plan));
        preflight(&[SpaceRequest { dir: root.to_path_buf(), bytes, label: plan.operation.clone() }])?;
        let journal = SyncJournal::new(root, journal_file);
        let stage = journal.begin(
            &plan.operation,
            &plan.replacements,
            &plan.stale,
            plan.commit_key.as_deref(),
            commit_recovery,
        )?;
        Ok(FileTransaction { plan, journal, stage })
    }

    /// Where the new `relative` file is staged (its folder created).
    pub fn stage_path(&self, relative: &str) -> AppResult<PathBuf> {
        let path = contained(&self.stage, &normalized_path(relative)?)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", parent.display())))?;
        }
        Ok(path)
    }

    /// Checks, swaps the staged files in, commits and completes; any failure rolls everything back.
    pub fn apply(self, hooks: ApplyHooks<'_>) -> AppResult<()> {
        if hooks.commit.is_some() != self.plan.commit_key.is_some() {
            return Err(self.abort(commit_mismatch()));
        }
        let steps = || -> AppResult<()> {
            if let Some(validate) = hooks.validate {
                validate()?;
            }
            self.journal.mark_prepared()?;
            if let Some(before) = hooks.before_activate {
                before()?;
            }
            if let Some(validate) = hooks.validate {
                validate()?;
            }
            self.journal.activate()?;
            if let Some(commit) = hooks.commit {
                self.journal.mark_committing()?;
                commit()?;
            }
            self.journal.complete()
        };
        steps().map_err(|e| self.rolled_back(e))
    }

    /// Gives up (a stage failed): everything is undone; `error` comes back, with a failed
    /// rollback added to it.
    pub fn abort(self, error: AppError) -> AppError {
        self.rolled_back(error)
    }

    fn rolled_back(&self, mut error: AppError) -> AppError {
        if matches!(self.journal.status().as_deref(), Some("complete" | "rolled_back" | "recovered")) {
            return error;
        }
        if let Err(rollback) = self.journal.rollback() {
            error.detail =
                format!("{}; {} rollback failed: {}", error.detail, self.plan.operation, rollback.detail);
        }
        error
    }
}

/// `begin`, `stage` (a stage that needs no `await`) and `apply` in one go.
pub fn execute(
    root: &Path,
    journal_file: &str,
    plan: TransactionPlan,
    stage: impl FnOnce(&FileTransaction) -> AppResult<()>,
    hooks: ApplyHooks<'_>,
) -> AppResult<()> {
    if hooks.commit.is_some() != plan.commit_key.is_some() {
        return Err(commit_mismatch());
    }
    let transaction = FileTransaction::begin(root, journal_file, plan, hooks.commit)?;
    if let Err(e) = stage(&transaction) {
        return Err(transaction.abort(e));
    }
    transaction.apply(hooks)
}

/// Transaction journals are `.launcher-<name>sync.json`.
fn is_journal(name: &str) -> bool {
    name.starts_with(".launcher-") && name.ends_with("sync.json")
}

/// Undoes every interrupted transaction in `dir` (a build's folder) before its game starts; one in
/// its commit step is left to the operation that owns it. Returns what could not be undone — each
/// such journal is set aside once reported (see `SyncJournal::set_aside`).
pub fn recover_interrupted(dir: &Path) -> Vec<AppError> {
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    let mut journals: Vec<String> =
        read.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_journal(n)).collect();
    journals.sort();
    journals
        .iter()
        .filter_map(|name| {
            let journal = SyncJournal::new(dir, name);
            if journal.status().as_deref() == Some("committing") {
                return None;
            }
            let error = journal.recover(None, None).err()?.with_param("journal", name);
            Some(match journal.set_aside() {
                Ok(kept) => {
                    AppError { detail: format!("{}; its record is kept as {kept}", error.detail), ..error }
                }
                Err(_) => error,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::{Value, json};

    use super::*;
    use crate::storage::journal::{SYNC_JOURNAL, SYNC_WORK_DIR};

    fn plan(replacements: &[&str], stale: &[&str]) -> TransactionPlan {
        TransactionPlan {
            replacements: replacements.iter().map(|s| s.to_string()).collect(),
            stale: stale.iter().map(|s| s.to_string()).collect(),
            ..TransactionPlan::new("test")
        }
    }

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn stage(tx: &FileTransaction, files: &[(&str, &[u8])]) -> AppResult<()> {
        for (relative, bytes) in files {
            fs::write(tx.stage_path(relative)?, bytes).unwrap();
        }
        Ok(())
    }

    fn journal(root: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(root.join(SYNC_JOURNAL)).unwrap()).unwrap()
    }

    #[test]
    fn a_change_swaps_files_in_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old a");
        put(root, "mods/stale.jar", b"stale");
        put(root, "keep.txt", b"keep");
        let files: [(&str, &[u8]); 2] = [("mods/a.jar", b"new a"), ("config/new.toml", b"new")];
        execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar", "config/new.toml"], &["mods/stale.jar", "MODS/A.JAR"]),
            |tx| stage(tx, &files),
            ApplyHooks::default(),
        )
        .unwrap();
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"new a");
        assert_eq!(fs::read(root.join("config/new.toml")).unwrap(), b"new");
        assert!(!root.join("mods/stale.jar").exists() && root.join("keep.txt").exists());
        let saved = journal(root);
        assert_eq!(
            (&saved["status"], &saved["schema_version"], &saved["operation"]),
            (&json!("complete"), &json!(2), &json!("test"))
        );
        let kinds: Vec<&str> =
            saved["entries"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["replace", "replace", "delete"], "a stale path equal to a replacement is dropped");
        assert!(!root.join(SYNC_WORK_DIR).exists());
    }

    #[test]
    fn a_failed_stage_or_check_leaves_everything_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old");
        let err = execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar"], &[]),
            |tx| {
                fs::write(tx.stage_path("mods/a.jar")?, b"half").unwrap();
                Err(AppError::new(ErrorCode::DownloadFailed, "network"))
            },
            ApplyHooks::default(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old");
        assert_eq!(journal(root)["status"], "rolled_back");
        let check = || -> AppResult<()> { Err(AppError::new(ErrorCode::IntegrityMismatch, "hash")) };
        let bad: [(&str, &[u8]); 1] = [("mods/a.jar", b"bad")];
        let err = execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar"], &[]),
            |tx| stage(tx, &bad),
            ApplyHooks { validate: Some(&check), ..ApplyHooks::default() },
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::IntegrityMismatch);
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old");
        assert_eq!(journal(root)["status"], "rolled_back");
        assert!(!root.join(SYNC_WORK_DIR).exists());
    }

    #[test]
    fn a_failure_while_swapping_restores_what_was_swapped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old a");
        put(root, "mods/b.jar", b"old b");
        // b is planned but never staged: swapping fails after a is already in place.
        let files: [(&str, &[u8]); 1] = [("mods/a.jar", b"new a")];
        let err = execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar", "mods/b.jar"], &[]),
            |tx| stage(tx, &files),
            ApplyHooks::default(),
        )
        .unwrap_err();
        assert!(err.detail.contains("mods/b.jar"), "{}", err.detail);
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old a");
        assert_eq!(fs::read(root.join("mods/b.jar")).unwrap(), b"old b");
        assert_eq!(journal(root)["status"], "rolled_back");
    }

    #[test]
    fn a_commit_runs_last_and_needs_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let commits = Cell::new(0);
        let commit = || -> AppResult<()> {
            assert!(root.join("a.txt").exists(), "files are in place before the commit");
            commits.set(commits.get() + 1);
            Ok(())
        };
        let files: [(&str, &[u8]); 1] = [("a.txt", b"a")];
        let keyed = TransactionPlan { commit_key: Some("sync:abc".into()), ..plan(&["a.txt"], &[]) };
        execute(
            root,
            SYNC_JOURNAL,
            keyed,
            |tx| stage(tx, &files),
            ApplyHooks { commit: Some(&commit), ..ApplyHooks::default() },
        )
        .unwrap();
        assert_eq!((commits.get(), journal(root)["commit_key"].clone()), (1, json!("sync:abc")));
        let other: [(&str, &[u8]); 1] = [("b.txt", b"b")];
        let unkeyed = execute(
            root,
            SYNC_JOURNAL,
            plan(&["b.txt"], &[]),
            |tx| stage(tx, &other),
            ApplyHooks { commit: Some(&commit), ..ApplyHooks::default() },
        )
        .unwrap_err();
        let keyless = TransactionPlan { commit_key: Some("sync:x".into()), ..plan(&["b.txt"], &[]) };
        let uncommitted =
            execute(root, SYNC_JOURNAL, keyless, |tx| stage(tx, &other), ApplyHooks::default()).unwrap_err();
        assert_eq!((unkeyed.code, uncommitted.code), (ErrorCode::InvalidInput, ErrorCode::InvalidInput));
        assert!(!root.join("b.txt").exists() && commits.get() == 1);
        let failing = || -> AppResult<()> { Err(AppError::new(ErrorCode::Network, "offline")) };
        let failed = TransactionPlan { commit_key: Some("sync:y".into()), ..plan(&["a.txt"], &[]) };
        let newer: [(&str, &[u8]); 1] = [("a.txt", b"newer")];
        assert!(
            execute(
                root,
                SYNC_JOURNAL,
                failed,
                |tx| stage(tx, &newer),
                ApplyHooks { commit: Some(&failing), ..ApplyHooks::default() }
            )
            .is_err()
        );
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"a", "a failed commit rolls the files back");
    }

    #[test]
    fn unsafe_or_duplicate_paths_touch_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for bad in [&["../x"][..], &["/abs"], &["C:/x"], &["a/../b"], &[""], &["mods/A.jar", "mods/a.JAR"]] {
            let err =
                execute(root, SYNC_JOURNAL, plan(bad, &[]), |_| Ok(()), ApplyHooks::default()).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
        }
        assert!(!root.join(SYNC_JOURNAL).exists() && !root.join(SYNC_WORK_DIR).exists());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            put(outside.path(), "a.jar", b"outside");
            std::os::unix::fs::symlink(outside.path(), root.join("mods")).unwrap();
            let files: [(&str, &[u8]); 1] = [("mods/a.jar", b"x")];
            let err = execute(
                root,
                SYNC_JOURNAL,
                plan(&["mods/a.jar"], &[]),
                |tx| stage(tx, &files),
                ApplyHooks::default(),
            )
            .unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput);
            assert_eq!(fs::read(outside.path().join("a.jar")).unwrap(), b"outside");
        }
    }

    #[test]
    fn there_must_be_room_for_the_change() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a.jar", b"a");
        let huge = TransactionPlan { staged_bytes: u64::MAX / 4, ..plan(&["a.jar"], &[]) };
        let err = execute(dir.path(), SYNC_JOURNAL, huge, |_| Ok(()), ApplyHooks::default()).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotEnoughSpace);
        assert!(!dir.path().join(SYNC_JOURNAL).exists());
    }

    /// An antivirus scans a staged file just written and holds it for a moment: the swap waits.
    #[cfg(windows)]
    #[test]
    fn a_staged_file_held_for_a_moment_still_takes_its_place() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old");
        execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar"], &[]),
            |tx| {
                let staged = tx.stage_path("mods/a.jar")?;
                fs::write(&staged, b"new").unwrap();
                let held = fs::OpenOptions::new().read(true).share_mode(0x1).open(&staged).unwrap();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    drop(held);
                });
                Ok(())
            },
            ApplyHooks::default(),
        )
        .unwrap();
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"new");
    }

    #[test]
    fn a_crash_is_cleared_by_the_next_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old");
        let tx = FileTransaction::begin(root, SYNC_JOURNAL, plan(&["mods/a.jar"], &[]), None).unwrap();
        fs::write(tx.stage_path("mods/a.jar").unwrap(), b"half").unwrap();
        drop(tx); // the launcher stopped here
        let files: [(&str, &[u8]); 1] = [("mods/c.jar", b"c")];
        execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/c.jar"], &[]),
            |tx| stage(tx, &files),
            ApplyHooks::default(),
        )
        .unwrap();
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old");
        assert!(root.join("mods/c.jar").exists());
        assert_eq!(fs::read_dir(root.join(SYNC_WORK_DIR)).map(|d| d.count()).unwrap_or(0), 0);
    }

    #[test]
    fn launches_undo_interrupted_changes_but_leave_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = "0123456789abcdef0123456789abcdef";
        put(root, &format!("{SYNC_WORK_DIR}/{id}/backup/mods/a.jar"), b"old");
        put(root, "mods/a.jar", b"new");
        let applying = json!({"schema_version": 2, "status": "applying", "transaction_id": id, "entries": [
            {"path": "mods/a.jar", "kind": "replace", "state": "applied", "had_original": true}]});
        fs::write(root.join(".launcher-modrinth-sync.json"), applying.to_string()).unwrap();
        let committing = json!({"schema_version": 2, "status": "committing", "commit_key": "sync:a",
            "transaction_id": "ffffffffffffffffffffffffffffffff", "entries": []});
        fs::write(root.join(SYNC_JOURNAL), committing.to_string()).unwrap();
        fs::write(root.join(".launcher-notes.json"), "{").unwrap();
        assert!(recover_interrupted(root).is_empty());
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old");
        assert_eq!(journal(root)["status"], "committing", "a commit waits for its own operation");
        fs::write(root.join(".launcher-pack-sync.json"), "{broken").unwrap();
        let problems = recover_interrupted(root);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(recover_interrupted(&root.join("missing")).is_empty());
    }

    #[test]
    fn a_change_that_cannot_be_undone_is_set_aside_after_one_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = "0123456789abcdef0123456789abcdef";
        // The backup of mods/a.jar is gone: this undo can never finish.
        put(root, "mods/a.jar", b"new");
        put(root, &format!("{SYNC_WORK_DIR}/{id}/stage/keep.txt"), b"work");
        let applying = json!({"schema_version": 2, "status": "applying", "transaction_id": id, "entries": [
            {"path": "mods/a.jar", "kind": "replace", "state": "applied", "had_original": true}]});
        let name = ".launcher-modrinth-sync.json";
        fs::write(root.join(name), applying.to_string()).unwrap();
        let problems = recover_interrupted(root);
        assert_eq!(problems.len(), 1, "{problems:?}");
        let kept: Vec<String> = fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&format!("{name}.")) && n.ends_with(".failed"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(
            problems[0].detail.ends_with(&format!("its record is kept as {}", kept[0])),
            "{}",
            problems[0].detail
        );
        assert!(!root.join(name).exists());
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"new", "nothing is guessed");
        assert!(root.join(SYNC_WORK_DIR).join(id).is_dir(), "the work folder, with any backups, stays");
        assert!(recover_interrupted(root).is_empty(), "one warning, not one per launch");
        // The operation that owns the journal works again.
        let files: [(&str, &[u8]); 1] = [("mods/b.jar", b"b")];
        execute(root, name, plan(&["mods/b.jar"], &[]), |tx| stage(tx, &files), ApplyHooks::default())
            .unwrap();
        assert_eq!(fs::read(root.join("mods/b.jar")).unwrap(), b"b");
    }

    #[test]
    fn a_broken_journal_is_set_aside_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".launcher-pack-sync.json"), "{broken").unwrap();
        assert_eq!(recover_interrupted(root).len(), 1);
        assert!(!root.join(".launcher-pack-sync.json").exists());
        assert!(recover_interrupted(root).is_empty());
    }

    #[test]
    fn a_folder_where_a_file_goes_is_refused_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "config/foo/inner.txt", b"mine");
        let over: [(&str, &[u8]); 1] = [("config/foo", b"file")];
        let err = execute(
            root,
            SYNC_JOURNAL,
            plan(&["config/foo"], &[]),
            |tx| stage(tx, &over),
            ApplyHooks::default(),
        )
        .unwrap_err();
        assert!(err.detail.contains("config/foo"), "{}", err.detail);
        assert_eq!(journal(root)["status"], "rolled_back");
        assert_eq!(fs::read(root.join("config/foo/inner.txt")).unwrap(), b"mine");
        put(root, "mods/olddir/keep.txt", b"keep");
        let files: [(&str, &[u8]); 1] = [("a.txt", b"a")];
        let failing = || -> AppResult<()> { Err(AppError::new(ErrorCode::Network, "offline")) };
        let keyed =
            TransactionPlan { commit_key: Some("sync:z".into()), ..plan(&["a.txt"], &["mods/olddir"]) };
        let hooks = ApplyHooks { commit: Some(&failing), ..ApplyHooks::default() };
        assert!(execute(root, SYNC_JOURNAL, keyed, |tx| stage(tx, &files), hooks).is_err());
        assert_eq!(journal(root)["status"], "rolled_back", "a stale folder is left alone, not a repair");
        execute(
            root,
            SYNC_JOURNAL,
            plan(&["a.txt"], &["mods/olddir"]),
            |tx| stage(tx, &files),
            ApplyHooks::default(),
        )
        .unwrap();
        assert!(root.join("mods/olddir/keep.txt").exists() && root.join("a.txt").exists());
    }

    #[test]
    fn a_linked_folder_is_refused_before_staging() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let outside = tempfile::tempdir().unwrap();
        put(outside.path(), "a.jar", b"outside");
        #[cfg(windows)]
        let linked = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("mods"))
            .arg(outside.path())
            .output()
            .unwrap()
            .status
            .success();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), root.join("mods")).is_ok();
        assert!(linked);
        let staged = Cell::new(false);
        let err = execute(
            root,
            SYNC_JOURNAL,
            plan(&["mods/a.jar"], &[]),
            |_| {
                staged.set(true);
                Ok(())
            },
            ApplyHooks::default(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(!staged.get() && !root.join(SYNC_JOURNAL).exists());
        assert_eq!(fs::read(outside.path().join("a.jar")).unwrap(), b"outside");
    }

    #[test]
    fn two_entries_for_one_file_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "mods/a.jar", b"old");
        let tx = FileTransaction::begin(root, SYNC_JOURNAL, plan(&["mods/a.jar"], &[]), None).unwrap();
        fs::write(tx.stage_path("mods/a.jar").unwrap(), b"new").unwrap();
        // Another entry naming the same file (a Windows alias) has already backed it up.
        let id = journal(root)["transaction_id"].as_str().unwrap().to_string();
        put(root, &format!("{SYNC_WORK_DIR}/{id}/backup/mods/a.jar"), b"other");
        let err = tx.apply(ApplyHooks::default()).unwrap_err();
        assert!(err.detail.contains("mods/a.jar"), "{}", err.detail);
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old");
        assert_eq!(journal(root)["status"], "rolled_back");
    }

    #[test]
    fn thousands_of_files_swap_in_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let names: Vec<String> = (0..2000).map(|i| format!("config/many/{i}.txt")).collect();
        for name in names.iter().step_by(2) {
            put(root, name, b"old");
        }
        let plan = TransactionPlan { replacements: names.clone(), ..TransactionPlan::new("bulk") };
        let started = std::time::Instant::now();
        let tx = FileTransaction::begin(root, SYNC_JOURNAL, plan, None).unwrap();
        for name in &names {
            fs::write(tx.stage_path(name).unwrap(), b"new").unwrap();
        }
        tx.apply(ApplyHooks::default()).unwrap();
        let took = started.elapsed();
        assert!(names.iter().all(|n| fs::read(root.join(n)).unwrap() == b"new"));
        // A journal written once per file takes minutes here; a slow disk, seconds.
        assert!(took < std::time::Duration::from_secs(60), "2000 files took {took:?}");
    }
}
