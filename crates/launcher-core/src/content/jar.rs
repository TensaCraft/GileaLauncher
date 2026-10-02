//! What a mod JAR says about itself: its id, name, version and description from
//! `fabric.mod.json`, `quilt.mod.json`, `META-INF/neoforge.mods.toml`, `META-INF/mods.toml` or
//! `mcmod.info` — read without unpacking the JAR and never more than 1 MiB of it.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use launcher_shared::LoaderKind;
use serde_json::Value;
use zip::ZipArchive;

/// The most of a descriptor that is read.
pub const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MANIFEST: &str = "META-INF/MANIFEST.MF";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const FORMATS: [&str; 5] =
    ["fabric.mod.json", "quilt.mod.json", "META-INF/neoforge.mods.toml", "META-INF/mods.toml", "mcmod.info"];

/// The first mod a JAR describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDescriptor {
    pub metadata_file: &'static str,
    pub mod_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
}

/// Why a JAR describes no mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectError {
    CorruptJar,
    UnreadableJar,
    MissingDescriptor,
    MetadataTooLarge,
    InvalidMetadata,
}

type Jar = ZipArchive<BufReader<File>>;

/// The descriptors to look for, those of the build's loader first.
pub fn metadata_order(loader: Option<LoaderKind>) -> Vec<&'static str> {
    let preferred: &[&'static str] = match loader {
        Some(LoaderKind::Fabric) => &["fabric.mod.json", "quilt.mod.json"],
        Some(LoaderKind::Quilt) => &["quilt.mod.json", "fabric.mod.json"],
        Some(LoaderKind::Forge) => &["META-INF/mods.toml", "mcmod.info"],
        Some(LoaderKind::NeoForge) => &["META-INF/neoforge.mods.toml", "META-INF/mods.toml", "mcmod.info"],
        _ => &[],
    };
    preferred.iter().copied().chain(FORMATS.into_iter().filter(|f| !preferred.contains(f))).collect()
}

/// A mod id: lower-case ASCII letters, digits and `_.-`.
pub fn clean_id(raw: &str) -> Option<String> {
    let id = raw.trim().to_lowercase();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))).then_some(id)
}

fn clean_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// What `path` describes, reading the descriptor of `loader` first.
pub fn inspect_mod_jar(path: &Path, loader: Option<LoaderKind>) -> Result<ModDescriptor, InspectError> {
    let file = File::open(path).map_err(|_| InspectError::UnreadableJar)?;
    let mut jar = ZipArchive::new(BufReader::new(file)).map_err(|_| InspectError::CorruptJar)?;
    let names: Vec<String> = jar.file_names().map(str::to_string).collect();
    let metadata_file = metadata_order(loader)
        .into_iter()
        .find(|f| names.iter().any(|n| n == f))
        .ok_or(InspectError::MissingDescriptor)?;
    if names.iter().filter(|n| *n == metadata_file).count() != 1 {
        return Err(InspectError::InvalidMetadata);
    }
    let payload = read_entry(&mut jar, metadata_file, MAX_METADATA_BYTES)?;
    let text = std::str::from_utf8(&payload).map_err(|_| InspectError::InvalidMetadata)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut descriptor = parse(metadata_file, text).ok_or(InspectError::InvalidMetadata)?;
    if descriptor.version.as_deref().is_some_and(|v| v.contains("${")) {
        descriptor.version = manifest_version(&mut jar);
    }
    Ok(descriptor)
}

fn read_entry(jar: &mut Jar, name: &str, limit: u64) -> Result<Vec<u8>, InspectError> {
    let entry = jar.by_name(name).map_err(|_| InspectError::CorruptJar)?;
    if entry.size() > limit {
        return Err(InspectError::MetadataTooLarge);
    }
    let mut payload = Vec::new();
    entry.take(limit + 1).read_to_end(&mut payload).map_err(|_| InspectError::CorruptJar)?;
    if payload.len() as u64 > limit {
        return Err(InspectError::MetadataTooLarge);
    }
    Ok(payload)
}

/// `Implementation-Version` of the JAR's manifest (Forge mods often declare `${file.jarVersion}`).
fn manifest_version(jar: &mut Jar) -> Option<String> {
    let payload = read_entry(jar, MANIFEST, MAX_MANIFEST_BYTES).ok()?;
    String::from_utf8_lossy(&payload)
        .lines()
        .find_map(|line| line.strip_prefix("Implementation-Version:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && !v.contains("${"))
}

fn json_descriptor(
    metadata_file: &'static str,
    id: &Value,
    name: &Value,
    version: &Value,
    description: &Value,
) -> Option<ModDescriptor> {
    Some(ModDescriptor {
        metadata_file,
        mod_id: clean_id(id.as_str()?)?,
        name: clean_text(name.as_str()),
        version: clean_text(version.as_str()),
        description: clean_text(description.as_str()),
    })
}

fn parse(metadata_file: &'static str, text: &str) -> Option<ModDescriptor> {
    match metadata_file {
        "fabric.mod.json" => {
            let root: Value = serde_json::from_str(text).ok()?;
            json_descriptor(metadata_file, &root["id"], &root["name"], &root["version"], &root["description"])
        }
        "quilt.mod.json" => {
            let root: Value = serde_json::from_str(text).ok()?;
            let loader = root.get("quilt_loader")?;
            let metadata = &loader["metadata"];
            json_descriptor(
                metadata_file,
                &loader["id"],
                &metadata["name"],
                &loader["version"],
                &metadata["description"],
            )
        }
        "mcmod.info" => {
            let root: Value = serde_json::from_str(text).ok()?;
            let first = match &root {
                Value::Array(list) => list.first()?,
                Value::Object(map) => match map.get("modList") {
                    Some(Value::Array(list)) => list.first()?,
                    _ => &root,
                },
                _ => return None,
            };
            let id = first.get("modid").or_else(|| first.get("modId")).unwrap_or(&Value::Null);
            json_descriptor(metadata_file, id, &first["name"], &first["version"], &first["description"])
        }
        _ => {
            let root: toml::Table = toml::from_str(text).ok()?;
            root.get("mods")?.as_array()?.iter().filter_map(toml::Value::as_table).find_map(|m| {
                let text = |key: &str| clean_text(m.get(key).and_then(toml::Value::as_str));
                Some(ModDescriptor {
                    metadata_file,
                    mod_id: clean_id(m.get("modId")?.as_str()?)?,
                    name: text("displayName"),
                    version: text("version"),
                    description: text("description"),
                })
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn jar(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (entry, bytes) in entries {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    const FABRIC: &[u8] = br#"{"id":"Sodium","name":" Sodium ","version":"0.6.0","description":"Fast"}"#;
    const QUILT: &[u8] =
        br#"{"quilt_loader":{"id":"qsl","version":"9.0","metadata":{"name":"QSL","description":"Std"}}}"#;

    #[test]
    fn every_descriptor_format_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let fabric =
            inspect_mod_jar(&jar(dir.path(), "a.jar", &[("fabric.mod.json", FABRIC)]), None).unwrap();
        assert_eq!(
            (
                fabric.mod_id.as_str(),
                fabric.name.as_deref(),
                fabric.version.as_deref(),
                fabric.description.as_deref()
            ),
            ("sodium", Some("Sodium"), Some("0.6.0"), Some("Fast"))
        );
        let quilt = inspect_mod_jar(&jar(dir.path(), "b.jar", &[("quilt.mod.json", QUILT)]), None).unwrap();
        assert_eq!(
            (quilt.mod_id.as_str(), quilt.name.as_deref(), quilt.version.as_deref()),
            ("qsl", Some("QSL"), Some("9.0"))
        );
        let toml = b"modLoader=\"javafml\"\n[[mods]]\nmodId=\"create\"\ndisplayName=\"Create\"\nversion=\"6.0.4\"\ndescription='''Build'''\n";
        let neo = inspect_mod_jar(&jar(dir.path(), "c.jar", &[("META-INF/neoforge.mods.toml", toml)]), None)
            .unwrap();
        assert_eq!(
            (neo.metadata_file, neo.mod_id.as_str(), neo.name.as_deref()),
            ("META-INF/neoforge.mods.toml", "create", Some("Create"))
        );
        let mcmod = br#"{"modListVersion":2,"modList":[{"modid":"jei","name":"JEI","version":"4.16"}]}"#;
        let old = inspect_mod_jar(&jar(dir.path(), "d.jar", &[("mcmod.info", mcmod)]), None).unwrap();
        assert_eq!((old.mod_id.as_str(), old.version.as_deref()), ("jei", Some("4.16")));
        let list = br#"[{"modid":"ic2","name":"IC2"}]"#;
        assert_eq!(
            inspect_mod_jar(&jar(dir.path(), "e.jar", &[("mcmod.info", list)]), None).unwrap().mod_id,
            "ic2"
        );
    }

    #[test]
    fn a_placeholder_version_comes_from_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let toml = b"[[mods]]\nmodId=\"jei\"\nversion=\"${file.jarVersion}\"\n";
        let manifest = b"Manifest-Version: 1.0\r\nImplementation-Version: 19.21.0.247\r\n";
        let with =
            jar(dir.path(), "a.jar", &[("META-INF/mods.toml", toml), ("META-INF/MANIFEST.MF", manifest)]);
        assert_eq!(inspect_mod_jar(&with, None).unwrap().version.as_deref(), Some("19.21.0.247"));
        let without = jar(dir.path(), "b.jar", &[("META-INF/mods.toml", toml)]);
        assert_eq!(inspect_mod_jar(&without, None).unwrap().version, None);
    }

    #[test]
    fn the_builds_loader_decides_which_descriptor_counts() {
        let dir = tempfile::tempdir().unwrap();
        let both = jar(dir.path(), "both.jar", &[("fabric.mod.json", FABRIC), ("quilt.mod.json", QUILT)]);
        assert_eq!(inspect_mod_jar(&both, Some(LoaderKind::Quilt)).unwrap().mod_id, "qsl");
        assert_eq!(inspect_mod_jar(&both, Some(LoaderKind::Fabric)).unwrap().mod_id, "sodium");
        assert_eq!(inspect_mod_jar(&both, None).unwrap().mod_id, "sodium", "format order");
        assert_eq!(
            metadata_order(Some(LoaderKind::NeoForge)),
            [
                "META-INF/neoforge.mods.toml",
                "META-INF/mods.toml",
                "mcmod.info",
                "fabric.mod.json",
                "quilt.mod.json"
            ]
        );
    }

    #[test]
    fn broken_jars_say_why() {
        let dir = tempfile::tempdir().unwrap();
        let none = jar(dir.path(), "none.jar", &[("readme.txt", b"hi")]);
        assert_eq!(inspect_mod_jar(&none, None).unwrap_err(), InspectError::MissingDescriptor);
        let big = vec![b' '; MAX_METADATA_BYTES as usize + 1];
        let huge = jar(dir.path(), "huge.jar", &[("fabric.mod.json", &big)]);
        assert_eq!(inspect_mod_jar(&huge, None).unwrap_err(), InspectError::MetadataTooLarge);
        let bad = jar(dir.path(), "bad.jar", &[("fabric.mod.json", b"{not json")]);
        assert_eq!(inspect_mod_jar(&bad, None).unwrap_err(), InspectError::InvalidMetadata);
        let bad_id = jar(dir.path(), "id.jar", &[("fabric.mod.json", br#"{"id":"Bad Id!"}"#)]);
        assert_eq!(inspect_mod_jar(&bad_id, None).unwrap_err(), InspectError::InvalidMetadata);
        let not_zip = dir.path().join("text.jar");
        std::fs::write(&not_zip, b"not a zip").unwrap();
        assert_eq!(inspect_mod_jar(&not_zip, None).unwrap_err(), InspectError::CorruptJar);
        assert_eq!(
            inspect_mod_jar(&dir.path().join("gone.jar"), None).unwrap_err(),
            InspectError::UnreadableJar
        );
        assert_eq!((clean_id(" Iris "), clean_id("my mod"), clean_id("")), (Some("iris".into()), None, None));
    }
}
