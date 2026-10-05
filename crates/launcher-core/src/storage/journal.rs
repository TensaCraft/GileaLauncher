//! The file synchronization journal in the original's schema 2: what a
//! transaction plans, how far it got and how to undo it — so a change a crash interrupted is rolled
//! back (or, in its commit step, finished) the next time.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Map, Value, json};

use super::atomic::rename_retrying;
use super::json::write_json_file;

/// The default journal in a build's folder.
pub const SYNC_JOURNAL: &str = ".launcher-sync.json";
/// Where transactions stage new files and back up the ones they replace.
pub const SYNC_WORK_DIR: &str = ".launcher-sync";
const REPAIR_STATUSES: [&str; 8] = [
    "running",
    "staging",
    "prepared",
    "applying",
    "files_applied",
    "committing",
    "failed",
    "repair_required",
];

type Payload = Map<String, Value>;

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", path.display()))
}

fn journal_error(detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Io, detail)
}

fn unsafe_path(raw: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("unsafe managed file path: {raw:?}"))
        .with_param("path", raw)
}

/// A part Windows reads as a device whatever its extension (`con`, `nul.txt`, `COM1.json`).
fn device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end().to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            let mut chars = n.chars();
            matches!((chars.next(), chars.next()), (Some('0'..='9' | '¹' | '²' | '³'), None))
        })
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

/// A managed path relative to the root: `/`-separated, without empty parts or `.`; never absolute,
/// never with a drive or `..`, and no part Windows would read as another name (a trailing `.` or
/// space, a `:` stream, a device such as `con` or `nul.txt`).
pub fn normalized_path(raw: &str) -> AppResult<String> {
    let text = raw.trim().replace('\\', "/");
    if text.is_empty() || text.starts_with('/') || text.as_bytes().get(1) == Some(&b':') {
        return Err(unsafe_path(raw));
    }
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let alias = |p: &&str| *p == ".." || p.ends_with('.') || p.ends_with(' ') || p.contains(':') || device(p);
    if parts.is_empty() || parts.iter().any(alias) {
        return Err(unsafe_path(raw));
    }
    Ok(parts.join("/"))
}

/// `relative` (normalized) under `root`, refusing any link on the way, so nothing outside the
/// root is ever reached. Modules planning a transaction check their paths with it too.
pub fn contained(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(unsafe_path(relative));
        }
    }
    Ok(path)
}

fn unique_paths(raw: &[String]) -> AppResult<Vec<String>> {
    let mut seen = HashSet::new();
    raw.iter()
        .map(|value| {
            let path = normalized_path(value)?;
            if !seen.insert(path.to_lowercase()) {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("duplicate managed file path {path}"),
                )
                .with_param("path", &path));
            }
            Ok(path)
        })
        .collect()
}

fn status_of(payload: &Payload) -> String {
    payload.get("status").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase()
}

fn schema_of(payload: &Payload) -> Option<u64> {
    payload.get("schema_version").and_then(Value::as_u64)
}

fn entry_count(payload: &Payload) -> usize {
    payload.get("entries").and_then(Value::as_array).map_or(0, Vec::len)
}

fn entry_at(payload: &Payload, i: usize) -> Value {
    payload.get("entries").and_then(|e| e.get(i)).cloned().unwrap_or(Value::Null)
}

fn set_entry(payload: &mut Payload, i: usize, key: &str, value: Value) {
    if let Some(entry) = payload.get_mut("entries").and_then(|e| e.get_mut(i)).and_then(Value::as_object_mut)
    {
        entry.insert(key.into(), value);
    }
}

fn set(payload: &mut Payload, key: &str, value: Value) {
    payload.insert(key.into(), value);
}

/// Puts a copy of `backup` at `destination` through a temporary file, keeping the backup.
fn restore_copy(backup: &Path, destination: &Path) -> AppResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let name = destination.file_name().unwrap_or_default().to_string_lossy();
    let temp = destination.with_file_name(format!(".{name}.rollback.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = fs::copy(backup, &temp).and_then(|_| rename_retrying(&temp, destination));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| io_error(destination, e))
}

/// A transaction journal in `root`.
#[derive(Debug, Clone)]
pub struct SyncJournal {
    root: PathBuf,
    path: PathBuf,
}

impl SyncJournal {
    pub fn new(root: &Path, file: &str) -> SyncJournal {
        SyncJournal { root: root.to_path_buf(), path: root.join(file) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The journal, if it is there and readable.
    pub fn read(&self) -> Option<Payload> {
        let text = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str::<Value>(&text).ok()?.as_object().cloned()
    }

    /// The journal's status, lower-cased.
    pub fn status(&self) -> Option<String> {
        self.read().map(|p| status_of(&p))
    }

    /// An unreadable journal, or one whose transaction did not finish.
    pub fn needs_repair(&self) -> bool {
        if !self.path.exists() {
            return false;
        }
        self.read().is_none_or(|p| REPAIR_STATUSES.contains(&status_of(&p).as_str()))
    }

    /// The original's repair marker (schema 1): the owner must repair the build fully.
    pub fn mark_repair_required(&self, reason: &str, details: Option<Value>) -> AppResult<()> {
        let mut payload = Payload::new();
        set(&mut payload, "schema_version", json!(1));
        set(&mut payload, "status", json!("repair_required"));
        set(&mut payload, "reason", json!(if reason.trim().is_empty() { "unknown" } else { reason }));
        set(&mut payload, "marked_at", json!(now()));
        if let Some(details) = details {
            set(&mut payload, "details", details);
        }
        self.write(&payload)
    }

    /// Moves the journal aside as `<file>.<unix ms>.failed` — no longer a journal — so a change that
    /// cannot be undone stops blocking its operation and warning on every launch. The transaction's
    /// work folder, with any backups, stays. Returns the new file name.
    pub fn set_aside(&self) -> AppResult<String> {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let file = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = format!("{file}.{millis}.failed");
        fs::rename(&self.path, self.path.with_file_name(&name)).map_err(|e| io_error(&self.path, e))?;
        Ok(name)
    }

    fn write(&self, payload: &Payload) -> AppResult<()> {
        write_json_file(&self.path, &Value::Object(payload.clone()), 2).map_err(|e| io_error(&self.path, e))
    }

    fn read_strict(&self) -> AppResult<Payload> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(journal_error("the file synchronization journal is missing"));
            }
            Err(e) => return Err(io_error(&self.path, e)),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(payload)) => Ok(payload),
            Ok(_) => Err(journal_error("the file synchronization journal is invalid")),
            Err(_) => Err(journal_error("the file synchronization journal is corrupted")),
        }
    }

    fn transaction_root(&self, payload: &Payload) -> AppResult<PathBuf> {
        let id = payload.get("transaction_id").and_then(Value::as_str).unwrap_or_default().trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(journal_error("the file transaction ID is invalid"));
        }
        Ok(self.root.join(SYNC_WORK_DIR).join(id))
    }

    /// Recovers what an earlier transaction left, then plans a new one: `replacements` (staged,
    /// then swapped in) and `stale` files (deleted). Returns the stage folder.
    pub fn begin(
        &self,
        operation: &str,
        replacements: &[String],
        stale: &[String],
        commit_key: Option<&str>,
        commit_recovery: Option<&dyn Fn() -> AppResult<()>>,
    ) -> AppResult<PathBuf> {
        self.recover(commit_recovery, commit_key)?;
        let replacements = unique_paths(replacements)?;
        let taken: HashSet<String> = replacements.iter().map(|p| p.to_lowercase()).collect();
        let stale: Vec<String> =
            unique_paths(stale)?.into_iter().filter(|p| !taken.contains(&p.to_lowercase())).collect();
        for path in replacements.iter().chain(&stale) {
            contained(&self.root, path)?;
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let transaction = self.root.join(SYNC_WORK_DIR).join(&id);
        for sub in ["stage", "backup"] {
            fs::create_dir_all(transaction.join(sub)).map_err(|e| io_error(&transaction, e))?;
        }
        let entries: Vec<Value> = replacements
            .iter()
            .map(|p| json!({"path": p, "kind": "replace", "state": "planned"}))
            .chain(stale.iter().map(|p| json!({"path": p, "kind": "delete", "state": "planned"})))
            .collect();
        let payload = json!({
            "schema_version": 2,
            "status": "staging",
            "operation": operation,
            "commit_key": commit_key,
            "transaction_id": id,
            "entries": entries,
            "started_at": now(),
        });
        self.write(payload.as_object().expect("the journal is an object"))?;
        Ok(transaction.join("stage"))
    }

    /// Where the new `relative` file is staged.
    pub fn stage_path(&self, relative: &str) -> AppResult<PathBuf> {
        let payload = self.read_strict()?;
        contained(&self.transaction_root(&payload)?.join("stage"), &normalized_path(relative)?)
    }

    pub fn mark_prepared(&self) -> AppResult<()> {
        let mut payload = self.read_strict()?;
        if schema_of(&payload) != Some(2) || status_of(&payload) != "staging" {
            return Err(journal_error("the file transaction is not staging"));
        }
        set(&mut payload, "status", json!("prepared"));
        set(&mut payload, "prepared_at", json!(now()));
        self.write(&payload)
    }

    /// Swaps every staged file in (and stale ones out), backing up what was there; a failure
    /// rolls everything back.
    pub fn activate(&self) -> AppResult<()> {
        let mut payload = self.read_strict()?;
        if schema_of(&payload) != Some(2) || status_of(&payload) != "prepared" {
            return Err(journal_error("the file transaction is not prepared"));
        }
        let transaction = self.transaction_root(&payload)?;
        if !payload.get("entries").is_some_and(Value::is_array) {
            return Err(journal_error("the journal's entries are invalid"));
        }
        set(&mut payload, "status", json!("applying"));
        self.write(&payload)?;
        let mut current = String::from("<unknown>");
        if let Err(e) = self.apply_entries(&mut payload, &transaction, &mut current) {
            set(&mut payload, "status", json!("failed"));
            set(&mut payload, "error", json!(e.detail));
            set(&mut payload, "failed_at", json!(now()));
            self.write(&payload)?;
            let mut failure =
                AppError::new(e.code, format!("failed to activate managed file {current}: {}", e.detail))
                    .with_param("path", &current);
            if let Err(rollback) = self.rollback_payload(&mut payload) {
                failure.detail = format!("{}; rollback failed: {}", failure.detail, rollback.detail);
            }
            return Err(failure);
        }
        set(&mut payload, "status", json!("files_applied"));
        set(&mut payload, "applied_at", json!(now()));
        self.write(&payload)
    }

    fn apply_entries(
        &self,
        payload: &mut Payload,
        transaction: &Path,
        current: &mut String,
    ) -> AppResult<()> {
        struct Step {
            index: usize,
            relative: String,
            destination: PathBuf,
            backup: PathBuf,
            staged: PathBuf,
            replace: bool,
            had_original: bool,
        }
        let mut steps = Vec::new();
        for i in 0..entry_count(payload) {
            let entry = entry_at(payload, i);
            let Some(entry) = entry.as_object() else {
                return Err(journal_error("a journal entry is invalid"));
            };
            let relative = normalized_path(entry.get("path").and_then(Value::as_str).unwrap_or_default())?;
            current.clone_from(&relative);
            let destination = contained(&self.root, &relative)?;
            let backup = contained(&transaction.join("backup"), &relative)?;
            let staged = contained(&transaction.join("stage"), &relative)?;
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or_default();
            if kind != "replace" && kind != "delete" {
                return Err(journal_error(format!("unsupported synchronization action {kind:?}")));
            }
            let present = fs::symlink_metadata(&destination).ok();
            if present.as_ref().is_some_and(|m| !m.is_file()) {
                if kind == "delete" {
                    // Not a file: nothing to delete, and the entry stays planned.
                    continue;
                }
                return Err(journal_error(format!("a folder is where the file should go: {relative}")));
            }
            if kind == "replace" && !staged.is_file() {
                return Err(journal_error(format!("the staged file is missing: {relative}")));
            }
            let had_original = present.is_some();
            if had_original && backup.exists() {
                return Err(journal_error(format!("two entries name the same file: {relative}")));
            }
            steps.push(Step {
                index: i,
                relative,
                destination,
                backup,
                staged,
                replace: kind == "replace",
                had_original,
            });
        }
        // Each phase is written once for all its entries: an undo reads the files (a backup there or
        // not), so an entry's state only needs to say which phase may have touched it.
        for step in &steps {
            set_entry(payload, step.index, "had_original", json!(step.had_original));
            set_entry(payload, step.index, "state", json!("backing_up"));
        }
        self.write(payload)?;
        for step in steps.iter().filter(|s| s.had_original) {
            current.clone_from(&step.relative);
            if let Some(parent) = step.backup.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            rename_retrying(&step.destination, &step.backup).map_err(|e| io_error(&step.destination, e))?;
        }
        for step in &steps {
            set_entry(payload, step.index, "state", json!("backed_up"));
        }
        self.write(payload)?;
        for step in steps.iter().filter(|s| s.replace) {
            current.clone_from(&step.relative);
            if let Some(parent) = step.destination.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            rename_retrying(&step.staged, &step.destination).map_err(|e| io_error(&step.destination, e))?;
        }
        for step in &steps {
            set_entry(payload, step.index, "state", json!("applied"));
        }
        self.write(payload)
    }

    pub fn mark_committing(&self) -> AppResult<()> {
        let mut payload = self.read_strict()?;
        if schema_of(&payload) != Some(2) || status_of(&payload) != "files_applied" {
            return Err(journal_error("the file transaction has not applied its files"));
        }
        set(&mut payload, "status", json!("committing"));
        set(&mut payload, "commit_started_at", json!(now()));
        self.write(&payload)
    }

    /// Marks the transaction complete and clears its work folder (a folder that will not go is
    /// noted as `cleanup_pending`).
    pub fn complete(&self) -> AppResult<()> {
        let mut payload = self.read().unwrap_or_else(|| {
            let mut p = Payload::new();
            set(&mut p, "schema_version", json!(1));
            p
        });
        set(&mut payload, "status", json!("complete"));
        set(&mut payload, "completed_at", json!(now()));
        self.write(&payload)?;
        self.cleanup_best_effort(&mut payload);
        Ok(())
    }

    /// Undoes the transaction.
    pub fn rollback(&self) -> AppResult<()> {
        let mut payload = self.read_strict()?;
        if schema_of(&payload) != Some(2) {
            return Err(journal_error("a legacy journal cannot be rolled back"));
        }
        self.rollback_payload(&mut payload)
    }

    fn undo_entry(&self, entry: &Map<String, Value>, state: &str, transaction: &Path) -> AppResult<()> {
        let relative = normalized_path(entry.get("path").and_then(Value::as_str).unwrap_or_default())?;
        let destination = contained(&self.root, &relative)?;
        let backup = contained(&transaction.join("backup"), &relative)?;
        let had_original = entry.get("had_original").and_then(Value::as_bool);
        let swapped = state == "backed_up" || state == "applied";
        if backup.is_file() {
            restore_copy(&backup, &destination)
        } else if had_original == Some(false) && swapped {
            if !destination.is_file() {
                return Ok(());
            }
            match fs::remove_file(&destination) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(&destination, e)),
                _ => Ok(()),
            }
        } else if had_original == Some(true) && swapped {
            Err(journal_error("the rollback backup is missing"))
        } else if had_original == Some(true) && state == "backing_up" && !destination.is_file() {
            Err(journal_error("the original file and its rollback backup are both missing"))
        } else {
            Ok(())
        }
    }

    fn rollback_payload(&self, payload: &mut Payload) -> AppResult<()> {
        let transaction = self.transaction_root(payload)?;
        if !payload.get("entries").is_some_and(Value::is_array) {
            return Err(journal_error("the journal's entries are invalid"));
        }
        let mut errors: Vec<String> = Vec::new();
        for i in (0..entry_count(payload)).rev() {
            let entry = entry_at(payload, i);
            let Some(entry) = entry.as_object() else { continue };
            let state = entry.get("state").and_then(Value::as_str).unwrap_or("planned").to_string();
            if state == "planned" || state == "rolled_back" {
                continue;
            }
            let path = entry.get("path").and_then(Value::as_str).unwrap_or("?").to_string();
            // Undone in memory, written once below: undoing again is harmless (a backup is copied
            // back, an added file removed), so a crash midway repeats it.
            match self.undo_entry(entry, &state, &transaction) {
                Ok(()) => set_entry(payload, i, "state", json!("rolled_back")),
                Err(e) => errors.push(format!("{path}: {}", e.detail)),
            }
        }
        if !errors.is_empty() {
            set(payload, "status", json!("repair_required"));
            set(payload, "reason", json!("rollback_failed"));
            set(payload, "details", json!({"errors": errors.iter().take(10).collect::<Vec<_>>()}));
            set(payload, "marked_at", json!(now()));
            self.write(payload)?;
            let shown: Vec<&str> = errors.iter().take(5).map(String::as_str).collect();
            return Err(journal_error(format!("could not roll back managed files: {}", shown.join("; "))));
        }
        self.cleanup(payload)?;
        set(payload, "status", json!("rolled_back"));
        set(payload, "rolled_back_at", json!(now()));
        self.write(payload)
    }

    fn cleanup(&self, payload: &Payload) -> AppResult<()> {
        if schema_of(payload) != Some(2) || payload.get("transaction_id").is_none_or(Value::is_null) {
            return Ok(());
        }
        let transaction = self.transaction_root(payload)?;
        if transaction.exists() {
            fs::remove_dir_all(&transaction).map_err(|e| io_error(&transaction, e))?;
        }
        if let Some(work) = transaction.parent() {
            let _ = fs::remove_dir(work);
        }
        Ok(())
    }

    fn cleanup_best_effort(&self, payload: &mut Payload) {
        if let Err(e) = self.cleanup(payload) {
            set(payload, "cleanup_pending", json!(true));
            set(payload, "cleanup_error", json!(e.detail));
            let _ = self.write(payload);
        }
    }

    /// Finishes what an earlier transaction left; `true` when it had to act. A transaction stopped in its commit step resumes only with its own `commit_key`
    /// and commit function.
    pub fn recover(
        &self,
        commit_recovery: Option<&dyn Fn() -> AppResult<()>>,
        commit_key: Option<&str>,
    ) -> AppResult<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let mut payload = self.read_strict()?;
        let status = status_of(&payload);
        if schema_of(&payload) != Some(2) {
            return Ok(REPAIR_STATUSES.contains(&status.as_str()));
        }
        match status.as_str() {
            "complete" => {
                self.cleanup_best_effort(&mut payload);
                Ok(false)
            }
            "rolled_back" | "recovered" => {
                self.cleanup(&payload)?;
                Ok(false)
            }
            "staging" | "prepared" => {
                self.cleanup(&payload)?;
                set(&mut payload, "status", json!("recovered"));
                set(&mut payload, "recovered_at", json!(now()));
                self.write(&payload)?;
                Ok(true)
            }
            "applying" | "files_applied" | "failed" | "repair_required" => {
                self.rollback_payload(&mut payload)?;
                Ok(true)
            }
            "committing" => {
                let Some(commit) = commit_recovery else {
                    return Err(journal_error("an interrupted commit needs the operation that started it"));
                };
                let stored = payload.get("commit_key").and_then(Value::as_str).unwrap_or_default();
                if stored.is_empty() || Some(stored) != commit_key {
                    return Err(journal_error("the interrupted commit belongs to another operation"));
                }
                if let Err(e) = commit() {
                    set(&mut payload, "commit_recovery_error", json!(e.detail));
                    set(&mut payload, "commit_recovery_failed_at", json!(now()));
                    self.write(&payload)?;
                    return Err(AppError::new(e.code, format!("could not recover the commit: {}", e.detail)));
                }
                self.complete()?;
                Ok(true)
            }
            other => Err(journal_error(format!("unknown file synchronization state {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A transaction that stopped while swapping: `mods/a.jar` replaced (the old copy in the
    /// backup) and `mods/b.jar` added.
    fn crashed(root: &Path, status: &str) -> SyncJournal {
        put(root, &format!("{SYNC_WORK_DIR}/{ID}/backup/mods/a.jar"), b"old");
        fs::create_dir_all(root.join(SYNC_WORK_DIR).join(ID).join("stage")).unwrap();
        put(root, "mods/a.jar", b"new");
        put(root, "mods/b.jar", b"added");
        let journal = SyncJournal::new(root, SYNC_JOURNAL);
        let payload = json!({
            "schema_version": 2, "status": status, "operation": "test", "commit_key": null, "transaction_id": ID,
            "entries": [
                {"path": "mods/a.jar", "kind": "replace", "state": "applied", "had_original": true},
                {"path": "mods/b.jar", "kind": "replace", "state": "applied", "had_original": false}
            ]
        });
        fs::write(journal.path(), payload.to_string()).unwrap();
        journal
    }

    fn saved(journal: &SyncJournal) -> Value {
        serde_json::from_str(&fs::read_to_string(journal.path()).unwrap()).unwrap()
    }

    #[test]
    fn managed_paths_stay_inside_the_root() {
        assert_eq!(normalized_path(" mods\\./a.jar ").unwrap(), "mods/a.jar");
        assert_eq!(normalized_path("config//x/y.txt").unwrap(), "config/x/y.txt");
        for bad in [
            "",
            " ",
            ".",
            "/abs",
            "\\abs",
            "C:/x",
            "c:x",
            "../x",
            "a/../b",
            "a\\..\\b",
            "mods/a.jar.",
            "mods /a.jar",
            "mods/a.jar::$DATA",
            "mods/x:y/z.jar",
            "mods./a.jar",
            "con",
            "mods/NUL.txt",
            "config/com1.json",
            "lpt²",
        ] {
            assert_eq!(normalized_path(bad).unwrap_err().code, ErrorCode::InvalidInput, "{bad:?}");
        }
        for fine in ["console.txt", "com10.txt", "mods/auxiliary.jar", "lpt.txt"] {
            assert_eq!(normalized_path(fine).unwrap(), fine);
        }
        let root = tempfile::tempdir().unwrap();
        assert_eq!(contained(root.path(), "mods/a.jar").unwrap(), root.path().join("mods").join("a.jar"));
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("mods")).unwrap();
            assert_eq!(contained(root.path(), "mods/a.jar").unwrap_err().code, ErrorCode::InvalidInput);
        }
    }

    #[test]
    fn an_interrupted_swap_is_undone() {
        for status in ["applying", "files_applied", "failed", "repair_required"] {
            let root = tempfile::tempdir().unwrap();
            let journal = crashed(root.path(), status);
            assert!(journal.needs_repair(), "{status}");
            assert!(journal.recover(None, None).unwrap(), "{status}");
            assert_eq!(fs::read(root.path().join("mods/a.jar")).unwrap(), b"old", "{status}");
            assert!(!root.path().join("mods/b.jar").exists(), "{status}");
            let after = saved(&journal);
            assert_eq!(after["status"], "rolled_back", "{status}");
            assert!(after["entries"].as_array().unwrap().iter().all(|e| e["state"] == "rolled_back"));
            assert!(!root.path().join(SYNC_WORK_DIR).exists(), "{status}");
            assert!(!journal.needs_repair());
            assert!(!journal.recover(None, None).unwrap(), "nothing left to do");
        }
    }

    #[test]
    fn unfinished_staging_is_cleared() {
        let root = tempfile::tempdir().unwrap();
        put(root.path(), "mods/a.jar", b"old");
        let journal = SyncJournal::new(root.path(), SYNC_JOURNAL);
        let stage = journal.begin("test", &["mods/a.jar".into()], &[], None, None).unwrap();
        fs::create_dir_all(stage.join("mods")).unwrap();
        fs::write(stage.join("mods/a.jar"), b"half").unwrap();
        assert_eq!(journal.status().as_deref(), Some("staging"));
        assert!(journal.recover(None, None).unwrap());
        assert_eq!(saved(&journal)["status"], "recovered");
        assert!(!root.path().join(SYNC_WORK_DIR).exists());
        assert_eq!(fs::read(root.path().join("mods/a.jar")).unwrap(), b"old");
    }

    #[test]
    fn an_undo_without_its_backup_asks_for_repair() {
        let root = tempfile::tempdir().unwrap();
        let journal = crashed(root.path(), "applying");
        fs::remove_file(root.path().join(SYNC_WORK_DIR).join(ID).join("backup/mods/a.jar")).unwrap();
        let err = journal.recover(None, None).unwrap_err();
        assert!(err.detail.contains("mods/a.jar"), "{}", err.detail);
        let after = saved(&journal);
        assert_eq!(
            (&after["status"], &after["reason"]),
            (&json!("repair_required"), &json!("rollback_failed"))
        );
        assert_eq!(after["details"]["errors"].as_array().unwrap().len(), 1);
        assert!(!root.path().join("mods/b.jar").exists(), "what could be undone was");
        assert!(journal.needs_repair());
    }

    #[test]
    fn a_commit_resumes_only_with_its_own_operation() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(SYNC_WORK_DIR).join(ID).join("stage")).unwrap();
        let journal = SyncJournal::new(root.path(), SYNC_JOURNAL);
        let payload = json!({"schema_version": 2, "status": "committing", "commit_key": "sync:abc", "transaction_id": ID, "entries": []});
        fs::write(journal.path(), payload.to_string()).unwrap();
        let commits = Cell::new(0);
        let commit = || -> AppResult<()> {
            commits.set(commits.get() + 1);
            Ok(())
        };
        assert!(journal.recover(None, None).is_err(), "needs its operation");
        assert!(journal.recover(Some(&commit), Some("sync:other")).is_err(), "another operation");
        let failing = || -> AppResult<()> { Err(AppError::new(ErrorCode::Network, "offline")) };
        assert!(journal.recover(Some(&failing), Some("sync:abc")).is_err());
        assert_eq!(saved(&journal)["commit_recovery_error"], "offline");
        assert!(journal.recover(Some(&commit), Some("sync:abc")).unwrap());
        assert_eq!((commits.get(), saved(&journal)["status"].clone()), (1, json!("complete")));
        assert!(!root.path().join(SYNC_WORK_DIR).exists());
    }

    #[test]
    fn a_journal_says_when_a_repair_is_needed() {
        let root = tempfile::tempdir().unwrap();
        let journal = SyncJournal::new(root.path(), SYNC_JOURNAL);
        assert!(!journal.needs_repair() && !journal.recover(None, None).unwrap());
        fs::write(journal.path(), "{not json").unwrap();
        assert!(journal.needs_repair());
        journal.mark_repair_required("crash", Some(json!({"log": "latest.log"}))).unwrap();
        let marker = saved(&journal);
        assert_eq!(
            (&marker["schema_version"], &marker["status"], &marker["reason"]),
            (&json!(1), &json!("repair_required"), &json!("crash"))
        );
        assert!(marker["marked_at"].as_str().unwrap().ends_with("+00:00"));
        assert!(journal.needs_repair());
        assert!(journal.recover(None, None).unwrap(), "a legacy marker stays for its owner");
        fs::write(journal.path(), json!({"schema_version": 2, "status": "complete"}).to_string()).unwrap();
        assert!(!journal.needs_repair());
    }

    #[test]
    fn a_crash_inside_a_phase_is_undone_from_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Every entry is marked backing_up; only a.jar was moved to the backup before the crash.
        put(root, &format!("{SYNC_WORK_DIR}/{ID}/backup/mods/a.jar"), b"old-a");
        put(root, &format!("{SYNC_WORK_DIR}/{ID}/stage/mods/a.jar"), b"new-a");
        put(root, &format!("{SYNC_WORK_DIR}/{ID}/stage/mods/b.jar"), b"new-b");
        put(root, "mods/b.jar", b"old-b");
        let journal = SyncJournal::new(root, SYNC_JOURNAL);
        let payload = json!({
            "schema_version": 2, "status": "applying", "operation": "test", "commit_key": null, "transaction_id": ID,
            "entries": [
                {"path": "mods/a.jar", "kind": "replace", "state": "backing_up", "had_original": true},
                {"path": "mods/b.jar", "kind": "replace", "state": "backing_up", "had_original": true}
            ]
        });
        fs::write(journal.path(), payload.to_string()).unwrap();
        assert!(journal.recover(None, None).unwrap());
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"old-a", "moved: put back");
        assert_eq!(fs::read(root.join("mods/b.jar")).unwrap(), b"old-b", "not reached: untouched");
        assert_eq!(saved(&journal)["status"], "rolled_back");
    }
}
