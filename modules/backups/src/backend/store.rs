//! Where a world's backups live and what they are: zips under
//! `<root>/<build folder>/<world folder>/`, each with its metadata beside it.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use launcher_core::net::preflight::{SpaceRequest, preflight};
use launcher_core::storage::json::write_json_file;
use launcher_core::storage::versions::Build;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;

use crate::dto::{BackupDto, Kind};

const LEVEL: &str = "level.dat";
const LOCK: &str = "session.lock";
const SCHEMA: u64 = 1;
const RESERVE: u64 = 4 * 1024 * 1024;

fn io_err(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

/// A name that is safe as one folder or file name: runs of characters outside `A-Za-z0-9._ -`
/// become `_`, spaces and dots go from the ends; nothing left is `item`.
pub fn safe_name(raw: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-') {
            out.push(c);
            replaced = false;
        } else if !replaced {
            out.push('_');
            replaced = true;
        }
    }
    let trimmed = out.trim_matches([' ', '.']);
    if trimmed.is_empty() { "item".to_string() } else { trimmed.to_string() }
}

/// The folder a build's backups go to: its version id, else its id or name.
pub fn build_folder(build: &Build) -> String {
    let named = [&build.version_id, &build.id, &build.name].into_iter().find(|s| !s.trim().is_empty());
    safe_name(named.map_or("version", |s| s.as_str()))
}

/// Milliseconds since the epoch: what file times are compared at (a stored float comes back from
/// JSON within a unit of its last place, not always exactly).
fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Unix seconds as the metadata keeps them, to the millisecond.
fn seconds(time: SystemTime) -> f64 {
    millis(time) as f64 / 1000.0
}

/// The worlds of the game folder `game`: folders of `saves/` with a `level.dat`, by name
/// whatever the case, each with its size (`session.lock` left out) and the time of its
/// `level.dat`.
pub fn worlds(game: &Path) -> io::Result<Vec<(String, u64, SystemTime)>> {
    let saves = game.join("saves");
    let Ok(entries) = fs::read_dir(&saves) else { return Ok(Vec::new()) };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        // The launcher's own folders (a restore underway) are no worlds.
        if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let level = entry.path().join(LEVEL);
        let Ok(meta) = fs::metadata(&level) else { continue };
        if !meta.is_file() {
            continue;
        }
        let size = files_of(&entry.path())?.iter().map(|(_, size)| size).sum();
        found.push((entry.file_name().to_string_lossy().into_owned(), size, meta.modified()?));
    }
    found.sort_by_key(|w| w.0.to_lowercase());
    Ok(found)
}

/// The files of `dir` a backup takes, as `/` paths relative to it with their sizes: no
/// `session.lock`, no links.
fn files_of(dir: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if rel == LOCK {
                continue;
            }
            files.push((rel, entry.metadata()?.len()));
        }
    }
    files.sort();
    Ok(files)
}

/// A backup's zip name without `.zip`: its time, `[Auto] ` before an automatic one's.
fn zip_stem(kind: Kind, now: DateTime<Utc>) -> String {
    let stamp = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    match kind {
        Kind::Auto => format!("[Auto] {stamp}"),
        Kind::Manual => stamp,
    }
}

/// `name` is one plain `.zip` file name.
fn plain_zip(name: &str) -> bool {
    name.len() > 4 && name.ends_with(".zip") && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    pub fn world_dir(&self, build_folder: &str, world: &str) -> PathBuf {
        self.root.join(safe_name(build_folder)).join(safe_name(world))
    }

    /// Backs world `world` of the game folder `game` up: a zip beside its metadata, written under
    /// a temporary name first; nothing is left of a failed one.
    pub fn create(
        &self,
        build: &Build,
        game: &Path,
        world: &str,
        kind: Kind,
        now: DateTime<Utc>,
    ) -> AppResult<BackupDto> {
        let source = game.join("saves").join(world);
        let level = source.join(LEVEL);
        let modified = fs::metadata(&level).and_then(|m| m.modified()).map_err(|e| io_err(&level, e))?;
        let dir = self.world_dir(&build_folder(build), world);
        if dir.starts_with(&source) {
            return Err(AppError::new(
                ErrorCode::InvalidDirectoryPath,
                "the backups folder is inside the world",
            )
            .with_param("path", dir.to_string_lossy()));
        }
        // Half-written archives an interrupted backup left (the build's folder is held: none is
        // being written now).
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".zip.tmp") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let files = files_of(&source).map_err(|e| io_err(&source, e))?;
        let bytes: u64 = files.iter().map(|(_, size)| size).sum();
        preflight(&[SpaceRequest {
            dir: dir.clone(),
            bytes: bytes.saturating_add(RESERVE),
            label: world.to_string(),
        }])?;
        let stem = zip_stem(kind, now);
        let zip_name = (0..1000)
            .map(|n| if n == 0 { format!("{stem}.zip") } else { format!("{stem}-{n}.zip") })
            .find(|name| !dir.join(name).exists() && !dir.join(format!("{name}.json")).exists())
            .ok_or_else(|| {
                AppError::new(ErrorCode::Io, "no free backup name").with_param("path", dir.to_string_lossy())
            })?;
        let (zip_path, meta_path, temp) =
            (dir.join(&zip_name), dir.join(format!("{zip_name}.json")), dir.join(format!("{zip_name}.tmp")));
        let written = (|| -> AppResult<()> {
            write_zip(&source, &files, &temp)?;
            fs::rename(&temp, &zip_path).map_err(|e| io_err(&zip_path, e))?;
            let meta = json!({
                "schema": SCHEMA,
                "kind": match kind { Kind::Auto => "auto", Kind::Manual => "manual" },
                "version_id": build.version_id,
                "version_name": build.name,
                "world_folder": world,
                "world_name": world,
                "source_path": source.to_string_lossy(),
                "source_modified_at": seconds(modified),
                "created_at": now.to_rfc3339(),
                "created_timestamp": now.timestamp_millis() as f64 / 1000.0,
                "zip_name": zip_name,
            });
            write_json_file(&meta_path, &meta, 2).map_err(|e| io_err(&meta_path, e))
        })();
        if let Err(e) = written {
            for path in [&temp, &zip_path, &meta_path] {
                let _ = fs::remove_file(path);
            }
            return Err(e);
        }
        let size = fs::metadata(&zip_path).map_or(0, |m| m.len());
        Ok(BackupDto { zip_name, kind, created: now.timestamp(), size })
    }

    /// The world's backups with valid metadata, the newest first.
    pub fn list(&self, build_folder: &str, world: &str) -> Vec<BackupDto> {
        let mut found: Vec<(f64, BackupDto)> = self
            .metadata(build_folder, world)
            .into_iter()
            .map(|(meta, dto)| (meta.get("created_timestamp").and_then(Value::as_f64).unwrap_or(0.0), dto))
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.into_iter().map(|(_, dto)| dto).collect()
    }

    /// Each backup's metadata and what it says of it.
    fn metadata(&self, build_folder: &str, world: &str) -> Vec<(Value, BackupDto)> {
        let dir = self.world_dir(build_folder, world);
        let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !plain_zip(&name) {
                continue;
            }
            let Ok(raw) = fs::read(dir.join(format!("{name}.json"))) else { continue };
            let Ok(meta) = serde_json::from_slice::<Value>(&raw) else { continue };
            let kind = match meta.get("kind").and_then(Value::as_str) {
                Some("auto") => Kind::Auto,
                Some("manual") => Kind::Manual,
                _ => continue,
            };
            // Worlds whose names make the same folder name share the folder; the metadata says whose
            // backup it is.
            if meta.get("schema").and_then(Value::as_u64) != Some(SCHEMA)
                || meta.get("zip_name").and_then(Value::as_str) != Some(name.as_str())
                || meta.get("world_folder").and_then(Value::as_str) != Some(world)
            {
                continue;
            }
            let created = meta.get("created_timestamp").and_then(Value::as_f64).unwrap_or(0.0) as i64;
            let size = entry.metadata().map_or(0, |m| m.len());
            found.push((meta, BackupDto { zip_name: name, kind, created, size }));
        }
        found
    }

    /// Whether the world changed since its newest automatic backup (or has none); metadata
    /// without the world's time counts from the backup's own.
    pub fn needs_auto(&self, build_folder: &str, world: &str, level_modified: SystemTime) -> bool {
        let newest = self
            .metadata(build_folder, world)
            .into_iter()
            .filter(|(_, dto)| dto.kind == Kind::Auto)
            .map(|(meta, _)| {
                let created = meta.get("created_timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                let source = meta.get("source_modified_at").and_then(Value::as_f64);
                (created, source.unwrap_or(created))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        match newest {
            None => true,
            Some((_, source)) => ((source * 1000.0).round() as i64) < millis(level_modified),
        }
    }

    /// Drops all but the `keep` newest automatic backups of the world; manual ones stay.
    pub fn prune_auto(&self, build_folder: &str, world: &str, keep: u32) -> AppResult<usize> {
        let autos: Vec<BackupDto> =
            self.list(build_folder, world).into_iter().filter(|b| b.kind == Kind::Auto).collect();
        let mut dropped = 0;
        for old in autos.iter().skip(keep.max(1) as usize) {
            self.delete(build_folder, world, &old.zip_name)?;
            dropped += 1;
        }
        Ok(dropped)
    }

    /// The zip of backup `zip_name`, when that is a backup's plain name.
    pub fn zip_path(&self, build_folder: &str, world: &str, zip_name: &str) -> AppResult<PathBuf> {
        if !plain_zip(zip_name) {
            return Err(
                AppError::new(ErrorCode::InvalidInput, "not a backup's name").with_param("name", zip_name)
            );
        }
        Ok(self.world_dir(build_folder, world).join(zip_name))
    }

    /// Deletes one backup and its metadata.
    pub fn delete(&self, build_folder: &str, world: &str, zip_name: &str) -> AppResult<()> {
        let zip = self.zip_path(build_folder, world, zip_name)?;
        fs::remove_file(&zip).map_err(|e| io_err(&zip, e))?;
        let _ = fs::remove_file(zip.with_file_name(format!("{zip_name}.json")));
        Ok(())
    }

    /// Deletes every backup of `build`: in its folder, the zips whose metadata says they are its
    /// backups, with that metadata, then the folders left empty. Nothing else there is touched:
    /// the backups folder may be one the player keeps other things in.
    pub fn delete_build(&self, build: &Build) -> AppResult<()> {
        let dir = self.root.join(build_folder(build));
        let Ok(worlds) = fs::read_dir(&dir) else { return Ok(()) };
        for world in worlds.flatten() {
            if !world.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let world_dir = world.path();
            let folder = world.file_name().to_string_lossy().into_owned();
            let Ok(entries) = fs::read_dir(&world_dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !plain_zip(&name) || !entry.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                let meta_path = world_dir.join(format!("{name}.json"));
                let meta =
                    fs::read(&meta_path).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
                if !meta.is_some_and(|meta| backup_of(&meta, build, &folder, &name)) {
                    continue;
                }
                let zip = world_dir.join(&name);
                fs::remove_file(&zip).map_err(|e| io_err(&zip, e))?;
                let _ = fs::remove_file(&meta_path);
            }
            // Only an empty folder goes.
            let _ = fs::remove_dir(&world_dir);
        }
        let _ = fs::remove_dir(&dir);
        Ok(())
    }
}

/// `meta` is the metadata of `build`'s backup `zip_name` of the world whose folder is `folder`.
fn backup_of(meta: &Value, build: &Build, folder: &str, zip_name: &str) -> bool {
    let text = |key: &str| meta.get(key).and_then(Value::as_str);
    meta.get("schema").and_then(Value::as_u64) == Some(SCHEMA)
        && matches!(text("kind"), Some("auto" | "manual"))
        && text("zip_name") == Some(zip_name)
        && text("version_id") == Some(build.version_id.as_str())
        && text("world_folder").is_some_and(|world| safe_name(world) == folder)
}

/// `files` of `source` into a zip at `target` (deflate, level 1); temporary files stay out.
fn write_zip(source: &Path, files: &[(String, u64)], target: &Path) -> AppResult<()> {
    let out = File::create(target).map_err(|e| io_err(target, e))?;
    let mut zip = zip::ZipWriter::new(io::BufWriter::new(out));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(1))
        .large_file(true);
    let mut buf = vec![0u8; 256 * 1024];
    for (rel, _) in files.iter().filter(|(rel, _)| !rel.ends_with(".tmp")) {
        let path = source.join(rel);
        let mut file = File::open(&path).map_err(|e| io_err(&path, e))?;
        zip.start_file(rel.as_str(), options).map_err(|e| io_err(target, io::Error::other(e)))?;
        loop {
            let n = file.read(&mut buf).map_err(|e| io_err(&path, e))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n]).map_err(|e| io_err(target, e))?;
        }
    }
    let writer = zip.finish().map_err(|e| io_err(target, io::Error::other(e)))?;
    let file = writer.into_inner().map_err(|e| io_err(target, e.into_error()))?;
    file.sync_all().map_err(|e| io_err(target, e))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::{TimeZone, Utc};
    use launcher_core::storage::versions::Build;
    use serde_json::json;

    use super::*;
    use crate::dto::Kind;

    fn build(key: &str) -> Build {
        Build::new(key)
    }

    fn world(game: &Path, name: &str) -> PathBuf {
        let dir = game.join("saves").join(name);
        fs::create_dir_all(dir.join("region")).unwrap();
        fs::write(dir.join("level.dat"), b"level").unwrap();
        fs::write(dir.join("region/r.0.0.mca"), vec![7u8; 5000]).unwrap();
        fs::write(dir.join("session.lock"), b"lock").unwrap();
        fs::write(dir.join("x.tmp"), b"tmp").unwrap();
        dir
    }

    #[test]
    fn names_are_made_safe_like_the_original() {
        assert_eq!(safe_name("Мій світ: 1/2"), "_ _ 1_2");
        assert_eq!(safe_name(" .. "), "item");
        assert_eq!(safe_name("New World-1.2"), "New World-1.2");
    }

    #[test]
    fn worlds_are_folders_with_level_dat_sized_without_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        world(tmp.path(), "b world");
        world(tmp.path(), "A world");
        fs::create_dir_all(tmp.path().join("saves/not a world")).unwrap();
        let found = worlds(tmp.path()).unwrap();
        assert_eq!(found.iter().map(|w| w.0.as_str()).collect::<Vec<_>>(), vec!["A world", "b world"]);
        assert_eq!(found[0].1, 5 + 5000 + 3, "level.dat, the region and x.tmp; not session.lock");
        assert!(worlds(&tmp.path().join("none")).unwrap().is_empty());
    }

    #[test]
    fn the_launcher_s_own_folders_are_not_worlds() {
        let tmp = tempfile::tempdir().unwrap();
        world(tmp.path(), "W");
        world(tmp.path(), ".W.restore-1234");
        let found = worlds(tmp.path()).unwrap();
        assert_eq!(found.iter().map(|w| w.0.as_str()).collect::<Vec<_>>(), vec!["W"]);
    }

    #[test]
    fn a_half_written_archive_an_interrupted_backup_left_goes() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        world(&game, "World");
        let store = Store::new(tmp.path().join("backups"));
        let dir = store.world_dir("my_build", "World");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-01-01_00-00-00.zip.tmp"), b"half").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 10, 20, 30).unwrap();
        store.create(&build("my_build"), &game, "World", Kind::Manual, now).unwrap();
        assert!(!dir.join("2026-01-01_00-00-00.zip.tmp").exists());
    }

    #[test]
    fn a_backup_is_a_zip_of_the_world_with_its_metadata_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        world(&game, "World");
        let store = Store::new(tmp.path().join("backups"));
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 10, 20, 30).unwrap();
        let made = store.create(&build("my_build"), &game, "World", Kind::Manual, now).unwrap();
        assert_eq!(made.zip_name, "2026-09-29_10-20-30.zip");
        let dir = store.world_dir("my_build", "World");
        let mut zip = zip::ZipArchive::new(fs::File::open(dir.join(&made.zip_name)).unwrap()).unwrap();
        let mut names: Vec<String> =
            (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["level.dat", "region/r.0.0.mca"]);
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("2026-09-29_10-20-30.zip.json")).unwrap()).unwrap();
        assert_eq!(
            (meta["schema"].clone(), meta["kind"].clone(), meta["world_folder"].clone()),
            (json!(1), json!("manual"), json!("World"))
        );
        let again = store.create(&build("my_build"), &game, "World", Kind::Auto, now).unwrap();
        assert_eq!(again.zip_name, "[Auto] 2026-09-29_10-20-30.zip");
        let third = store.create(&build("my_build"), &game, "World", Kind::Manual, now).unwrap();
        assert_eq!(third.zip_name, "2026-09-29_10-20-30-1.zip", "a taken name gets a number");
        assert_eq!(store.list("my_build", "World").len(), 3);
        assert!(
            !fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn worlds_whose_names_share_a_folder_keep_their_own_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        world(&game, "Тест");
        world(&game, "Хардкор");
        let store = Store::new(tmp.path().join("backups"));
        assert_eq!(
            store.world_dir("b", "Тест"),
            store.world_dir("b", "Хардкор"),
            "the original's folder names"
        );
        store.create(&build("b"), &game, "Тест", Kind::Auto, Utc::now()).unwrap();
        assert!(store.list("b", "Хардкор").is_empty(), "another world's backup is not this one's");
        let modified = fs::metadata(game.join("saves/Хардкор/level.dat")).unwrap().modified().unwrap();
        assert!(store.needs_auto("b", "Хардкор", modified));
        store.create(&build("b"), &game, "Хардкор", Kind::Auto, Utc::now()).unwrap();
        assert_eq!(store.prune_auto("b", "Хардкор", 1).unwrap(), 0);
        assert_eq!((store.list("b", "Тест").len(), store.list("b", "Хардкор").len()), (1, 1));
    }

    #[test]
    fn an_unchanged_world_is_not_backed_up_again() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let dir = world(&game, "World");
        let store = Store::new(tmp.path().join("backups"));
        let modified = fs::metadata(dir.join("level.dat")).unwrap().modified().unwrap();
        assert!(store.needs_auto("b", "World", modified), "no auto backup yet");
        store.create(&build("b"), &game, "World", Kind::Auto, Utc::now()).unwrap();
        assert!(!store.needs_auto("b", "World", modified));
        assert!(store.needs_auto("b", "World", modified + std::time::Duration::from_secs(60)));
    }

    #[test]
    fn rotation_keeps_the_newest_autos_and_every_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        world(&game, "W");
        let store = Store::new(tmp.path().join("backups"));
        let at = |s: u32| Utc.with_ymd_and_hms(2026, 9, 29, 10, 0, s).unwrap();
        for s in 0..4 {
            store.create(&build("b"), &game, "W", Kind::Auto, at(s)).unwrap();
        }
        store.create(&build("b"), &game, "W", Kind::Manual, at(9)).unwrap();
        assert_eq!(store.prune_auto("b", "W", 2).unwrap(), 2);
        let left: Vec<String> = store.list("b", "W").into_iter().map(|b| b.zip_name).collect();
        assert_eq!(
            left,
            vec![
                "2026-09-29_10-00-09.zip",
                "[Auto] 2026-09-29_10-00-03.zip",
                "[Auto] 2026-09-29_10-00-02.zip"
            ]
        );
    }

    #[test]
    fn a_backup_is_deleted_with_its_metadata_and_a_build_s_backups_all_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        world(&game, "W");
        let store = Store::new(tmp.path().join("backups"));
        let made = store.create(&build("b"), &game, "W", Kind::Manual, Utc::now()).unwrap();
        store.delete("b", "W", &made.zip_name).unwrap();
        assert!(store.list("b", "W").is_empty());
        assert!(store.delete("b", "W", "../../x.zip").is_err());
        store.create(&build("b"), &game, "W", Kind::Manual, Utc::now()).unwrap();
        store.delete_build(&build("b")).unwrap();
        assert!(!tmp.path().join("backups").join("b").exists());
    }
}
