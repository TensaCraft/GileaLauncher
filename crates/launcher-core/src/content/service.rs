//! A build's installed content: the list of one kind, and switching or
//! deleting one item — never while the build's game runs or another operation holds its folder,
//! and only for a file the list has just shown.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ContentKind, ContentList, ErrorCode, LoaderKind, Text};

use super::backups::{backup_of, backups, restore};
use super::inventory::{
    Entry, MetadataCache, iris_properties, mods_supported, options_path, scan_mods, scan_resource_packs,
    scan_shader_packs,
};
use super::packs::{
    INCOMPATIBLE_RESOURCE_PACKS, RESOURCE_PACKS, legacy_pack_names, read_options_list, remove_options_entry,
    resourcepack_entry, write_options_list, write_properties,
};
use super::screenshots::{SCREENSHOTS, ScreenshotFile, list_screenshots};
use crate::feedback::FeedbackService;
use crate::launch::options::{component_id, game_dir};
use crate::lock::Coordinator;
use crate::storage::versions::{Build, VersionStore};

pub struct ContentService {
    versions: Arc<VersionStore>,
    instances: Arc<Coordinator>,
    feedback: Arc<FeedbackService>,
    cache: MetadataCache,
    /// The backups' own: they never push the listed mods out of `cache`.
    backup_cache: MetadataCache,
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

/// Renames `from` to `to`, refusing to replace anything already there (`content_file_exists`
/// names it).
fn rename_new(from: &Path, to: &Path) -> AppResult<()> {
    if fs::symlink_metadata(to).is_ok() {
        let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(AppError::new(ErrorCode::ContentConflict, format!("{} already exists", to.display()))
            .with_param("name", name));
    }
    fs::rename(from, to).map_err(|e| io_error(from, e))
}

fn sibling(entry: &Entry, name: &str) -> PathBuf {
    entry.path.with_file_name(name)
}

fn display_name(entry: &Entry) -> String {
    entry.item.name.clone().unwrap_or_else(|| entry.item.filename.clone())
}

/// A legacy `.disabled` pack gets its own name back.
fn restore_name(entry: &Entry) -> AppResult<()> {
    if entry.item.file != entry.item.filename {
        rename_new(&entry.path, &sibling(entry, &entry.item.filename))?;
    }
    Ok(())
}

fn toggle_mod(entry: &Entry) -> AppResult<bool> {
    if entry.item.enabled {
        rename_new(&entry.path, &sibling(entry, &format!("{}.disabled", entry.item.file)))?;
        Ok(false)
    } else {
        rename_new(&entry.path, &sibling(entry, &entry.item.filename))?;
        Ok(true)
    }
}

/// Drops pack `filename` from both lists of `options.txt`.
fn unlist_resource_pack(options: &Path, filename: &str, legacy: bool) -> AppResult<()> {
    let pack = resourcepack_entry(filename, legacy);
    for key in [RESOURCE_PACKS, INCOMPATIBLE_RESOURCE_PACKS] {
        remove_options_entry(options, key, &pack).map_err(|e| io_error(options, e))?;
    }
    Ok(())
}

fn toggle_resource_pack(game: &Path, entry: &Entry, legacy: bool) -> AppResult<bool> {
    let options = options_path(game);
    if entry.item.enabled {
        unlist_resource_pack(&options, &entry.item.filename, legacy)?;
        return Ok(false);
    }
    restore_name(entry)?;
    let pack = resourcepack_entry(&entry.item.filename, legacy);
    let mut listed = read_options_list(&options, RESOURCE_PACKS);
    if !listed.contains(&pack) {
        listed.push(pack);
    }
    write_options_list(&options, RESOURCE_PACKS, &listed).map_err(|e| io_error(&options, e))?;
    Ok(true)
}

fn toggle_shader_pack(game: &Path, entry: &Entry) -> AppResult<bool> {
    if !entry.item.toggle_supported {
        return Err(AppError::new(ErrorCode::Unsupported, "shader packs switch only with Iris installed"));
    }
    let properties = iris_properties(game);
    if entry.item.enabled {
        write_properties(&properties, &[("enableShaders", "false")]).map_err(|e| io_error(&properties, e))?;
        return Ok(false);
    }
    restore_name(entry)?;
    write_properties(&properties, &[("enableShaders", "true"), ("shaderPack", &entry.item.filename)])
        .map_err(|e| io_error(&properties, e))?;
    Ok(true)
}

/// Deletes the file, or the folder with its legacy `.disabled` marker.
fn remove(entry: &Entry) -> AppResult<()> {
    let result = if entry.item.folder {
        fs::remove_dir_all(&entry.path).and_then(|()| {
            match fs::remove_file(sibling(entry, &format!("{}.disabled", entry.item.filename))) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        })
    } else {
        fs::remove_file(&entry.path)
    };
    result.map_err(|e| io_error(&entry.path, e))
}

impl ContentService {
    pub fn new(
        versions: Arc<VersionStore>,
        instances: Arc<Coordinator>,
        feedback: Arc<FeedbackService>,
    ) -> ContentService {
        ContentService {
            versions,
            instances,
            feedback,
            cache: MetadataCache::default(),
            backup_cache: MetadataCache::default(),
        }
    }

    fn build(&self, key: &str) -> AppResult<Build> {
        self.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })
    }

    fn folder(&self, build: &Build) -> PathBuf {
        game_dir(build, self.versions.minecraft_dir())
    }

    fn scan(&self, build: &Build, kind: ContentKind) -> (bool, Vec<Entry>) {
        let dir = self.folder(build);
        let loader = component_id(build).and_then(LoaderKind::of_component);
        match kind {
            ContentKind::Mods if !mods_supported(build.client.as_deref()) => (false, Vec::new()),
            ContentKind::Mods => {
                let found = backups(&dir, loader, &self.backup_cache);
                (true, scan_mods(&dir, loader, &self.cache, &found))
            }
            ContentKind::ResourcePacks => {
                (true, scan_resource_packs(&dir, legacy_pack_names(build.version.as_deref())))
            }
            ContentKind::ShaderPacks => (true, scan_shader_packs(&dir, loader, &self.cache)),
        }
    }

    fn list_of(&self, build: &Build, kind: ContentKind) -> ContentList {
        let (supported, entries) = self.scan(build, kind);
        ContentList { kind, supported, items: entries.into_iter().map(|e| e.item).collect() }
    }

    /// Build `key`'s content of `kind` (blocking).
    pub fn list(&self, key: &str, kind: ContentKind) -> AppResult<ContentList> {
        Ok(self.list_of(&self.build(key)?, kind))
    }

    /// The folder of `kind` in build `key`'s game folder, created when missing.
    pub fn dir(&self, key: &str, kind: ContentKind) -> AppResult<PathBuf> {
        let dir = self.folder(&self.build(key)?).join(kind.folder());
        fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        Ok(dir)
    }

    /// Build `key`'s screenshots, newest first (blocking).
    pub fn screenshots(&self, key: &str) -> AppResult<Vec<ScreenshotFile>> {
        Ok(list_screenshots(&self.folder(&self.build(key)?)))
    }

    /// Screenshot `name` of build `key` — only one its list has.
    pub fn screenshot(&self, key: &str, name: &str) -> AppResult<ScreenshotFile> {
        self.screenshots(key)?.into_iter().find(|s| s.name == name).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("no screenshot {name}")).with_param("name", name)
        })
    }

    /// Deletes screenshot `name` (also while the game runs: nothing holds old screenshots); the
    /// list after it.
    pub fn delete_screenshot(&self, key: &str, name: &str) -> AppResult<Vec<ScreenshotFile>> {
        let shot = self.screenshot(key, name)?;
        fs::remove_file(&shot.path).map_err(|e| io_error(&shot.path, e).with_param("name", name))?;
        self.feedback.info(Text::key("screenshot_deleted").param("name", name));
        self.screenshots(key)
    }

    /// `<game>/screenshots` of build `key`, created when missing.
    pub fn screenshots_dir(&self, key: &str) -> AppResult<PathBuf> {
        let dir = self.folder(&self.build(key)?).join(SCREENSHOTS);
        fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        Ok(dir)
    }

    /// Switches `file` on (`enable`) or off; an item already so stays as it is (the page may have
    /// shown an older state). The list after it.
    pub fn toggle(&self, key: &str, kind: ContentKind, file: &str, enable: bool) -> AppResult<ContentList> {
        self.change(key, kind, file, |game, entry, legacy| {
            if entry.item.enabled == enable {
                return Ok(None);
            }
            let on = match kind {
                ContentKind::Mods => toggle_mod(entry)?,
                ContentKind::ResourcePacks => toggle_resource_pack(game, entry, legacy)?,
                ContentKind::ShaderPacks => toggle_shader_pack(game, entry)?,
            };
            Ok(Some(Text::key(kind.toggled_key(on)).param("name", display_name(entry))))
        })
    }

    /// Deletes `file` (an enabled shader pack switches shaders off first, a resource pack leaves
    /// `options.txt`); the list after it.
    pub fn delete(&self, key: &str, kind: ContentKind, file: &str) -> AppResult<ContentList> {
        self.change(key, kind, file, |game, entry, legacy| {
            match kind {
                ContentKind::Mods => {}
                ContentKind::ResourcePacks => {
                    unlist_resource_pack(&options_path(game), &entry.item.filename, legacy)?;
                }
                ContentKind::ShaderPacks if entry.item.enabled => {
                    let properties = iris_properties(game);
                    write_properties(&properties, &[("enableShaders", "false")])
                        .map_err(|e| io_error(&properties, e))?;
                }
                ContentKind::ShaderPacks => {}
            }
            remove(entry)?;
            Ok(Some(Text::key(kind.deleted_key()).param("name", display_name(entry))))
        })
    }

    /// Puts mod `file`'s newest backup of itself back (see `content::backups`); the list after it.
    pub fn restore(&self, key: &str, kind: ContentKind, file: &str) -> AppResult<ContentList> {
        if kind != ContentKind::Mods {
            return Err(AppError::new(ErrorCode::Unsupported, "only mods have backups"));
        }
        let loader = component_id(&self.build(key)?).and_then(LoaderKind::of_component);
        self.change(key, kind, file, |game, entry, _| {
            let found = backups(game, loader, &self.backup_cache);
            let backup = backup_of(&found, &entry.item.filename, entry.item.mod_id.as_deref(), &entry.path)
                .ok_or_else(|| {
                AppError::new(ErrorCode::BackupNotFound, format!("no backup of {}", entry.item.filename))
                    .with_param("name", entry.item.filename.clone())
            })?;
            restore(game, backup, &entry.path, entry.item.enabled)?;
            Ok(Some(Text::key("mod_restored").param("name", display_name(entry))))
        })
    }

    /// Runs `action` on the listed `file` of build `key` (with the build's pack naming, see
    /// `legacy_pack_names`) under its folder's lease; announces what it did, if anything, and
    /// returns the new list. A running game does not stop it: a file the game holds open is
    /// `FileInUse`.
    fn change(
        &self,
        key: &str,
        kind: ContentKind,
        file: &str,
        action: impl FnOnce(&Path, &Entry, bool) -> AppResult<Option<Text>>,
    ) -> AppResult<ContentList> {
        let build = self.build(key)?;
        let game = self.folder(&build);
        let _lease = self
            .instances
            .try_acquire(&game, "content")
            .map_err(|e| e.with_param("version", build.name.clone()))?;
        let (_, entries) = self.scan(&build, kind);
        let entry = entries.iter().find(|e| e.item.file == file).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("no {file} in {}", kind.folder()))
                .with_param("path", file)
        })?;
        if let Some(message) = action(&game, entry, legacy_pack_names(build.version.as_deref()))? {
            self.feedback.info(message);
        }
        Ok(self.list_of(&build, kind))
    }
}
