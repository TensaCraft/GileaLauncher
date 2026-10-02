//! A `.mrpack`: its `modrinth.index.json` — Minecraft, the loader, the files
//! to download — and its `overrides/`, then `client-overrides/`, checked before
//! anything is written (`launcher_core::packs`): plain relative paths only, no launcher files, no
//! duplicates, no file under another, within limits.

use std::io::{Read, Seek};

use launcher_core::net::downloader::ExpectedHash;
pub use launcher_core::packs::{
    Limits, Override, Pack, PackFile, PackRecord, check_places, extract, invalid, override_sha1s,
    overrides_of, place,
};
use launcher_shared::{AppResult, LoaderKind};
use serde_json::Value;
use zip::ZipArchive;

use super::catalog::{secure_url, text};

pub const INDEX: &str = "modrinth.index.json";
/// What the launcher keeps of an installed pack.
pub const PACK_RECORD: &str = ".launcher/modrinth-pack.json";
const MAX_INDEX: u64 = 16 * 1024 * 1024;
/// Where overrides come from, the later over the earlier.
const OVERRIDES: [&str; 2] = ["overrides/", "client-overrides/"];

/// SHA-512, else SHA-256, else SHA-1, when well formed.
pub fn hash_of(hashes: &Value) -> Option<ExpectedHash> {
    let digest = |key: &str, len: usize| {
        hashes
            .get(key)
            .and_then(Value::as_str)
            .map(|d| d.trim().to_ascii_lowercase())
            .filter(|d| d.len() == len && d.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    digest("sha512", 128)
        .map(|d| ExpectedHash::sha512(&d))
        .or_else(|| digest("sha256", 64).map(|d| ExpectedHash::sha256(&d)))
        .or_else(|| digest("sha1", 40).map(|d| ExpectedHash::sha1(&d)))
}

/// The Modrinth project a pack file comes from (`…/data/<project>/versions/…`, Modrinth's CDN).
pub fn project_of(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let segments: Vec<&str> = url.path_segments()?.collect();
    let at = segments.iter().position(|s| *s == "data")?;
    (segments.get(at + 2) == Some(&"versions"))
        .then(|| segments[at + 1].to_string())
        .filter(|p| !p.is_empty())
}

/// The version's `.mrpack` file (the primary one first).
pub fn pack_download(version: &Value) -> Option<PackFile> {
    let mut files: Vec<&Value> =
        version.get("files").and_then(Value::as_array).into_iter().flatten().collect();
    files.sort_by_key(|f| !f.get("primary").and_then(Value::as_bool).unwrap_or(false));
    files.into_iter().find_map(|f| {
        let (url, name) = (text(f, "url"), text(f, "filename"));
        let size = f.get("size").and_then(Value::as_u64).filter(|s| *s > 0)?;
        if !name.to_ascii_lowercase().ends_with(".mrpack") || !secure_url(&url) {
            return None;
        }
        Some(PackFile {
            path: name,
            url,
            size,
            hash: hash_of(f.get("hashes")?)?,
            sha1: None,
            project: None,
            credential: None,
            local: None,
        })
    })
}

pub fn read<R: Read + Seek>(reader: R) -> AppResult<Pack> {
    read_with(reader, Limits::default())
}

/// The pack in `reader`, all of it checked.
pub fn read_with<R: Read + Seek>(reader: R, limits: Limits) -> AppResult<Pack> {
    let mut zip = ZipArchive::new(reader).map_err(|e| invalid(format!("not a .mrpack: {e}")))?;
    let index = read_index(&mut zip)?;
    let deps = index.get("dependencies").cloned().unwrap_or(Value::Null);
    let minecraft = text(&deps, "minecraft");
    if minecraft.is_empty() {
        return Err(invalid("the pack names no Minecraft version"));
    }
    let loader = [
        ("neoforge", LoaderKind::NeoForge),
        ("forge", LoaderKind::Forge),
        ("fabric-loader", LoaderKind::Fabric),
        ("quilt-loader", LoaderKind::Quilt),
    ]
    .into_iter()
    .find_map(|(key, kind)| Some(text(&deps, key)).filter(|v| !v.is_empty()).map(|v| (kind, v)));
    let files = files_of(&index, limits)?;
    let overrides = overrides_of(&mut zip, &OVERRIDES, limits)?;
    check_places(&files, &overrides)?;
    Ok(Pack {
        name: text(&index, "name"),
        version: text(&index, "versionId"),
        summary: text(&index, "summary"),
        minecraft,
        loader,
        files,
        overrides,
    })
}

fn read_index<R: Read + Seek>(zip: &mut ZipArchive<R>) -> AppResult<Value> {
    let mut entry = zip.by_name(INDEX).map_err(|_| invalid("the pack has no modrinth.index.json"))?;
    if entry.is_symlink() || entry.size() > MAX_INDEX {
        return Err(invalid("modrinth.index.json is not a plain file of a sane size"));
    }
    let mut body = Vec::new();
    (&mut entry).take(MAX_INDEX + 1).read_to_end(&mut body).map_err(|e| invalid(e.to_string()))?;
    let index: Value =
        serde_json::from_slice(&body).map_err(|e| invalid(format!("modrinth.index.json: {e}")))?;
    let format = index.get("formatVersion").and_then(Value::as_u64);
    if !index.is_object() || format != Some(1) || text(&index, "game") != "minecraft" {
        return Err(invalid("not a Minecraft pack of format 1"));
    }
    Ok(index)
}

fn files_of(index: &Value, limits: Limits) -> AppResult<Vec<PackFile>> {
    let listed = index.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    if listed.len() > limits.files {
        return Err(invalid(format!("the pack lists {} files", listed.len())));
    }
    let mut files = Vec::new();
    for raw in &listed {
        if raw.get("env").map(|e| text(e, "client")).as_deref() == Some("unsupported") {
            continue;
        }
        let path = place(&text(raw, "path"))?;
        let bad = |why: &str| invalid(format!("{path}: {why}")).with_param("name", path.clone());
        let url = raw
            .get("downloads")
            .and_then(Value::as_array)
            .and_then(|d| d.first())
            .and_then(Value::as_str)
            .filter(|u| secure_url(u))
            .ok_or_else(|| bad("no safe address"))?
            .to_string();
        let hash = raw.get("hashes").and_then(hash_of).ok_or_else(|| bad("no usable hash"))?;
        let size =
            raw.get("fileSize").and_then(Value::as_u64).filter(|s| *s > 0).ok_or_else(|| bad("no size"))?;
        let sha1 = raw
            .get("hashes")
            .and_then(|h| h.get("sha1"))
            .and_then(Value::as_str)
            .map(|d| d.trim().to_ascii_lowercase())
            .filter(|d| d.len() == 40 && d.bytes().all(|b| b.is_ascii_hexdigit()));
        let project = project_of(&url);
        files.push(PackFile { path, url, size, hash, sha1, project, credential: None, local: None });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use launcher_shared::ErrorCode;
    use serde_json::json;
    use zip::write::SimpleFileOptions;

    use super::*;

    const SHA: &str = "ab";

    fn file(path: &str) -> Value {
        json!({"path": path, "downloads": ["https://cdn.modrinth.com/data/x/a.jar"], "fileSize": 5,
               "hashes": {"sha512": SHA.repeat(64), "sha1": "cd".repeat(20)}})
    }

    fn index(files: Vec<Value>, deps: Value) -> Value {
        json!({"formatVersion": 1, "game": "minecraft", "versionId": "1.2", "name": "Pack",
               "summary": "Fast", "files": files, "dependencies": deps})
    }

    fn mrpack(index: &Value, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        zip.start_file(INDEX, options).unwrap();
        zip.write_all(index.to_string().as_bytes()).unwrap();
        for (name, body) in entries {
            if name.ends_with('/') {
                zip.add_directory(*name, options).unwrap();
                continue;
            }
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn fabric() -> Value {
        json!({"minecraft": "1.21.1", "fabric-loader": "0.16.9"})
    }

    #[test]
    fn a_pack_names_its_game_loader_files_and_client_overrides() {
        let mut client_only = file("mods/client.jar");
        client_only["env"] = json!({"client": "required", "server": "unsupported"});
        let mut server_only = file("mods/server.jar");
        server_only["env"] = json!({"client": "unsupported", "server": "required"});
        let bytes = mrpack(
            &index(vec![file("mods/a.jar"), client_only, server_only], fabric()),
            &[
                ("overrides/config/", b""),
                ("overrides/config/a.txt", b"common"),
                ("overrides/options.txt", b"o"),
                ("client-overrides/config/a.txt", b"client"),
                ("server-overrides/server.properties", b"s"),
            ],
        );
        let pack = read(Cursor::new(bytes)).unwrap();
        assert_eq!(
            (pack.name.as_str(), pack.version.as_str(), pack.minecraft.as_str()),
            ("Pack", "1.2", "1.21.1")
        );
        assert_eq!(pack.summary, "Fast");
        assert_eq!(pack.loader, Some((LoaderKind::Fabric, "0.16.9".into())));
        let paths: Vec<&str> = pack.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["mods/a.jar", "mods/client.jar"], "a file the client does not need is skipped");
        assert_eq!(pack.files[0].hash, ExpectedHash::sha512(&SHA.repeat(64)));
        let overrides: Vec<(&str, &str)> =
            pack.overrides.iter().map(|o| (o.path.as_str(), o.entry.as_str())).collect();
        assert_eq!(
            overrides,
            [("config/a.txt", "client-overrides/config/a.txt"), ("options.txt", "overrides/options.txt")],
            "client overrides win; folders and server overrides are not files to place"
        );
    }

    #[test]
    fn the_loader_is_the_first_of_neoforge_forge_fabric_quilt() {
        let loader = |deps: Value| read(Cursor::new(mrpack(&index(vec![], deps), &[]))).unwrap().loader;
        assert_eq!(loader(json!({"minecraft": "1.21.1"})), None);
        assert_eq!(
            loader(json!({"minecraft": "1.21.1", "forge": "52.0.1", "neoforge": "21.1.209"})),
            Some((LoaderKind::NeoForge, "21.1.209".into()))
        );
        assert_eq!(
            loader(json!({"minecraft": "1.20.1", "quilt-loader": "0.26.4"})),
            Some((LoaderKind::Quilt, "0.26.4".into()))
        );
        let missing =
            read(Cursor::new(mrpack(&index(vec![], json!({"fabric-loader": "0.16.9"})), &[]))).unwrap_err();
        assert_eq!(missing.code, ErrorCode::InvalidInput);
        let mut other = index(vec![], fabric());
        other["game"] = json!("terraria");
        assert!(read(Cursor::new(mrpack(&other, &[]))).is_err());
        assert!(read(Cursor::new(b"not a zip".to_vec())).is_err());
    }

    #[test]
    fn unsafe_launcher_duplicate_and_nested_paths_are_refused() {
        for bad in [
            vec![file("../evil.jar")],
            vec![file("/abs.jar")],
            vec![file("C:/x.jar")],
            vec![file("mods\\x.jar")],
            vec![file(".launcher/modrinth-content.json")],
            vec![file(".launcher-modrinth-sync.json")],
            vec![file("version.json")],
            vec![file("Version.JSON")],
            vec![file("mods/a.jar"), file("MODS/A.jar")],
            vec![file("config"), file("config/x.txt")],
        ] {
            let what = format!("{bad:?}");
            assert!(read(Cursor::new(mrpack(&index(bad, fabric()), &[]))).is_err(), "{what}");
        }
        let clash = mrpack(&index(vec![file("config/a.txt")], fabric()), &[("overrides/config/a.txt", b"x")]);
        assert!(read(Cursor::new(clash)).is_err(), "a download and an override of one path");
        let reserved = mrpack(&index(vec![], fabric()), &[("overrides/.launcher/x.json", b"x")]);
        assert!(read(Cursor::new(reserved)).is_err(), "no launcher files through overrides");
        let record = mrpack(&index(vec![], fabric()), &[("client-overrides/version.json", b"{}")]);
        assert!(read(Cursor::new(record)).is_err(), "no build record through overrides");
    }

    #[test]
    fn downloads_need_a_safe_address_a_hash_and_a_size() {
        let mut http = file("mods/a.jar");
        http["downloads"] = json!(["http://cdn.modrinth.com/a.jar"]);
        let mut no_hash = file("mods/b.jar");
        no_hash["hashes"] = json!({"md5": "00"});
        let mut empty = file("mods/c.jar");
        empty["fileSize"] = json!(0);
        for bad in [http, no_hash, empty] {
            assert!(read(Cursor::new(mrpack(&index(vec![bad], fabric()), &[]))).is_err());
        }
        let mut sha256 = file("mods/d.jar");
        sha256["hashes"] = json!({"sha256": "ef".repeat(32)});
        let pack = read(Cursor::new(mrpack(&index(vec![sha256], fabric()), &[]))).unwrap();
        assert_eq!(pack.files[0].hash, ExpectedHash::sha256(&"ef".repeat(32)));
    }

    #[test]
    fn limits_hold_for_files_and_overrides() {
        let small = Limits { files: 1, entry: 4, total: 6 };
        let two = mrpack(&index(vec![file("mods/a.jar"), file("mods/b.jar")], fabric()), &[]);
        assert!(read_with(Cursor::new(two), small).is_err(), "too many files");
        let big = mrpack(&index(vec![], fabric()), &[("overrides/a.txt", b"12345")]);
        assert!(read_with(Cursor::new(big), small).is_err(), "one override too large");
        let sum =
            mrpack(&index(vec![], fabric()), &[("overrides/a.txt", b"1234"), ("overrides/b.txt", b"1234")]);
        assert!(
            read_with(Cursor::new(sum), Limits { files: 5, ..small }).is_err(),
            "overrides too large together"
        );
    }

    #[test]
    fn overrides_come_out_whole_and_nowhere_else() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("pack.mrpack");
        std::fs::write(&archive, mrpack(&index(vec![], fabric()), &[("overrides/config/a.txt", b"hello")]))
            .unwrap();
        let pack = read(std::fs::File::open(&archive).unwrap()).unwrap();
        let dest = dir.path().join("out/config/a.txt");
        let done = std::cell::Cell::new(0);
        extract(&archive, &[(pack.overrides[0].clone(), dest.clone())], &|bytes| done.set(bytes)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello", "its folders are made");
        assert_eq!(done.get(), 5, "progress in bytes");
        let lying = Override { entry: "overrides/config/a.txt".into(), path: "config/a.txt".into(), size: 2 };
        assert!(
            extract(&archive, &[(lying, dir.path().join("x.txt"))], &|_| {}).is_err(),
            "not the size the index said"
        );
    }

    #[test]
    fn a_version_s_pack_file_is_its_mrpack() {
        let version = json!({"files": [
            {"url": "https://cdn.modrinth.com/x/readme.txt", "filename": "readme.txt", "size": 3, "primary": true,
             "hashes": {"sha512": SHA.repeat(64)}},
            {"url": "https://cdn.modrinth.com/x/pack.mrpack", "filename": "Pack 1.2.mrpack", "size": 10,
             "hashes": {"sha1": "cd".repeat(20)}}]});
        let found = pack_download(&version).unwrap();
        assert_eq!((found.path.as_str(), found.size), ("Pack 1.2.mrpack", 10));
        assert!(pack_download(&json!({"files": []})).is_none());
    }
}
