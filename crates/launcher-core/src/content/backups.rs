//! Mods a replacement took away: `mods/.backups/<old name>.backup` — one per
//! mod, the newest: backing a mod up drops its older backups (by the JAR's mod id), so updates
//! that rename the file do not pile them up. A listed mod offers its newest backup — the same
//! file name or the same mod id — unless that holds the same bytes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};

use super::inventory::MetadataCache;
use super::jar::inspect_mod_jar;

pub const BACKUPS: &str = ".backups";
const SUFFIX: &str = ".backup";

/// `<game>/mods/.backups`.
pub fn backups_dir(game: &Path) -> PathBuf {
    game.join("mods").join(BACKUPS)
}

fn io_error(code: ErrorCode, path: &Path, e: io::Error) -> AppError {
    AppError::new(code, format!("{}: {e}", path.display())).with_param("path", path.to_string_lossy())
}

/// Copies mod `relative` of `game` to `mods/.backups/<filename>.backup` over an older one and drops
/// the other backups of the same mod; a failed copy leaves no part behind.
pub fn back_up(game: &Path, relative: &str, filename: &str) -> AppResult<()> {
    let dir = backups_dir(game);
    fs::create_dir_all(&dir).map_err(|e| io_error(ErrorCode::BackupFailed, &dir, e))?;
    let target = dir.join(format!("{filename}{SUFFIX}"));
    let part = dir.join(format!("{filename}{SUFFIX}.part"));
    fs::copy(game.join(relative), &part).and_then(|_| fs::rename(&part, &target)).map_err(|e| {
        let _ = fs::remove_file(&part);
        io_error(ErrorCode::BackupFailed, &target, e)
    })?;
    if let Ok(mine) = inspect_mod_jar(&target, None) {
        drop_older(&dir, &target, &mine.mod_id);
    }
    Ok(())
}

/// Removes the backups in `dir` other than `kept` whose JAR is mod `mod_id` (best effort).
fn drop_older(dir: &Path, kept: &Path, mod_id: &str) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let is_backup = entry.file_name().to_string_lossy().ends_with(SUFFIX);
        if is_backup && path != kept && inspect_mod_jar(&path, None).is_ok_and(|d| d.mod_id == mod_id) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// One backup file.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub path: PathBuf,
    /// The name the mod had: the backup's name without `.backup`.
    pub filename: String,
    pub mod_id: Option<String>,
    pub size: u64,
    modified: Option<SystemTime>,
}

/// The backups of `game`'s mods, newest first.
pub fn backups(game: &Path, loader: Option<LoaderKind>, cache: &MetadataCache) -> Vec<Backup> {
    let Ok(read) = fs::read_dir(backups_dir(game)) else { return Vec::new() };
    let mut found: Vec<Backup> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let filename = name.strip_suffix(SUFFIX).filter(|f| !f.is_empty())?.to_string();
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).ok().filter(|m| m.is_file())?;
            let mod_id = cache.descriptor(&path, loader).map(|d| d.mod_id);
            Some(Backup { path, filename, mod_id, size: meta.len(), modified: meta.modified().ok() })
        })
        .collect();
    found.sort_by_key(|b| std::cmp::Reverse(b.modified));
    found
}

/// The newest backup of mod file `current` (named `filename`, JAR id `mod_id`), unless it holds the
/// same bytes: an older backup is never offered, so restoring twice cannot walk back further.
pub fn backup_of<'a>(
    backups: &'a [Backup],
    filename: &str,
    mod_id: Option<&str>,
    current: &Path,
) -> Option<&'a Backup> {
    backups
        .iter()
        .find(|b| {
            b.filename.eq_ignore_ascii_case(filename) || (mod_id.is_some() && b.mod_id.as_deref() == mod_id)
        })
        .filter(|b| !same_bytes(b, current))
}

fn same_bytes(backup: &Backup, current: &Path) -> bool {
    fs::metadata(current).is_ok_and(|m| m.len() == backup.size)
        && matches!((fs::read(&backup.path), fs::read(current)), (Ok(a), Ok(b)) if a == b)
}

/// Puts `backup` back in place of mod file `current`: under the backup's own name, switched off
/// when `current` was (`enabled`); `current` goes when the names differ. The backup stays. A file
/// of the new name that is not `current` is never overwritten.
pub fn restore(game: &Path, backup: &Backup, current: &Path, enabled: bool) -> AppResult<PathBuf> {
    let mods = game.join("mods");
    let name = if enabled { backup.filename.clone() } else { format!("{}.disabled", backup.filename) };
    let target = mods.join(&name);
    let is_current = |p: &Path| p.to_string_lossy().eq_ignore_ascii_case(&current.to_string_lossy());
    for taken in [mods.join(&backup.filename), mods.join(format!("{}.disabled", backup.filename))] {
        if !is_current(&taken) && fs::symlink_metadata(&taken).is_ok() {
            return Err(AppError::new(
                ErrorCode::ContentConflict,
                format!("{} already exists", taken.display()),
            )
            .with_param("name", backup.filename.clone()));
        }
    }
    let part = backups_dir(game).join(format!("{}.restore.part", backup.filename));
    fs::copy(&backup.path, &part).and_then(|_| fs::rename(&part, &target)).map_err(|e| {
        let _ = fs::remove_file(&part);
        io_error(ErrorCode::of_io(&e), &target, e)
    })?;
    if !is_current(&target) {
        fs::remove_file(current).map_err(|e| io_error(ErrorCode::of_io(&e), current, e))?;
    }
    Ok(target)
}
