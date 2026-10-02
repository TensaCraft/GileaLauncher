//! Version JSON files (`versions/<id>/<id>.json`) and their `inheritsFrom` chains.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::builds::ids::validate_component_id;

pub(crate) const MAX_INHERITANCE: usize = 8;

pub fn version_dir(mc_dir: &Path, id: &str) -> PathBuf {
    mc_dir.join("versions").join(id)
}

pub fn version_json_path(mc_dir: &Path, id: &str) -> PathBuf {
    version_dir(mc_dir, id).join(format!("{id}.json"))
}

fn damaged(id: &str, why: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("version {id}: {why}")).with_param("version", id)
}

/// The JSON of an installed version: `VersionNotFound` when the file is missing, `InvalidInput`
/// when it is damaged or names another id.
pub fn read_version_json(mc_dir: &Path, id: &str) -> AppResult<Map<String, Value>> {
    validate_component_id(id)?;
    let path = version_json_path(mc_dir, id);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::new(ErrorCode::VersionNotFound, format!("{} is missing", path.display()))
                .with_param("version", id));
        }
        Err(e) => return Err(AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| damaged(id, e))?;
    let Value::Object(json) = value else { return Err(damaged(id, "not an object")) };
    if json.get("id").and_then(Value::as_str) != Some(id) {
        return Err(damaged(id, "the file names another id"));
    }
    Ok(json)
}

/// `name` without its last segment (`group:artifact[:version]`), as MLL compares libraries.
fn library_key(library: &Value) -> Option<String> {
    let name = library.get("name")?.as_str()?;
    let mut parts: Vec<&str> = name.split(':').collect();
    parts.pop();
    Some(parts.join(":"))
}

/// `child` on top of `parent` as MLL's `inherit_json` does it: the child's libraries first, then
/// the parent's whose key the child lacks; lists concatenate child first; in objects only list
/// values merge (parent first, missing ones added); anything else — the child wins.
pub fn inherit(child: &Map<String, Value>, parent: &Map<String, Value>) -> Map<String, Value> {
    let mut merged = parent.clone();
    let mut libraries: Vec<Value> =
        child.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default();
    let taken: HashSet<String> = libraries.iter().filter_map(library_key).collect();
    for library in parent.get("libraries").and_then(Value::as_array).into_iter().flatten() {
        if library_key(library).is_none_or(|key| !taken.contains(&key)) {
            libraries.push(library.clone());
        }
    }
    merged.insert("libraries".to_string(), Value::Array(libraries));
    for (key, value) in child {
        if key == "libraries" {
            continue;
        }
        let replace = match (value, merged.get_mut(key)) {
            (Value::Array(items), Some(Value::Array(existing))) => {
                let mut list = items.clone();
                list.append(existing);
                *existing = list;
                false
            }
            (Value::Object(items), Some(Value::Object(existing))) => {
                for (sub, item) in items {
                    let Value::Array(extra) = item else { continue };
                    match existing.get_mut(sub) {
                        Some(Value::Array(list)) => list.extend(extra.iter().cloned()),
                        _ => {
                            existing.insert(sub.clone(), item.clone());
                        }
                    }
                }
                false
            }
            _ => true,
        };
        if replace {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

/// The version with its whole `inheritsFrom` chain applied; reads only local files.
pub fn load_merged(mc_dir: &Path, id: &str) -> AppResult<Map<String, Value>> {
    let mut chain = vec![read_version_json(mc_dir, id)?];
    let mut seen = HashSet::from([id.to_string()]);
    while let Some(parent) =
        chain.last().and_then(|json| json.get("inheritsFrom")).and_then(Value::as_str).map(str::to_string)
    {
        if !seen.insert(parent.clone()) || chain.len() > MAX_INHERITANCE {
            return Err(damaged(id, format!("inheritance loop at {parent}")));
        }
        chain.push(read_version_json(mc_dir, &parent)?);
    }
    let mut merged = chain.pop().expect("the chain holds the version itself");
    while let Some(child) = chain.pop() {
        merged = inherit(&child, &merged);
    }
    Ok(merged)
}

/// A `{id?, url, sha1?, size?}` download of the version JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FileRef {
    #[serde(default)]
    pub id: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct RawLogClient {
    file: FileRef,
    #[serde(default)]
    argument: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawLogging {
    #[serde(default)]
    client: Option<RawLogClient>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawJava {
    #[serde(default)]
    component: Option<String>,
    #[serde(default)]
    major_version: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawVersion {
    id: String,
    #[serde(default)]
    inherits_from: Option<String>,
    #[serde(default)]
    main_class: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    release_time: Option<String>,
    #[serde(default)]
    assets: Option<String>,
    #[serde(default)]
    asset_index: Option<FileRef>,
    #[serde(default)]
    downloads: BTreeMap<String, FileRef>,
    #[serde(default)]
    logging: Option<RawLogging>,
    #[serde(default)]
    java_version: Option<RawJava>,
    #[serde(default)]
    jar: Option<String>,
    #[serde(default)]
    libraries: Vec<Value>,
}

/// The parts of a (merged) version JSON that installs and launches read.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionInfo {
    pub id: String,
    pub inherits_from: Option<String>,
    pub main_class: Option<String>,
    pub kind: Option<String>,
    pub release_time: Option<String>,
    /// Name of the asset index: `assets/indexes/<name>.json`.
    pub asset_index_name: Option<String>,
    pub asset_index: Option<FileRef>,
    pub client: Option<FileRef>,
    /// `logging.client.file`; its `id` is the file name under `assets/log_configs`.
    pub log_config: Option<FileRef>,
    pub log_argument: Option<String>,
    pub java_component: Option<String>,
    pub java_major: Option<u32>,
    /// Folder and name of the client jar: `jar`, else `id`.
    pub jar_id: String,
    pub libraries: Vec<Value>,
}

impl VersionInfo {
    pub fn from_json(json: &Map<String, Value>) -> AppResult<VersionInfo> {
        let id = json.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let raw: RawVersion =
            serde_json::from_value(Value::Object(json.clone())).map_err(|e| damaged(&id, e))?;
        let (log_config, log_argument) = match raw.logging.and_then(|l| l.client) {
            Some(client) => (Some(client.file), client.argument),
            None => (None, None),
        };
        let info = VersionInfo {
            asset_index_name: raw
                .assets
                .clone()
                .or_else(|| raw.asset_index.as_ref().and_then(|i| i.id.clone())),
            jar_id: raw.jar.filter(|j| !j.is_empty()).unwrap_or_else(|| raw.id.clone()),
            id: raw.id,
            inherits_from: raw.inherits_from,
            main_class: raw.main_class,
            kind: raw.kind,
            release_time: raw.release_time,
            asset_index: raw.asset_index,
            client: raw.downloads.get("client").cloned(),
            log_config,
            log_argument,
            java_component: raw.java_version.as_ref().and_then(|j| j.component.clone()),
            java_major: raw.java_version.and_then(|j| j.major_version),
            libraries: raw.libraries,
        };
        let names = [
            ("jar", Some(&info.jar_id)),
            ("asset index", info.asset_index_name.as_ref()),
            ("log config", info.log_config.as_ref().and_then(|f| f.id.as_ref())),
            ("Java runtime", info.java_component.as_ref()),
        ];
        for (what, name) in names {
            if let Some(name) = name
                && validate_component_id(name).is_err()
            {
                return Err(damaged(&info.id, format!("unsafe {what} name {name:?}")));
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn put(mc: &Path, json: Value) {
        let id = json["id"].as_str().unwrap().to_string();
        fs::create_dir_all(version_dir(mc, &id)).unwrap();
        fs::write(version_json_path(mc, &id), json.to_string()).unwrap();
    }

    fn names(merged: &Map<String, Value>) -> Vec<String> {
        merged["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn missing_damaged_and_foreign_files_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path();
        assert_eq!(read_version_json(mc, "1.21.1").unwrap_err().code, ErrorCode::VersionNotFound);
        fs::create_dir_all(version_dir(mc, "1.21.1")).unwrap();
        fs::write(version_json_path(mc, "1.21.1"), "{ broken").unwrap();
        assert_eq!(read_version_json(mc, "1.21.1").unwrap_err().code, ErrorCode::InvalidInput);
        fs::write(version_json_path(mc, "1.21.1"), r#"{"id": "1.20.1"}"#).unwrap();
        assert_eq!(read_version_json(mc, "1.21.1").unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(read_version_json(mc, "../x").unwrap_err().code, ErrorCode::InvalidInput);
        fs::write(version_json_path(mc, "1.21.1"), r#"{"id": "1.21.1"}"#).unwrap();
        assert!(read_version_json(mc, "1.21.1").is_ok());
    }

    #[test]
    fn inheritance_matches_the_original() {
        let parent = json!({
            "id": "1.21.1", "mainClass": "net.minecraft.client.main.Main", "type": "release",
            "libraries": [{"name": "org.ow2.asm:asm:9.6"}, {"name": "com.mojang:brigadier:1.3.10"}],
            "arguments": {"game": ["--username", "${auth_player_name}"], "jvm": ["-cp", "${classpath}"]},
            "downloads": {"client": {"url": "https://x.example/client.jar", "sha1": "aa", "size": 1}},
            "extra": [1, 2]
        });
        let child = json!({
            "id": "fabric-loader-0.16.5-1.21.1", "inheritsFrom": "1.21.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{"name": "org.ow2.asm:asm:9.7.1"}, {"name": "net.fabricmc:fabric-loader:0.16.5"}],
            "arguments": {"game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "]},
            "extra": [3]
        });
        let merged = inherit(child.as_object().unwrap(), parent.as_object().unwrap());
        assert_eq!(
            names(&merged),
            ["org.ow2.asm:asm:9.7.1", "net.fabricmc:fabric-loader:0.16.5", "com.mojang:brigadier:1.3.10"]
        );
        assert_eq!(merged["mainClass"], json!("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        assert_eq!(
            merged["arguments"]["jvm"],
            json!(["-cp", "${classpath}", "-DFabricMcEmu= net.minecraft.client.main.Main "])
        );
        assert_eq!(merged["arguments"]["game"], json!(["--username", "${auth_player_name}"]));
        assert_eq!(merged["downloads"], parent["downloads"]);
        assert_eq!(merged["extra"], json!([3, 1, 2]), "lists: child first");
        assert_eq!(merged["id"], json!("fabric-loader-0.16.5-1.21.1"));
    }

    #[test]
    fn chains_are_merged_and_loops_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path();
        put(mc, json!({"id": "base", "libraries": [{"name": "a:base:1"}], "assets": "17"}));
        put(mc, json!({"id": "mid", "inheritsFrom": "base", "libraries": [{"name": "a:mid:1"}]}));
        put(mc, json!({"id": "top", "inheritsFrom": "mid", "libraries": [{"name": "a:top:1"}]}));
        let merged = load_merged(mc, "top").unwrap();
        assert_eq!(names(&merged), ["a:top:1", "a:mid:1", "a:base:1"]);
        assert_eq!(merged["assets"], json!("17"));
        put(mc, json!({"id": "loop-a", "inheritsFrom": "loop-b"}));
        put(mc, json!({"id": "loop-b", "inheritsFrom": "loop-a"}));
        assert_eq!(load_merged(mc, "loop-a").unwrap_err().code, ErrorCode::InvalidInput);
        put(mc, json!({"id": "orphan", "inheritsFrom": "gone"}));
        assert_eq!(load_merged(mc, "orphan").unwrap_err().code, ErrorCode::VersionNotFound);
    }

    #[test]
    fn version_info_reads_what_installs_need() {
        let json = json!({
            "id": "1.21.1", "type": "release", "mainClass": "M", "releaseTime": "2024-08-08T12:24:45+00:00",
            "assets": "17", "assetIndex": {"id": "17", "url": "https://x.example/17.json", "sha1": "ab", "size": 3, "totalSize": 9},
            "downloads": {"client": {"url": "https://x.example/client.jar", "sha1": "cd", "size": 5},
                          "server": {"url": "https://x.example/s.jar", "sha1": "ef", "size": 6}},
            "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}",
                                   "file": {"id": "client-1.12.xml", "url": "https://x.example/l.xml", "sha1": "12", "size": 7},
                                   "type": "log4j2-xml"}},
            "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
            "libraries": [{"name": "a:b:1"}]
        });
        let info = VersionInfo::from_json(json.as_object().unwrap()).unwrap();
        assert_eq!(info.asset_index_name.as_deref(), Some("17"));
        assert_eq!(info.client.as_ref().map(|c| c.url.as_str()), Some("https://x.example/client.jar"));
        assert_eq!(info.log_config.as_ref().and_then(|f| f.id.as_deref()), Some("client-1.12.xml"));
        assert_eq!(info.log_argument.as_deref(), Some("-Dlog4j.configurationFile=${path}"));
        assert_eq!((info.java_component.as_deref(), info.java_major), (Some("java-runtime-delta"), Some(21)));
        assert_eq!((info.jar_id.as_str(), info.libraries.len()), ("1.21.1", 1));
        let old = json!({"id": "1.2.5", "logging": {}, "libraries": []});
        let info = VersionInfo::from_json(old.as_object().unwrap()).unwrap();
        assert_eq!((info.log_config, info.java_component, info.asset_index_name), (None, None, None));
        let hostile = json!({"id": "x", "jar": "../../evil"});
        assert_eq!(
            VersionInfo::from_json(hostile.as_object().unwrap()).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        let odd_index = json!({"id": "x", "assets": "..\\up"});
        assert_eq!(
            VersionInfo::from_json(odd_index.as_object().unwrap()).unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
}
