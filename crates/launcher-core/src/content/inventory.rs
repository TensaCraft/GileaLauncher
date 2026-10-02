//! What a build has installed: mods (`.jar` on, `.jar.disabled` off),
//! resource and shader packs (`.zip`, `.zip.disabled` or folders). Links are never listed, so
//! nothing outside the build's content folders is shown or changed.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use launcher_shared::{ContentItem, LoaderKind};

use super::jar::{ModDescriptor, inspect_mod_jar};
use super::packs::{RESOURCE_PACKS, read_options_list, read_properties, resourcepack_entry};
use crate::builds::components::dir_size;

const DISABLED: &str = ".disabled";
const CACHE_SIZE: usize = 512;
const CACHE_MAX_CHARS: usize = 4096;

/// An installed item and where it lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub item: ContentItem,
}

pub use launcher_shared::mods_supported;

pub fn options_path(game_dir: &Path) -> PathBuf {
    game_dir.join("options.txt")
}

pub fn iris_properties(game_dir: &Path) -> PathBuf {
    game_dir.join("config").join("iris.properties")
}

type Stamp = (u64, Option<SystemTime>);
type Cached = (PathBuf, Option<LoaderKind>, Stamp, Option<ModDescriptor>);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// Mod metadata by file and loader, kept while the file's size and time stay (512 files).
#[derive(Default)]
pub struct MetadataCache {
    entries: Mutex<VecDeque<Cached>>,
}

impl MetadataCache {
    pub fn descriptor(&self, path: &Path, loader: Option<LoaderKind>) -> Option<ModDescriptor> {
        let before = stamp(path)?;
        {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) =
                entries.iter().position(|(p, l, s, _)| p == path && *l == loader && *s == before)
            {
                let hit = entries.remove(at)?;
                let descriptor = hit.3.clone();
                entries.push_back(hit);
                return descriptor;
            }
        }
        let descriptor = inspect_mod_jar(path, loader).ok();
        let chars = descriptor.as_ref().map_or(0, |d| {
            d.mod_id.len()
                + [&d.name, &d.version, &d.description]
                    .iter()
                    .map(|t| t.as_deref().map_or(0, str::len))
                    .sum::<usize>()
        });
        if chars <= CACHE_MAX_CHARS && stamp(path) == Some(before) {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries.retain(|(p, l, ..)| !(p == path && *l == loader));
            entries.push_back((path.to_path_buf(), loader, before, descriptor.clone()));
            while entries.len() > CACHE_SIZE {
                entries.pop_front();
            }
        }
        descriptor
    }
}

/// The files and folders directly in `dir` (links skipped), with names that are valid UTF-8.
fn children(dir: &Path) -> Vec<(PathBuf, String, fs::Metadata)> {
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    read.flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().into_string().ok()?;
            let meta = fs::symlink_metadata(&path).ok()?;
            (!meta.file_type().is_symlink()).then_some((path, name, meta))
        })
        .collect()
}

pub(crate) fn ends_with_ci(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len()
        && name.get(name.len() - suffix.len()..).is_some_and(|s| s.eq_ignore_ascii_case(suffix))
}

/// For files of `ext`: the name without `.disabled`, and whether the file is on.
fn split_disabled<'a>(name: &'a str, ext: &str) -> Option<(&'a str, bool)> {
    if ends_with_ci(name, DISABLED) {
        let base = &name[..name.len() - DISABLED.len()];
        ends_with_ci(base, ext).then_some((base, false))
    } else {
        ends_with_ci(name, ext).then_some((name, true))
    }
}

/// The mods in `<game>/mods`, by name, with what each JAR says of itself.
/// `backups` are the build's mod backups (`content::backups::backups`), for `has_backup`.
pub fn scan_mods(
    game_dir: &Path,
    loader: Option<LoaderKind>,
    cache: &MetadataCache,
    backups: &[super::backups::Backup],
) -> Vec<Entry> {
    let mut entries: Vec<Entry> = children(&game_dir.join("mods"))
        .into_iter()
        .filter(|(_, _, meta)| meta.is_file())
        .filter_map(|(path, name, meta)| {
            let (filename, enabled) = split_disabled(&name, ".jar")?;
            let filename = filename.to_string();
            let descriptor = cache.descriptor(&path, loader);
            let text = |pick: fn(&ModDescriptor) -> &Option<String>| {
                descriptor.as_ref().and_then(|d| pick(d).clone())
            };
            let mod_id = descriptor.as_ref().map(|d| d.mod_id.clone());
            let has_backup =
                super::backups::backup_of(backups, &filename, mod_id.as_deref(), &path).is_some();
            let item = ContentItem {
                name: text(|d| &d.name),
                version: text(|d| &d.version),
                description: text(|d| &d.description),
                mod_id,
                file: name,
                filename,
                size: meta.len(),
                enabled,
                folder: false,
                toggle_supported: true,
                has_backup,
            };
            Some(Entry { path, item })
        })
        .collect();
    entries.sort_by_key(|e| e.item.name.clone().unwrap_or_else(|| e.item.filename.clone()).to_lowercase());
    entries
}

/// The packs in `dir` — `.zip` / `.zip.disabled` files and folders (hidden ones skipped) — by
/// name; `on` says which are switched on.
fn scan_packs(dir: &Path, on: impl Fn(&str) -> bool, toggle_supported: bool) -> Vec<Entry> {
    let mut entries: Vec<Entry> = children(dir)
        .into_iter()
        .filter_map(|(path, name, meta)| {
            let (filename, folder, on_disk, size) = if meta.is_dir() {
                if name.starts_with('.') {
                    return None;
                }
                (name.clone(), true, true, dir_size(&path))
            } else if meta.is_file() {
                let (base, on_disk) = split_disabled(&name, ".zip")?;
                (base.to_string(), false, on_disk, meta.len())
            } else {
                return None;
            };
            let enabled = on_disk && on(&filename);
            let item = ContentItem {
                file: name,
                filename,
                size,
                enabled,
                folder,
                toggle_supported,
                name: None,
                version: None,
                description: None,
                mod_id: None,
                has_backup: false,
            };
            Some(Entry { path, item })
        })
        .collect();
    entries.sort_by_key(|e| e.item.filename.to_lowercase());
    entries
}

/// Resource packs: on when not `.disabled` and `options.txt` lists them (by bare name when
/// `legacy`, see `legacy_pack_names`).
pub fn scan_resource_packs(game_dir: &Path, legacy: bool) -> Vec<Entry> {
    let listed = read_options_list(&options_path(game_dir), RESOURCE_PACKS);
    scan_packs(
        &game_dir.join("resourcepacks"),
        |name| listed.contains(&resourcepack_entry(name, legacy)),
        true,
    )
}

/// Shader packs: with Iris they switch, and the one `iris.properties` names is on while
/// `enableShaders=true`.
pub fn scan_shader_packs(game_dir: &Path, loader: Option<LoaderKind>, cache: &MetadataCache) -> Vec<Entry> {
    let iris = has_iris(&game_dir.join("mods"), loader, cache);
    let properties = if iris { read_properties(&iris_properties(game_dir)) } else { HashMap::new() };
    // Iris counts a missing switch as on.
    let shaders_on = properties.get("enableShaders").is_none_or(|v| v.eq_ignore_ascii_case("true"));
    let chosen = properties.get("shaderPack").cloned();
    scan_packs(&game_dir.join("shaderpacks"), |name| shaders_on && chosen.as_deref() == Some(name), iris)
}

/// Iris is installed: an enabled `.jar` named `iris…` or `…iris-shaders…`, or whose metadata says
/// it is Iris.
pub fn has_iris(mods_dir: &Path, loader: Option<LoaderKind>, cache: &MetadataCache) -> bool {
    children(mods_dir).into_iter().filter(|(_, name, meta)| meta.is_file() && ends_with_ci(name, ".jar")).any(
        |(path, name, _)| {
            let lower = name.to_lowercase();
            lower.starts_with("iris")
                || lower.contains("iris-shaders")
                || cache.descriptor(&path, loader).is_some_and(|d| {
                    d.mod_id == "iris" || d.name.is_some_and(|n| n.to_lowercase().starts_with("iris shaders"))
                })
        },
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn jar(path: &Path, descriptor: &str) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("fabric.mod.json", options).unwrap();
        zip.write_all(descriptor.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn mods_are_listed_by_name_with_their_state() {
        let game = tempfile::tempdir().unwrap();
        let mods = game.path().join("mods");
        fs::create_dir_all(mods.join("folder.jar")).unwrap();
        jar(&mods.join("sodium.jar"), r#"{"id":"sodium","name":"Sodium","version":"0.6"}"#);
        jar(&mods.join("Lithium.JAR.disabled"), r#"{"id":"lithium","name":"Lithium"}"#);
        fs::write(mods.join("notes.txt"), b"x").unwrap();
        fs::write(mods.join("broken.jar"), b"not a zip").unwrap();
        let cache = MetadataCache::default();
        let items: Vec<ContentItem> =
            scan_mods(game.path(), None, &cache, &[]).into_iter().map(|e| e.item).collect();
        let view: Vec<(&str, &str, bool, Option<&str>)> = items
            .iter()
            .map(|i| (i.file.as_str(), i.filename.as_str(), i.enabled, i.name.as_deref()))
            .collect();
        assert_eq!(
            view,
            [
                ("broken.jar", "broken.jar", true, None),
                ("Lithium.JAR.disabled", "Lithium.JAR", false, Some("Lithium")),
                ("sodium.jar", "sodium.jar", true, Some("Sodium")),
            ]
        );
        assert_eq!((items[2].version.as_deref(), items[2].mod_id.as_deref()), (Some("0.6"), Some("sodium")));
        assert!(scan_mods(&game.path().join("missing"), None, &cache, &[]).is_empty());
    }

    #[test]
    fn packs_are_files_and_visible_folders() {
        let game = tempfile::tempdir().unwrap();
        let packs = game.path().join("resourcepacks");
        fs::create_dir_all(packs.join("Folder").join("assets")).unwrap();
        fs::write(packs.join("Folder").join("assets").join("a.png"), vec![0u8; 10]).unwrap();
        fs::write(packs.join("Folder").join("pack.mcmeta"), vec![0u8; 5]).unwrap();
        fs::create_dir_all(packs.join(".cache")).unwrap();
        fs::write(packs.join("A.zip"), vec![0u8; 7]).unwrap();
        fs::write(packs.join("old.zip.disabled"), b"z").unwrap();
        fs::write(packs.join("readme.txt"), b"z").unwrap();
        fs::write(
            options_path(game.path()),
            "resourcePacks:[\"vanilla\",\"file/A.zip\",\"file/Folder\",\"file/old.zip\"]\n",
        )
        .unwrap();
        let items: Vec<ContentItem> =
            scan_resource_packs(game.path(), false).into_iter().map(|e| e.item).collect();
        let view: Vec<(&str, bool, bool, u64)> =
            items.iter().map(|i| (i.file.as_str(), i.enabled, i.folder, i.size)).collect();
        assert_eq!(
            view,
            [("A.zip", true, false, 7), ("Folder", true, true, 15), ("old.zip.disabled", false, false, 1)]
        );
        assert!(items.iter().all(|i| i.toggle_supported));
    }

    #[test]
    fn shader_packs_switch_only_with_iris() {
        let game = tempfile::tempdir().unwrap();
        let shaders = game.path().join("shaderpacks");
        fs::create_dir_all(&shaders).unwrap();
        fs::write(shaders.join("BSL.zip"), b"z").unwrap();
        fs::write(shaders.join("Other.zip"), b"z").unwrap();
        fs::create_dir_all(game.path().join("config")).unwrap();
        fs::write(iris_properties(game.path()), "enableShaders=true\nshaderPack=BSL.zip\n").unwrap();
        let cache = MetadataCache::default();
        let without = scan_shader_packs(game.path(), None, &cache);
        assert!(without.iter().all(|e| !e.item.toggle_supported && !e.item.enabled));
        let mods = game.path().join("mods");
        fs::create_dir_all(&mods).unwrap();
        jar(&mods.join("renderer.jar"), r#"{"id":"iris","name":"Iris"}"#);
        assert!(has_iris(&mods, None, &cache), "by its metadata id");
        let with: Vec<(String, bool)> = scan_shader_packs(game.path(), None, &cache)
            .into_iter()
            .map(|e| (e.item.file, e.item.enabled))
            .collect();
        assert_eq!(with, [("BSL.zip".to_string(), true), ("Other.zip".to_string(), false)]);
        fs::remove_file(mods.join("renderer.jar")).unwrap();
        fs::write(mods.join("Iris-Fabric-1.8.jar"), b"not a zip").unwrap();
        assert!(has_iris(&mods, None, &cache), "by its file name");
        fs::rename(mods.join("Iris-Fabric-1.8.jar"), mods.join("Iris-Fabric-1.8.jar.disabled")).unwrap();
        assert!(!has_iris(&mods, None, &cache), "a disabled Iris does not count");
    }

    #[test]
    fn iris_counts_a_missing_switch_as_on() {
        let game = tempfile::tempdir().unwrap();
        fs::create_dir_all(game.path().join("shaderpacks")).unwrap();
        fs::write(game.path().join("shaderpacks").join("BSL.zip"), b"z").unwrap();
        fs::create_dir_all(game.path().join("mods")).unwrap();
        fs::write(game.path().join("mods").join("iris-1.8.jar"), b"not a zip").unwrap();
        fs::create_dir_all(game.path().join("config")).unwrap();
        fs::write(iris_properties(game.path()), "shaderPack=BSL.zip\n").unwrap();
        let cache = MetadataCache::default();
        let on: Vec<bool> =
            scan_shader_packs(game.path(), None, &cache).into_iter().map(|e| e.item.enabled).collect();
        assert_eq!(on, [true]);
    }
}
