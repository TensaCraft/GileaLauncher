//! A CurseForge modpack: its zip's `manifest.json` — Minecraft, the loader, the files by project
//! and file — and its overrides folder; the files as CurseForge describes them become the pack's
//! downloads. All of it is checked before anything is written (`launcher_core::packs`): plain
//! relative paths only, no launcher files, no duplicates, no file under another, within limits.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use launcher_core::net::downloader::Credential;
pub use launcher_core::packs::{Limits, Pack, PackFile, check_places, invalid, overrides_of, place};
use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};
use serde_json::{Value, json};
use zip::ZipArchive;

use super::catalog::{InstallFile, Source, install_file, text};
use super::held::Held;
use crate::types::MODPACKS_CLASS;

pub const MANIFEST: &str = "manifest.json";
/// What the launcher keeps of an installed pack.
pub const PACK_RECORD: &str = ".launcher/curseforge-pack.json";
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
/// CurseForge's classes of worlds and data packs.
const WORLDS_CLASS: u64 = 17;
const DATA_PACKS_CLASS: u64 = 6945;

/// A file the pack takes: CurseForge's project and its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wanted {
    pub project: u64,
    pub file: u64,
}

/// A pack's `manifest.json`, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub minecraft: String,
    /// `None` for plain Minecraft.
    pub loader: Option<(LoaderKind, String)>,
    /// The files the author keeps on: one marked not required is one they turned off.
    pub files: Vec<Wanted>,
    /// The archive's folder of overrides, with its slash (`overrides/`).
    pub overrides: String,
}

/// A file of the pack CurseForge gives no address for: its author keeps it from other apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackHeld {
    pub held: Held,
    pub project: u64,
    /// Its place in the build.
    pub path: String,
}

impl PackHeld {
    /// The pack's file taken from another provider's copy of its mod (`found`): in the same
    /// folder, under that copy's name.
    pub fn alternative_file(&self, found: &InstallFile) -> PackFile {
        let folder = self.path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let path =
            if folder.is_empty() { found.filename.clone() } else { format!("{folder}/{}", found.filename) };
        PackFile {
            path,
            url: found.url.clone(),
            size: found.size,
            hash: found.hash.clone(),
            sha1: Some(found.hash.hex.clone()),
            project: Some(self.project.to_string()),
            credential: None,
            local: None,
        }
    }

    /// The pack's file as `found` elsewhere.
    pub fn pack_file(&self, found: &InstallFile) -> PackFile {
        PackFile {
            path: self.path.clone(),
            url: found.url.clone(),
            size: found.size,
            hash: found.hash.clone(),
            sha1: Some(self.held.sha1.clone()),
            project: Some(self.project.to_string()),
            credential: None,
            local: match &found.from {
                Source::Copy(path) => Some(path.clone()),
                _ => None,
            },
        }
    }
}

/// The loader of a manifest's `modLoaders` id (`forge-47.2.0`, `neoforge-21.1.77`,
/// `fabric-0.16.9`, `quilt-0.27.1`) and its build.
pub fn loader_of(id: &str, minecraft: &str) -> Option<(LoaderKind, String)> {
    let id = id.trim();
    let (name, rest) = id.split_once('-')?;
    let kind = match name.to_ascii_lowercase().as_str() {
        "forge" => LoaderKind::Forge,
        "neoforge" => LoaderKind::NeoForge,
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        _ => return None,
    };
    // Some manifests name the Minecraft version too (`forge-1.12.2-14.23.5.2860`).
    let rest = rest.strip_prefix(&format!("{minecraft}-")).unwrap_or(rest);
    let rest = rest.strip_suffix(&format!("-{minecraft}")).unwrap_or(rest);
    (!rest.is_empty()).then(|| (kind, rest.to_string()))
}

/// The manifest of the pack in `zip`, all of it checked.
pub fn read_manifest<R: Read + Seek>(zip: &mut ZipArchive<R>, limits: Limits) -> AppResult<Manifest> {
    let mut entry = zip.by_name(MANIFEST).map_err(|_| invalid("the pack has no manifest.json"))?;
    if entry.is_symlink() || entry.size() > MAX_MANIFEST {
        return Err(invalid("manifest.json is not a plain file of a sane size"));
    }
    let mut body = Vec::new();
    (&mut entry).take(MAX_MANIFEST + 1).read_to_end(&mut body).map_err(|e| invalid(e.to_string()))?;
    let manifest: Value =
        serde_json::from_slice(&body).map_err(|e| invalid(format!("manifest.json: {e}")))?;
    let kind = text(&manifest, "manifestType");
    if !manifest.is_object() || !(kind.is_empty() || kind == "minecraftModpack") {
        return Err(invalid("not a Minecraft modpack's manifest"));
    }
    let minecraft = text(&manifest["minecraft"], "version").trim().to_string();
    if minecraft.is_empty() {
        return Err(invalid("the pack names no Minecraft version"));
    }
    let loaders: Vec<&Value> = manifest["minecraft"]["modLoaders"].as_array().into_iter().flatten().collect();
    let primary = loaders.iter().find(|l| l["primary"].as_bool() == Some(true)).or(loaders.first());
    let loader = match primary {
        Some(l) => Some(
            loader_of(&text(l, "id"), &minecraft)
                .ok_or_else(|| invalid(format!("unknown mod loader {:?}", text(l, "id"))))?,
        ),
        None => None,
    };
    let listed = manifest["files"].as_array().cloned().unwrap_or_default();
    if listed.len() > limits.files {
        return Err(invalid(format!("the pack lists {} files", listed.len())));
    }
    let (mut files, mut seen) = (Vec::new(), HashSet::new());
    for raw in &listed {
        if raw["required"].as_bool() == Some(false) {
            continue;
        }
        let (Some(project), Some(file)) = (raw["projectID"].as_u64(), raw["fileID"].as_u64()) else {
            return Err(invalid(format!("a file of the pack has no project or file id: {raw}")));
        };
        // A file listed twice is still one file of the pack.
        if seen.insert(Wanted { project, file }) {
            files.push(Wanted { project, file });
        }
    }
    let folder = text(&manifest, "overrides");
    let folder = if folder.trim().is_empty() { "overrides".to_string() } else { place(folder.trim())? };
    Ok(Manifest {
        name: text(&manifest, "name"),
        version: text(&manifest, "version"),
        minecraft,
        loader,
        files,
        overrides: format!("{folder}/"),
    })
}

/// Where a file of CurseForge's class `class` goes in a build; `None` for a world (the game
/// cannot open its zip where a pack would put it).
fn folder_of(class: Option<u64>) -> Option<&'static str> {
    match class {
        Some(12) => Some("resourcepacks"),
        Some(6552) => Some("shaderpacks"),
        Some(DATA_PACKS_CLASS) => Some("datapacks"),
        Some(WORLDS_CLASS) => None,
        _ => Some("mods"),
    }
}

/// The pack of `manifest` with `overrides`: each wanted file as CurseForge describes it (`files`,
/// by file id) in the folder of its project's class (`projects`, by project id), downloaded with
/// `credential`. The files CurseForge keeps from other apps come apart, to be found elsewhere.
pub fn pack_of(
    manifest: &Manifest,
    overrides: Vec<launcher_core::packs::Override>,
    files: &HashMap<u64, Value>,
    projects: &HashMap<u64, Value>,
    credential: &Credential,
) -> AppResult<(Pack, Vec<PackHeld>)> {
    let mut downloads = Vec::new();
    let mut held_files = Vec::new();
    for wanted in &manifest.files {
        let project = projects.get(&wanted.project);
        let title = project.map(|p| text(p, "name")).filter(|n| !n.is_empty());
        let missing = || {
            AppError::new(
                ErrorCode::NoFileFound,
                format!("CurseForge has no file {} of {}", wanted.file, wanted.project),
            )
            .with_param("name", title.clone().unwrap_or_else(|| wanted.project.to_string()))
        };
        let file = files
            .get(&wanted.file)
            .filter(|f| f["modId"].as_u64() == Some(wanted.project))
            .ok_or_else(missing)?;
        let class = project.and_then(|p| p["classId"].as_u64());
        if class == Some(u64::from(MODPACKS_CLASS)) {
            return Err(invalid(format!("the pack takes another pack: {}", wanted.project)));
        }
        let Some(folder) = folder_of(class) else {
            tracing::warn!("the pack's world {} is left out: worlds come in its overrides", wanted.project);
            continue;
        };
        let file_name = text(file, "fileName");
        let path = place(&format!("{folder}/{file_name}"))?;
        match install_file(file) {
            Some(ready) => downloads.push(PackFile {
                path,
                url: ready.url,
                size: ready.size,
                sha1: Some(ready.hash.hex.clone()),
                hash: ready.hash,
                project: Some(wanted.project.to_string()),
                credential: Some(credential.clone()),
                local: None,
            }),
            None => {
                // Nothing to know a copy by: no copy can be taken for it.
                let none = json!({"id": wanted.project});
                let held = Held::of(project.unwrap_or(&none), file).ok_or_else(missing)?;
                held_files.push(PackHeld { held, project: wanted.project, path });
            }
        }
    }
    let placed: Vec<PackFile> = downloads
        .iter()
        .cloned()
        .chain(held_files.iter().map(|h: &PackHeld| h.pack_file(&h.held.install_file(Source::Elsewhere))))
        .collect();
    check_places(&placed, &overrides)?;
    let pack = Pack {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        summary: String::new(),
        minecraft: manifest.minecraft.clone(),
        loader: manifest.loader.clone(),
        files: downloads,
        overrides,
    };
    Ok((pack, held_files))
}

/// The manifest and the overrides of the pack in `reader`.
pub fn read<R: Read + Seek>(
    reader: R,
    limits: Limits,
) -> AppResult<(Manifest, Vec<launcher_core::packs::Override>)> {
    let mut zip = ZipArchive::new(reader).map_err(|e| invalid(format!("not a modpack's zip: {e}")))?;
    let manifest = read_manifest(&mut zip, limits)?;
    let overrides = overrides_of(&mut zip, &[manifest.overrides.as_str()], limits)?;
    Ok((manifest, overrides))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use reqwest::header::HeaderName;
    use serde_json::json;

    use super::*;

    fn zip_of(manifest: &Value, entries: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file(MANIFEST, options).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        Cursor::new(zip.finish().unwrap().into_inner())
    }

    fn manifest(loader: &str, files: Value) -> Value {
        json!({
            "minecraft": {"version": "1.20.1", "modLoaders": [{"id": loader, "primary": true}]},
            "manifestType": "minecraftModpack", "manifestVersion": 1,
            "name": "Big Pack", "version": "1.2", "author": "A", "files": files, "overrides": "overrides"
        })
    }

    #[test]
    fn a_loader_id_names_the_loader_and_its_build() {
        let cases = [
            ("forge-47.2.0", Some((LoaderKind::Forge, "47.2.0"))),
            ("forge-1.20.1-47.2.0", Some((LoaderKind::Forge, "47.2.0"))),
            ("neoforge-21.1.77", Some((LoaderKind::NeoForge, "21.1.77"))),
            ("fabric-0.16.9", Some((LoaderKind::Fabric, "0.16.9"))),
            ("quilt-0.27.1-beta.1", Some((LoaderKind::Quilt, "0.27.1-beta.1"))),
            ("liteloader-1.0", None),
            ("forge", None),
            ("forge-", None),
        ];
        for (id, expected) in cases {
            let got = loader_of(id, "1.20.1");
            assert_eq!(got.as_ref().map(|(k, v)| (*k, v.as_str())), expected, "{id}");
        }
    }

    #[test]
    fn the_manifest_names_minecraft_the_loader_and_the_files_kept_on() {
        let files = json!([
            {"projectID": 1, "fileID": 10, "required": true},
            {"projectID": 2, "fileID": 20, "required": false},
            {"projectID": 3, "fileID": 30}
        ]);
        let mut zip = ZipArchive::new(zip_of(&manifest("forge-47.2.0", files), &[])).unwrap();
        let read = read_manifest(&mut zip, Limits::default()).unwrap();
        assert_eq!(
            read,
            Manifest {
                name: "Big Pack".into(),
                version: "1.2".into(),
                minecraft: "1.20.1".into(),
                loader: Some((LoaderKind::Forge, "47.2.0".into())),
                files: vec![Wanted { project: 1, file: 10 }, Wanted { project: 3, file: 30 }],
                overrides: "overrides/".into(),
            },
            "a file the author turned off is not the pack's"
        );
    }

    #[test]
    fn a_file_listed_twice_is_taken_once() {
        let files = json!([
            {"projectID": 1, "fileID": 10, "required": true},
            {"projectID": 1, "fileID": 10, "required": true}
        ]);
        let mut zip = ZipArchive::new(zip_of(&manifest("forge-47.2.0", files), &[])).unwrap();
        let read = read_manifest(&mut zip, Limits::default()).unwrap();
        assert_eq!(read.files, [Wanted { project: 1, file: 10 }]);
    }

    #[test]
    fn a_manifest_that_cannot_be_used_is_refused() {
        let bad = [
            json!({"minecraft": {"version": ""}, "files": []}),
            json!({"manifestType": "other", "minecraft": {"version": "1.20.1"}}),
            manifest("liteloader-1.0", json!([])),
            manifest("forge-47.2.0", json!([{"projectID": 1}])),
            {
                let mut m = manifest("forge-47.2.0", json!([]));
                m["overrides"] = json!("../outside");
                m
            },
        ];
        for manifest in bad {
            let mut zip = ZipArchive::new(zip_of(&manifest, &[])).unwrap();
            let e = read_manifest(&mut zip, Limits::default()).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidInput, "{manifest}");
        }
        let mut no_manifest = zip::ZipWriter::new(Cursor::new(Vec::new()));
        no_manifest.start_file("x.txt", zip::write::SimpleFileOptions::default()).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(no_manifest.finish().unwrap().into_inner())).unwrap();
        assert!(read_manifest(&mut zip, Limits::default()).is_err());
    }

    #[test]
    fn overrides_come_from_the_manifest_s_folder() {
        let mut m = manifest("fabric-0.16.9", json!([]));
        m["overrides"] = json!("extra");
        let zip = zip_of(&m, &[("extra/config/a.toml", b"a"), ("overrides/config/b.toml", b"b")]);
        let (manifest, overrides) = read(zip, Limits::default()).unwrap();
        assert_eq!(manifest.overrides, "extra/");
        let paths: Vec<&str> = overrides.iter().map(|o| o.path.as_str()).collect();
        assert_eq!(paths, ["config/a.toml"]);
        let zip = zip_of(&m, &[("extra/version.json", b"{}")]);
        assert!(read(zip, Limits::default()).is_err(), "not the build's own record");
    }

    fn credential() -> Credential {
        Credential::new(HeaderName::from_static("x-api-key"), "k".parse().unwrap(), |_| true)
    }

    fn cf_file(id: u64, project: u64, name: &str, url: Value) -> Value {
        json!({"id": id, "modId": project, "fileName": name, "fileLength": 5, "downloadUrl": url,
               "hashes": [{"value": "b".repeat(40), "algo": 1}]})
    }

    fn cf_project(id: u64, name: &str, class: u64) -> Value {
        json!({"id": id, "name": name, "classId": class,
               "links": {"websiteUrl": format!("https://www.curseforge.com/minecraft/x/{id}")}})
    }

    #[test]
    fn files_go_to_their_class_s_folder_and_held_ones_come_apart() {
        let manifest = Manifest {
            name: "P".into(),
            version: "1".into(),
            minecraft: "1.20.1".into(),
            loader: Some((LoaderKind::Forge, "47.2.0".into())),
            files: [(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]
                .into_iter()
                .map(|(project, file)| Wanted { project, file })
                .collect(),
            overrides: "overrides/".into(),
        };
        let url = |n: &str| json!(format!("https://edge.forgecdn.net/files/{n}"));
        let files: HashMap<u64, Value> = [
            cf_file(10, 1, "a.jar", url("a.jar")),
            cf_file(20, 2, "faithful.zip", url("faithful.zip")),
            cf_file(30, 3, "shaders.zip", url("shaders.zip")),
            cf_file(40, 4, "held.jar", Value::Null),
            cf_file(50, 5, "world.zip", url("world.zip")),
        ]
        .into_iter()
        .map(|f| (f["id"].as_u64().unwrap(), f))
        .collect();
        let projects: HashMap<u64, Value> = [
            cf_project(1, "A", 6),
            cf_project(2, "Faithful", 12),
            cf_project(3, "Shaders", 6552),
            cf_project(4, "Held", 6),
            cf_project(5, "World", WORLDS_CLASS),
        ]
        .into_iter()
        .map(|p| (p["id"].as_u64().unwrap(), p))
        .collect();
        let (pack, held) = pack_of(&manifest, Vec::new(), &files, &projects, &credential()).unwrap();
        let placed: Vec<(&str, Option<&str>)> =
            pack.files.iter().map(|f| (f.path.as_str(), f.project.as_deref())).collect();
        assert_eq!(
            placed,
            [
                ("mods/a.jar", Some("1")),
                ("resourcepacks/faithful.zip", Some("2")),
                ("shaderpacks/shaders.zip", Some("3"))
            ],
            "a world is left out"
        );
        assert!(pack.files.iter().all(|f| f.credential.is_some() && f.sha1 == Some("b".repeat(40))));
        assert_eq!(
            held,
            [PackHeld {
                held: Held {
                    title: "Held".into(),
                    slug: String::new(),
                    file_name: "held.jar".into(),
                    page: Some("https://www.curseforge.com/minecraft/x/4/files/40".into()),
                    size: 5,
                    sha1: "b".repeat(40),
                },
                project: 4,
                path: "mods/held.jar".into(),
            }]
        );
    }

    #[test]
    fn a_file_curseforge_does_not_have_or_of_another_project_stops_the_pack() {
        let manifest = Manifest {
            name: "P".into(),
            version: "1".into(),
            minecraft: "1.20.1".into(),
            loader: None,
            files: vec![Wanted { project: 1, file: 10 }],
            overrides: "overrides/".into(),
        };
        let projects: HashMap<u64, Value> = [(1, cf_project(1, "A", 6))].into_iter().collect();
        let none = HashMap::new();
        let e = pack_of(&manifest, Vec::new(), &none, &projects, &credential()).unwrap_err();
        assert_eq!((e.code, e.params.get("name").map(String::as_str)), (ErrorCode::NoFileFound, Some("A")));
        let other: HashMap<u64, Value> =
            [(10, cf_file(10, 2, "a.jar", json!("https://edge.forgecdn.net/a.jar")))].into_iter().collect();
        assert_eq!(
            pack_of(&manifest, Vec::new(), &other, &projects, &credential()).unwrap_err().code,
            ErrorCode::NoFileFound
        );
    }

    #[test]
    fn two_files_at_one_place_stop_the_pack() {
        let manifest = Manifest {
            name: "P".into(),
            version: "1".into(),
            minecraft: "1.20.1".into(),
            loader: None,
            files: vec![Wanted { project: 1, file: 10 }, Wanted { project: 2, file: 20 }],
            overrides: "overrides/".into(),
        };
        let url = json!("https://edge.forgecdn.net/a.jar");
        let files: HashMap<u64, Value> =
            [(10, cf_file(10, 1, "a.jar", url.clone())), (20, cf_file(20, 2, "A.jar", url))]
                .into_iter()
                .collect();
        let projects: HashMap<u64, Value> =
            [(1, cf_project(1, "A", 6)), (2, cf_project(2, "B", 6))].into_iter().collect();
        let e = pack_of(&manifest, Vec::new(), &files, &projects, &credential()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput);
    }
}
