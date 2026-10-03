//! Putting a backup back: the archive is unpacked beside the world, then the
//! world and the unpacked copy swap places through a journal, so a crash at any point leaves
//! either the old world or the restored one — never neither — once the next operation has run.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::net::preflight::{SpaceRequest, preflight};
use launcher_core::safe_path::safe_relative;
use launcher_core::storage::json::write_json_file;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};
use zip::ZipArchive;

const LEVEL: &str = "level.dat";
const RESERVE: u64 = 4 * 1024 * 1024;

fn io_err(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn invalid(what: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidInput, what.into())
}

#[cfg(test)]
thread_local! {
    /// Makes the step that puts the restored world in place fail (tests only).
    static FAIL_ACTIVATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Renames a folder, trying again for a while when it is refused: on Windows a scanner or an
/// indexer holding a file just written keeps the folder from being renamed for a moment.
fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 10;
    for attempt in 1..=ATTEMPTS {
        match fs::rename(from, to) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < ATTEMPTS => {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            other => return other,
        }
    }
    Ok(())
}

/// Puts the unpacked copy where the world belongs.
fn activate(staged: &Path, target: &Path) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_ACTIVATION.with(std::cell::Cell::get) {
        return Err(io::Error::other("the restored world could not be put in place"));
    }
    rename_retrying(staged, target)
}

/// Finishes or undoes every restore of `saves` a crash cut short, then removes the copies a crash
/// left half unpacked with no journal (as launchers before this one could): no restore is under
/// way when this runs (the build's folder is held).
pub fn recover_all(saves: &Path) -> Vec<AppError> {
    let Ok(entries) = fs::read_dir(saves) else { return Vec::new() };
    let names: Vec<String> =
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let worlds: Vec<String> = names
        .iter()
        .filter_map(|name| name.strip_prefix('.')?.strip_suffix("-world-restore.json").map(str::to_string))
        .collect();
    let problems: Vec<AppError> = worlds.iter().filter_map(|world| recover(saves, world).err()).collect();
    for name in &names {
        // `.W.restore-<hex>`: a staged copy. Never a `.previous`, which may hold the original.
        let Some((world, suffix)) = name.strip_prefix('.').and_then(|n| n.rsplit_once(".restore-")) else {
            continue;
        };
        let orphan = !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_hexdigit())
            && !journal_path(saves, world).exists();
        if orphan && let Err(e) = fs::remove_dir_all(saves.join(name)) {
            tracing::warn!("A half-restored copy {name} stays: {e}");
        }
    }
    problems
}

fn journal_path(saves: &Path, world: &str) -> PathBuf {
    saves.join(format!(".{world}-world-restore.json"))
}

/// `world` is one plain folder name of `saves/`.
fn plain_world(world: &str) -> AppResult<()> {
    let bad = world.is_empty()
        || world.starts_with('.')
        || world.contains(['/', '\\', '\0'])
        || world.trim() != world;
    if bad {
        Err(invalid(format!("not a world's folder: {world:?}")).with_param("name", world))
    } else {
        Ok(())
    }
}

/// Makes the renames in `saves` durable before the journal says they happened.
fn sync_dir(saves: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(saves) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = saves;
}

struct Journal {
    path: PathBuf,
    world: String,
    staged: String,
    previous: String,
    had_original: bool,
}

impl Journal {
    fn write(&self, phase: &str) -> AppResult<()> {
        let body = json!({
            "schema_version": 1,
            "phase": phase,
            "target_name": self.world,
            "staged_name": self.staged,
            "previous_name": self.previous,
            "had_original": self.had_original,
        });
        write_json_file(&self.path, &body, 2).map_err(|e| io_err(&self.path, e))
    }
}

/// Restores world `world` of `saves` from the backup `archive`: every entry a plain relative path
/// and `level.dat` among them, or the world is not touched.
pub fn restore(archive: &Path, saves: &Path, world: &str) -> AppResult<()> {
    plain_world(world)?;
    fs::create_dir_all(saves).map_err(|e| io_err(saves, e))?;
    recover(saves, world)?;
    let target = saves.join(world);
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(invalid(format!("{} is a link", target.display())));
    }
    let mut zip = ZipArchive::new(File::open(archive).map_err(|e| io_err(archive, e))?)
        .map_err(|e| invalid(format!("not a backup: {e}")))?;
    let mut entries: Vec<(usize, PathBuf, u64)> = Vec::new();
    let mut has_level = false;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| invalid(e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let rel = safe_relative(entry.name())
            .filter(|_| !entry.is_symlink())
            .ok_or_else(|| invalid(format!("unsafe path in the backup: {:?}", entry.name())))?;
        has_level |= rel == Path::new(LEVEL);
        entries.push((i, rel, entry.size()));
    }
    if !has_level {
        return Err(invalid("the backup holds no level.dat"));
    }
    let bytes = entries.iter().map(|e| e.2).fold(RESERVE, u64::saturating_add);
    preflight(&[SpaceRequest { dir: saves.to_path_buf(), bytes, label: world.to_string() }])?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let staged = format!(".{world}.restore-{:08x}", (stamp as u64) ^ u64::from(std::process::id()));
    let journal = Journal {
        path: journal_path(saves, world),
        world: world.to_string(),
        previous: format!("{staged}.previous"),
        staged,
        had_original: target.exists(),
    };
    let staged_dir = saves.join(&journal.staged);
    // Before the first file: a crash while unpacking leaves a copy the journal names, so the next
    // operation removes it.
    journal.write("unpacking")?;
    if let Err(e) = unpack(&mut zip, &entries, &staged_dir) {
        let _ = fs::remove_dir_all(&staged_dir);
        let _ = fs::remove_file(&journal.path);
        return Err(e);
    }
    if let Err(e) = journal.write("prepared") {
        let _ = fs::remove_dir_all(&staged_dir);
        return Err(e);
    }
    sync_dir(saves);
    let previous_dir = saves.join(&journal.previous);
    let swapped = (|| -> AppResult<()> {
        if journal.had_original {
            rename_retrying(&target, &previous_dir).map_err(|e| io_err(&target, e))?;
            sync_dir(saves);
            journal.write("original_moved")?;
        }
        activate(&staged_dir, &target).map_err(|e| io_err(&staged_dir, e))?;
        sync_dir(saves);
        journal.write("activated")
    })();
    if let Err(e) = swapped {
        // Undone at once: the world must not wait aside for a next operation that may not come.
        if let Err(undo) = recover(saves, world) {
            tracing::warn!("the failed restore of {world} could not be undone yet: {}", undo.detail);
        }
        return Err(e);
    }
    if journal.had_original
        && let Err(e) = fs::remove_dir_all(&previous_dir)
    {
        // The journal stays: the next operation finishes the cleanup.
        tracing::warn!("the replaced world {} stays: {e}", previous_dir.display());
        return Ok(());
    }
    fs::remove_file(&journal.path).map_err(|e| io_err(&journal.path, e))
}

fn unpack<R: Read + io::Seek>(
    zip: &mut ZipArchive<R>,
    entries: &[(usize, PathBuf, u64)],
    into: &Path,
) -> AppResult<()> {
    for (index, rel, size) in entries {
        let dest = into.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let mut entry = zip.by_index(*index).map_err(|e| invalid(e.to_string()))?;
        let mut out = File::create(&dest).map_err(|e| io_err(&dest, e))?;
        let written = io::copy(&mut (&mut entry).take(size + 1), &mut out).map_err(|e| io_err(&dest, e))?;
        if written != *size {
            return Err(invalid(format!("{} is not {size} bytes", rel.display())));
        }
    }
    Ok(())
}

/// Finishes or undoes a restore of `world` a crash cut short; without a journal
/// there is nothing to do.
pub fn recover(saves: &Path, world: &str) -> AppResult<()> {
    let path = journal_path(saves, world);
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read(&path).map_err(|e| io_err(&path, e))?;
    let journal: Value =
        serde_json::from_slice(&raw).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let text = |key: &str| journal.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let (staged, previous) = (text("staged_name"), text("previous_name"));
    let ours = staged.starts_with(&format!(".{world}.restore-"))
        && !staged.contains(['/', '\\'])
        && previous == format!("{staged}.previous")
        && text("target_name") == world;
    if !ours {
        return Err(invalid(format!("{} names folders that are not its own", path.display())));
    }
    let had_original = journal.get("had_original").and_then(Value::as_bool).unwrap_or(true);
    let (staged, previous, target) = (saves.join(staged), saves.join(previous), saves.join(world));
    let lost =
        || AppError::new(ErrorCode::Io, format!("the world {world} was lost in an interrupted restore"));
    match (staged.exists(), previous.exists(), target.exists()) {
        (true, true, true) => return Err(lost()),
        (true, true, false) => {
            rename_retrying(&previous, &target).map_err(|e| io_err(&previous, e))?;
            fs::remove_dir_all(&staged).map_err(|e| io_err(&staged, e))?;
        }
        (true, false, target_there) => {
            if had_original && !target_there {
                return Err(lost());
            }
            fs::remove_dir_all(&staged).map_err(|e| io_err(&staged, e))?;
        }
        (false, previous_there, true) => {
            if previous_there {
                fs::remove_dir_all(&previous).map_err(|e| io_err(&previous, e))?;
            }
        }
        (false, true, false) => rename_retrying(&previous, &target).map_err(|e| io_err(&previous, e))?,
        (false, false, false) => {
            if had_original {
                return Err(lost());
            }
        }
    }
    sync_dir(saves);
    fs::remove_file(&path).map_err(|e| io_err(&path, e))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;

    fn saves_with(world: &str, level: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("saves");
        fs::create_dir_all(saves.join(world)).unwrap();
        fs::write(saves.join(world).join("level.dat"), level).unwrap();
        (tmp, saves)
    }

    fn zip_of(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path =
            dir.join(format!("backup-{}.zip", entries.len() * 7 + entries.first().map_or(0, |e| e.0.len())));
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for (name, body) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    /// What `saves` holds that the launcher made: hidden folders and files.
    fn leftovers(saves: &Path) -> Vec<String> {
        fs::read_dir(saves)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect()
    }

    /// The state a crash leaves in `phase` of restoring `world` from a copy whose level is `new`.
    fn cut_short(saves: &Path, world: &str, phase: &str) {
        let staged = format!(".{world}.restore-test");
        let previous = format!("{staged}.previous");
        let unpacked = |at: &Path| {
            fs::create_dir_all(at).unwrap();
            fs::write(at.join("level.dat"), b"new").unwrap();
        };
        match phase {
            "prepared" => unpacked(&saves.join(&staged)),
            "original_moved" => {
                unpacked(&saves.join(&staged));
                fs::rename(saves.join(world), saves.join(&previous)).unwrap();
            }
            "activated" => {
                fs::rename(saves.join(world), saves.join(&previous)).unwrap();
                unpacked(&saves.join(world));
            }
            other => panic!("{other}"),
        }
        fs::write(
            saves.join(format!(".{world}-world-restore.json")),
            json!({"schema_version": 1, "phase": phase, "target_name": world, "staged_name": staged,
                   "previous_name": previous, "had_original": true})
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_restore_replaces_the_world_with_the_archive_s() {
        let (tmp, saves) = saves_with("W", b"old");
        let archive = zip_of(tmp.path(), &[("level.dat", b"new"), ("region/r.0.0.mca", b"r")]);
        restore(&archive, &saves, "W").unwrap();
        assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"new");
        assert!(saves.join("W/region/r.0.0.mca").is_file());
        assert_eq!(leftovers(&saves), Vec::<String>::new(), "no staged folder, previous folder or journal");
    }

    #[test]
    fn a_world_that_was_deleted_is_restored_too() {
        let (tmp, saves) = saves_with("W", b"old");
        fs::remove_dir_all(saves.join("W")).unwrap();
        let archive = zip_of(tmp.path(), &[("level.dat", b"new")]);
        restore(&archive, &saves, "W").unwrap();
        assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"new");
    }

    #[test]
    fn a_hostile_or_empty_archive_never_replaces_the_world() {
        let (tmp, saves) = saves_with("W", b"old");
        for entries in [
            vec![("../evil", b"x" as &[u8]), ("level.dat", b"new")],
            vec![("/abs", b"x" as &[u8]), ("level.dat", b"new")],
            vec![("region/r.mca", b"x" as &[u8])],
        ] {
            let archive = zip_of(tmp.path(), &entries);
            assert!(restore(&archive, &saves, "W").is_err(), "{entries:?}");
            assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"old");
            assert!(!tmp.path().join("evil").exists());
            assert_eq!(leftovers(&saves), Vec::<String>::new());
        }
        assert!(restore(&zip_of(tmp.path(), &[("level.dat", b"n")]), &saves, "../W").is_err());
    }

    #[test]
    fn a_failed_swap_puts_the_world_back_at_once() {
        let (tmp, saves) = saves_with("W", b"old");
        let archive = zip_of(tmp.path(), &[("level.dat", b"new")]);
        FAIL_ACTIVATION.with(|f| f.set(true));
        let failed = restore(&archive, &saves, "W");
        FAIL_ACTIVATION.with(|f| f.set(false));
        assert!(failed.is_err());
        assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"old", "the world is back in place");
        assert_eq!(leftovers(&saves), Vec::<String>::new(), "no staged or previous folder, no journal");
    }

    #[test]
    fn a_copy_a_crash_left_half_unpacked_goes() {
        // Left by a crash while unpacking (before the journal came first): a world-sized folder no
        // one lists. The world beside it, and an original moved aside, are never touched.
        let (_tmp, saves) = saves_with("W", b"old");
        fs::create_dir_all(saves.join(".W.restore-0000abcd/region")).unwrap();
        fs::write(saves.join(".W.restore-0000abcd/region/r.0.0.mca"), b"half").unwrap();
        fs::create_dir_all(saves.join(".W.restore-1.previous")).unwrap();
        assert!(recover_all(&saves).is_empty());
        assert!(!saves.join(".W.restore-0000abcd").exists());
        assert!(saves.join(".W.restore-1.previous").exists(), "an original is never removed blindly");
        assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"old");
    }

    #[test]
    fn every_cut_restore_of_saves_is_finished_in_one_go() {
        let (_tmp, saves) = saves_with("W", b"old");
        fs::create_dir_all(saves.join("V")).unwrap();
        fs::write(saves.join("V/level.dat"), b"v-old").unwrap();
        cut_short(&saves, "W", "original_moved");
        cut_short(&saves, "V", "activated");
        assert!(recover_all(&saves).is_empty());
        assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), b"old");
        assert_eq!(fs::read(saves.join("V/level.dat")).unwrap(), b"new");
        assert_eq!(leftovers(&saves), Vec::<String>::new());
    }

    #[test]
    fn an_interrupted_restore_is_finished_or_undone_by_the_next_operation() {
        for (phase, expected) in
            [("prepared", b"old" as &[u8]), ("original_moved", b"old"), ("activated", b"new")]
        {
            let (_tmp, saves) = saves_with("W", b"old");
            cut_short(&saves, "W", phase);
            recover(&saves, "W").unwrap();
            assert_eq!(fs::read(saves.join("W/level.dat")).unwrap(), expected, "{phase}");
            assert_eq!(leftovers(&saves), Vec::<String>::new(), "{phase}");
        }
    }
}
