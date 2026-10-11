//! The launcher's own files moved, once, to where they live now: every backup in its build's
//! folder of the backups folder (`crate::backups`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};

use crate::backups::{DIR_KEY, LAYOUT_KEY, default_root, old_default};
use crate::lock::path_key;
use crate::storage::atomic::rename_retrying;
use crate::storage::config::ConfigStore;

/// The layout `LAYOUT_KEY` names once every world backup sits in its build's `worlds/`.
const LAYOUT: u64 = 2;
/// Where the old default folder waits while its builds move out of it.
const OLD_WORLDS: &str = ".layout-worlds";
/// The prefix a build's folder in a custom backups folder takes while its worlds move down a level.
const MOVING: &str = ".layout-";

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `dir` is a build's folder of world backups as the backups module writes them: a world's folder
/// holding a zip's metadata. A folder the player keeps something else in is not one.
fn holds_world_backups(dir: &Path) -> bool {
    folders(dir).iter().any(|world| {
        fs::read_dir(world).into_iter().flatten().flatten().any(|file| {
            name_of(&file.path()).ends_with(".zip.json")
                && fs::read(file.path())
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                    .is_some_and(|meta| meta.get("world_folder").is_some())
        })
    })
}

/// Moves the worlds in `from` into `to` (a build's `worlds/`): the whole folder when `to` is not
/// there yet, else each world `to` lacks (what an interrupted move left). How many worlds moved.
fn move_worlds(from: &Path, to: &Path) -> AppResult<usize> {
    let worlds = folders(from);
    if !to.exists() {
        let parent = to.parent().unwrap_or(to);
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        rename_retrying(from, to).map_err(|e| io_error(from, e))?;
        return Ok(worlds.len());
    }
    let mut moved = 0;
    for world in worlds {
        let dest = to.join(name_of(&world));
        if !dest.exists() {
            rename_retrying(&world, &dest).map_err(|e| io_error(&world, e))?;
            moved += 1;
        }
    }
    let _ = fs::remove_dir(from);
    Ok(moved)
}

/// Moves `entries` into `dir`, keeping their names (made first when missing).
fn put_back(entries: &[PathBuf], dir: &Path) -> AppResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    for entry in entries {
        let dest = dir.join(name_of(entry));
        if !dest.exists() {
            rename_retrying(entry, &dest).map_err(|e| io_error(entry, e))?;
        }
    }
    Ok(())
}

/// Copies `from` (a file, or a folder with everything in it) to `to`.
fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)?.flatten() {
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Moves `from` (a file or a folder) to `to`, its folder made first: a rename, or across drives
/// a copy, with `from` removed only once the copy is whole (a copy cut short is removed instead).
pub fn move_dir(from: &Path, to: &Path) -> io::Result<()> {
    move_with(from, to, &copy_all)
}

fn move_with(from: &Path, to: &Path, copy: &dyn Fn(&Path, &Path) -> io::Result<()>) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match rename_retrying(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(e) = copy(from, to) {
                let _ = remove_any(to);
                return Err(e);
            }
            remove_any(from)
        }
        other => other,
    }
}

/// The mods' backups a build kept in `mods/.backups/` move to `to` (its build's `mods/` of the
/// backups folder); the folder goes with what was left in it. How many moved.
pub fn move_mod_backups(game: &Path, to: &Path) -> AppResult<usize> {
    let legacy = game.join("mods").join(crate::content::backups::BACKUPS);
    let Ok(entries) = fs::read_dir(&legacy) else { return Ok(0) };
    let mut moved = 0;
    for entry in entries.flatten() {
        let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
        let dest = to.join(&name);
        if name.ends_with(".backup") && entry.file_type().is_ok_and(|t| t.is_file()) && !dest.exists() {
            move_dir(&path, &dest).map_err(|e| io_error(&path, e))?;
            moved += 1;
        }
    }
    // A part of a copy cut short, or a backup the new place has already.
    fs::remove_dir_all(&legacy).map_err(|e| io_error(&legacy, e))?;
    Ok(moved)
}

/// The copies a modpack update kept in `.launcher/pack-backups/<time>/` move to `to` (its
/// build's `modpacks/` of the backups folder). How many updates' copies moved.
pub fn move_pack_backups(game: &Path, to: &Path) -> AppResult<usize> {
    let legacy = game.join(".launcher").join("pack-backups");
    let mut moved = 0;
    for update in folders(&legacy) {
        let dest = to.join(name_of(&update));
        if dest.exists() {
            // Moved before, by a move cut short.
            fs::remove_dir_all(&update).map_err(|e| io_error(&update, e))?;
        } else {
            move_dir(&update, &dest).map_err(|e| io_error(&update, e))?;
            moved += 1;
        }
    }
    let _ = fs::remove_dir(&legacy);
    Ok(moved)
}

/// World backups from `<old root>/<build>/<world>/` to `<root>/<build>/worlds/<world>/`, once.
/// With the default folder (or the old default the wizard saved) they leave
/// `<minecraft>/backups/worlds` for `<minecraft>/backups`; a folder of the player's own keeps its
/// place. Only builds' folders of world backups move. How many worlds moved.
pub fn move_world_backups(config: &ConfigStore, mc_dir: &Path) -> AppResult<usize> {
    if config.get(LAYOUT_KEY).and_then(|v| v.as_u64()).is_some_and(|n| n >= LAYOUT) {
        return Ok(0);
    }
    let setting = config.get_str(DIR_KEY).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let names_old_default =
        setting.as_deref().is_some_and(|s| path_key(Path::new(s)) == path_key(&old_default(mc_dir)));
    let mut moved = 0;
    match setting {
        Some(custom) if !names_old_default => {
            let root = PathBuf::from(custom);
            // A build's folder moves aside first: one of its worlds may be named `worlds`.
            for build in folders(&root) {
                let name = name_of(&build);
                if name.starts_with('.') || !holds_world_backups(&build) {
                    continue;
                }
                let aside = root.join(format!("{MOVING}{name}"));
                rename_retrying(&build, &aside).map_err(|e| io_error(&build, e))?;
            }
            for aside in folders(&root) {
                let Some(name) = name_of(&aside).strip_prefix(MOVING).map(str::to_string) else { continue };
                moved += move_worlds(&aside, &root.join(name).join("worlds"))?;
            }
        }
        _ => {
            let (root, old) = (default_root(mc_dir), old_default(mc_dir));
            let waiting = root.join(OLD_WORLDS);
            if old.is_dir() {
                if waiting.exists() {
                    put_back(
                        &fs::read_dir(&old)
                            .map_err(|e| io_error(&old, e))?
                            .flatten()
                            .map(|e| e.path())
                            .collect::<Vec<_>>(),
                        &waiting,
                    )?;
                    let _ = fs::remove_dir(&old);
                } else {
                    rename_retrying(&old, &waiting).map_err(|e| io_error(&old, e))?;
                }
            }
            for build in folders(&waiting) {
                if holds_world_backups(&build) {
                    moved += move_worlds(&build, &root.join(name_of(&build)).join("worlds"))?;
                }
            }
            // What is not a build's backups goes back where it was.
            let left: Vec<PathBuf> =
                fs::read_dir(&waiting).into_iter().flatten().flatten().map(|e| e.path()).collect();
            put_back(&left, &old)?;
            let _ = fs::remove_dir(&waiting);
            if names_old_default {
                config.delete(DIR_KEY).map_err(|e| io_error(&root, e))?;
            }
        }
    }
    config.set(LAYOUT_KEY, json!(LAYOUT)).map_err(|e| io_error(mc_dir, e))?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::backups::{DIR_KEY, LAYOUT_KEY, default_root, old_default};
    use crate::storage::config::ConfigStore;

    /// A world backup as the backups module writes it: a zip beside its metadata.
    fn backup(dir: &Path, build: &str, world: &str) {
        let folder = dir.join(build).join(world);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.zip"), b"zip").unwrap();
        let meta = json!({"schema": 1, "kind": "manual", "version_id": build, "world_folder": world,
                          "zip_name": "a.zip"});
        fs::write(folder.join("a.zip.json"), meta.to_string()).unwrap();
    }

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, ConfigStore) {
        let tmp = tempfile::tempdir().unwrap();
        let mc = tmp.path().join("minecraft");
        let config = ConfigStore::open(tmp.path().join("config.json"));
        (tmp, mc, config)
    }

    #[test]
    fn world_backups_move_under_worlds_with_the_default() {
        let (_tmp, mc, config) = setup();
        backup(&old_default(&mc), "aero", "World");
        backup(&old_default(&mc), "aero", "Other");
        backup(&old_default(&mc), "zeta", "World");
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 3);
        let root = default_root(&mc);
        assert!(
            root.join("aero/worlds/World/a.zip").is_file()
                && root.join("aero/worlds/Other/a.zip.json").is_file()
        );
        assert!(root.join("zeta/worlds/World/a.zip").is_file());
        assert!(!old_default(&mc).join("aero").exists(), "nothing is left in the old folder");
        assert_eq!(config.get(LAYOUT_KEY), Some(json!(2)));
    }

    #[test]
    fn the_old_default_saved_counts_as_the_default() {
        let (_tmp, mc, config) = setup();
        config.set(DIR_KEY, json!(old_default(&mc).to_string_lossy())).unwrap();
        backup(&old_default(&mc), "aero", "World");
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 1);
        assert!(default_root(&mc).join("aero/worlds/World/a.zip").is_file());
        assert!(!config.contains(DIR_KEY), "the old default is no longer named");
    }

    #[test]
    fn a_custom_root_keeps_its_place_and_its_other_folders() {
        let (tmp, mc, config) = setup();
        let custom = tmp.path().join("Мої бекапи");
        config.set(DIR_KEY, json!(custom.to_string_lossy())).unwrap();
        backup(&custom, "aero", "World");
        fs::create_dir_all(custom.join("notes/2026")).unwrap();
        fs::write(custom.join("notes/2026/todo.txt"), b"mine").unwrap();
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 1);
        assert!(custom.join("aero/worlds/World/a.zip").is_file());
        assert!(custom.join("notes/2026/todo.txt").is_file(), "a folder holding no backups stays as it is");
        assert!(!custom.join("notes/worlds").exists());
        assert!(config.contains(DIR_KEY));
    }

    #[test]
    fn worlds_and_builds_named_like_the_new_folders_move_like_any_other() {
        let (_tmp, mc, config) = setup();
        backup(&old_default(&mc), "aero", "mods");
        backup(&old_default(&mc), "aero", "worlds");
        backup(&old_default(&mc), "worlds", "World");
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 3);
        let root = default_root(&mc);
        assert!(root.join("aero/worlds/mods/a.zip").is_file());
        assert!(root.join("aero/worlds/worlds/a.zip").is_file());
        assert!(root.join("worlds/worlds/World/a.zip").is_file(), "a build named worlds");
    }

    #[test]
    fn legacy_mod_backups_move_and_their_folder_goes() {
        let tmp = tempfile::tempdir().unwrap();
        let (game, to) = (tmp.path().join("game"), tmp.path().join("backups/aero/mods"));
        let legacy = game.join("mods").join(".backups");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("sodium.jar.backup"), b"old sodium").unwrap();
        fs::write(legacy.join("iris.jar.backup.part"), b"half").unwrap();
        fs::write(game.join("mods/sodium.jar"), b"new sodium").unwrap();
        assert_eq!(move_mod_backups(&game, &to).unwrap(), 1);
        assert_eq!(fs::read(to.join("sodium.jar.backup")).unwrap(), b"old sodium");
        assert!(!legacy.exists(), "the build keeps no backups of its own");
        assert!(game.join("mods/sodium.jar").is_file());
        assert_eq!(move_mod_backups(&game, &to).unwrap(), 0, "nothing left to move");
    }

    #[test]
    fn legacy_pack_backups_move() {
        let tmp = tempfile::tempdir().unwrap();
        let (game, to) = (tmp.path().join("game"), tmp.path().join("backups/aero/modpacks"));
        let update = game.join(".launcher/pack-backups/2026-10-01_10-00-00");
        fs::create_dir_all(update.join("config")).unwrap();
        fs::write(update.join("config/a.toml"), b"mine").unwrap();
        fs::write(game.join(".launcher/modrinth-pack.json"), b"{}").unwrap();
        assert_eq!(move_pack_backups(&game, &to).unwrap(), 1);
        assert_eq!(fs::read(to.join("2026-10-01_10-00-00/config/a.toml")).unwrap(), b"mine");
        assert!(!game.join(".launcher/pack-backups").exists());
        assert!(game.join(".launcher/modrinth-pack.json").is_file(), "the pack's note stays");
    }

    #[test]
    fn the_move_runs_once() {
        let (_tmp, mc, config) = setup();
        backup(&old_default(&mc), "aero", "World");
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 1);
        backup(&old_default(&mc), "late", "World");
        assert_eq!(move_world_backups(&config, &mc).unwrap(), 0, "the layout is the new one already");
        assert!(old_default(&mc).join("late/World/a.zip").is_file());
    }
}
