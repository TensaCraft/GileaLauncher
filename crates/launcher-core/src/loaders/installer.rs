//! Forge and NeoForge installer jars: `install_profile.json` in its two shapes,
//! the version JSON, the jars shipped in `maven/` and files for the processors.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Map, Value};
use zip::ZipArchive;

use crate::safe_path::safe_relative;

/// One step of a modern installer: `java -cp <jar + classpath> <Main-Class> <args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processor {
    pub jar: String,
    pub classpath: Vec<String>,
    pub args: Vec<String>,
    /// Output → expected SHA-1, both still with `{TOKENS}`.
    pub outputs: BTreeMap<String, String>,
    /// `None` runs on both sides.
    pub sides: Option<Vec<String>>,
}

impl Processor {
    pub fn runs_on_client(&self) -> bool {
        self.sides.as_ref().is_none_or(|sides| sides.iter().any(|side| side == "client"))
    }
}

/// `spec` 0 and 1 profiles (Forge 1.12.2 re-releases and newer, every NeoForge).
#[derive(Debug, Clone, PartialEq)]
pub struct ModernProfile {
    pub minecraft: String,
    /// The version JSON inside the jar, without the leading `/`.
    pub json: String,
    /// The client values of `data`.
    pub data: BTreeMap<String, String>,
    pub processors: Vec<Processor>,
    pub libraries: Vec<Value>,
}

/// Old Forge (1.7.10 and the like): the version JSON is `versionInfo`, the loader jar lies in the
/// installer's root and is copied to its Maven path.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyProfile {
    pub minecraft: String,
    pub version: Map<String, Value>,
    /// The loader jar in the installer (`install.filePath`).
    pub file_path: String,
    /// Its Maven coordinates (`install.path`).
    pub coords: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstallProfile {
    Modern(ModernProfile),
    Legacy(LegacyProfile),
}

impl InstallProfile {
    /// The Minecraft version the installer is for.
    pub fn minecraft(&self) -> &str {
        match self {
            InstallProfile::Modern(profile) => &profile.minecraft,
            InstallProfile::Legacy(profile) => &profile.minecraft,
        }
    }
}

fn invalid(what: impl Into<String>) -> AppError {
    let what = what.into();
    AppError::new(ErrorCode::InvalidInput, format!("installer: {what}")).with_param("error", what)
}

fn io_error(path: &Path, e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn processor(value: &Value) -> AppResult<Processor> {
    Ok(Processor {
        jar: text(value, "jar").ok_or_else(|| invalid("a processor without a jar"))?,
        classpath: texts(value.get("classpath")),
        args: texts(value.get("args")),
        outputs: value
            .get("outputs")
            .and_then(Value::as_object)
            .map(|outputs| {
                outputs.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()
            })
            .unwrap_or_default(),
        sides: value.get("sides").map(|sides| texts(Some(sides))),
    })
}

/// Reads `install_profile.json`.
pub fn parse_profile(json: &Value) -> AppResult<InstallProfile> {
    if let (Some(install), Some(Value::Object(version))) = (json.get("install"), json.get("versionInfo")) {
        return Ok(InstallProfile::Legacy(LegacyProfile {
            minecraft: text(install, "minecraft").ok_or_else(|| invalid("no Minecraft version"))?,
            version: version.clone(),
            file_path: text(install, "filePath").ok_or_else(|| invalid("no loader jar"))?,
            coords: text(install, "path").ok_or_else(|| invalid("no loader coordinates"))?,
        }));
    }
    let minecraft = text(json, "minecraft").ok_or_else(|| invalid("no Minecraft version"))?;
    let entry = text(json, "json").unwrap_or_else(|| "/version.json".into());
    let data = json
        .get("data")
        .and_then(Value::as_object)
        .map(|data| {
            data.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.get("client")?.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let processors = match json.get("processors").and_then(Value::as_array) {
        Some(list) => list.iter().map(processor).collect::<AppResult<Vec<_>>>()?,
        None => Vec::new(),
    };
    Ok(InstallProfile::Modern(ModernProfile {
        minecraft,
        json: entry.trim_start_matches('/').to_string(),
        data,
        processors,
        libraries: json.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default(),
    }))
}

/// Puts `bytes` at `dest`: a file already so is left alone, any other is replaced whole (never
/// rewritten in place: another build on this loader may be running with it on its classpath).
fn write_file(dest: &Path, bytes: &[u8]) -> AppResult<()> {
    let same = fs::metadata(dest).is_ok_and(|m| m.len() == bytes.len() as u64)
        && fs::read(dest).is_ok_and(|present| present == bytes);
    if same {
        return Ok(());
    }
    crate::storage::atomic::atomic_write(dest, bytes).map_err(|e| io_error(dest, e))
}

/// An installer jar opened for reading.
pub struct InstallerJar {
    path: PathBuf,
    archive: ZipArchive<BufReader<File>>,
}

impl InstallerJar {
    /// Opens `path`; a file that is not a zip is `DownloadFailed` (it gets downloaded again).
    pub fn open(path: &Path) -> AppResult<InstallerJar> {
        let broken = |why: String| {
            AppError::new(ErrorCode::DownloadFailed, format!("{}: {why}", path.display()))
                .with_param("error", why)
        };
        let file = File::open(path).map_err(|e| broken(e.to_string()))?;
        let archive = ZipArchive::new(BufReader::new(file)).map_err(|e| broken(e.to_string()))?;
        Ok(InstallerJar { path: path.to_path_buf(), archive })
    }

    fn read(&mut self, name: &str) -> AppResult<Vec<u8>> {
        let mut entry = self.archive.by_name(name).map_err(|_| invalid(format!("no {name}")))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| io_error(&self.path, e))?;
        Ok(bytes)
    }

    fn read_json(&mut self, name: &str) -> AppResult<Value> {
        serde_json::from_slice(&self.read(name)?).map_err(|e| invalid(format!("{name}: {e}")))
    }

    pub fn profile(&mut self) -> AppResult<InstallProfile> {
        parse_profile(&self.read_json("install_profile.json")?)
    }

    /// The version JSON the installer would write (still under its own id).
    pub fn version_json(&mut self, profile: &InstallProfile) -> AppResult<Map<String, Value>> {
        match profile {
            InstallProfile::Legacy(legacy) => Ok(legacy.version.clone()),
            InstallProfile::Modern(modern) => match self.read_json(&modern.json)? {
                Value::Object(version) => Ok(version),
                _ => Err(invalid(format!("{} is not an object", modern.json))),
            },
        }
    }

    /// Copies `entry` to `dest`.
    pub fn extract(&mut self, entry: &str, dest: &Path) -> AppResult<()> {
        let bytes = self.read(entry)?;
        write_file(dest, &bytes)
    }

    /// Unpacks `maven/**` into `libraries` (Forge ships its own jars there). Every entry is checked
    /// first: one that would land outside refuses the whole jar. Returns the files it wrote.
    pub fn extract_maven(&mut self, libraries: &Path) -> AppResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        for index in 0..self.archive.len() {
            let entry = self.archive.by_index(index).map_err(|e| io_error(&self.path, e))?;
            let Some(rest) = entry.name().strip_prefix("maven/") else { continue };
            if entry.is_dir() || rest.is_empty() {
                continue;
            }
            let relative =
                safe_relative(rest).ok_or_else(|| invalid(format!("unsafe entry {}", entry.name())))?;
            files.push((entry.name().to_string(), libraries.join(relative)));
        }
        for (name, dest) in &files {
            self.extract(name, dest)?;
        }
        Ok(files.into_iter().map(|(_, dest)| dest).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use serde_json::json;

    use super::*;

    fn jar(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let path = dir.join("installer.jar");
        fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
        path
    }

    #[test]
    fn modern_profiles_list_client_data_and_processors() {
        let profile = parse_profile(&json!({
            "spec": 1, "minecraft": "1.20.1", "json": "/version.json",
            "data": {
                "PATCHED": {"client": "[net.minecraftforge:forge:1.20.1-47.4.10:client]", "server": "[x:y:z]"},
                "SERVER_ONLY": {"server": "'x'"}
            },
            "processors": [
                {"sides": ["server"], "jar": "a:b:1", "args": ["--x"]},
                {"jar": "a:c:1", "classpath": ["a:d:1"], "args": ["--output", "{PATCHED}"],
                 "outputs": {"{PATCHED}": "{PATCHED_SHA}"}}
            ],
            "libraries": [{"name": "a:b:1"}]
        }))
        .unwrap();
        assert_eq!(profile.minecraft(), "1.20.1");
        let InstallProfile::Modern(p) = profile else { panic!("a modern profile") };
        assert_eq!(p.json, "version.json");
        assert_eq!(p.data.len(), 1, "server-only values are left out");
        assert!(!p.processors[0].runs_on_client() && p.processors[1].runs_on_client());
        assert_eq!(
            (p.processors[1].classpath.len(), p.processors[1].outputs["{PATCHED}"].as_str()),
            (1, "{PATCHED_SHA}")
        );
        assert_eq!(p.libraries.len(), 1);
    }

    #[test]
    fn legacy_profiles_keep_their_version_info() {
        let profile = parse_profile(&json!({
            "install": {"minecraft": "1.7.10", "path": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10",
                        "filePath": "forge-universal.jar"},
            "versionInfo": {"id": "1.7.10-Forge10.13.4.1614-1.7.10", "inheritsFrom": "1.7.10"}
        }))
        .unwrap();
        assert_eq!(profile.minecraft(), "1.7.10");
        let InstallProfile::Legacy(p) = profile else { panic!("a legacy profile") };
        assert_eq!(
            (p.file_path.as_str(), p.version["inheritsFrom"].as_str()),
            ("forge-universal.jar", Some("1.7.10"))
        );
        assert_eq!(parse_profile(&json!({"spec": 1})).unwrap_err().code, ErrorCode::InvalidInput);
    }

    #[test]
    fn a_jar_already_in_place_is_left_alone_and_a_changed_one_is_replaced_whole() {
        // Another build on this Forge may be running with the jar on its classpath.
        let dir = tempfile::tempdir().unwrap();
        let path = jar(dir.path(), &[("maven/net/f/forge/1/forge-1.jar", b"forge")]);
        let libraries = dir.path().join("libraries");
        let dest = libraries.join("net/f/forge/1/forge-1.jar");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"forge").unwrap();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(&dest).unwrap().set_modified(long_ago).unwrap();
        InstallerJar::open(&path).unwrap().extract_maven(&libraries).unwrap();
        assert_eq!(fs::metadata(&dest).unwrap().modified().unwrap(), long_ago, "not written again");
        fs::write(&dest, b"other").unwrap();
        InstallerJar::open(&path).unwrap().extract_maven(&libraries).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"forge");
        let temps = fs::read_dir(dest.parent().unwrap()).unwrap().count();
        assert_eq!(temps, 1, "no temp file stays beside it");
    }

    #[test]
    fn installer_jars_give_their_version_and_maven_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = jar(
            dir.path(),
            &[
                ("install_profile.json", br#"{"spec":0,"minecraft":"1.12.2","json":"/version.json"}"#),
                ("version.json", br#"{"id":"1.12.2-forge-14.23.5.2859","inheritsFrom":"1.12.2"}"#),
                (
                    "maven/net/minecraftforge/forge/1.12.2-14.23.5.2859/forge-1.12.2-14.23.5.2859.jar",
                    b"forge",
                ),
                ("data/client.lzma", b"patches"),
            ],
        );
        let mut installer = InstallerJar::open(&path).unwrap();
        let profile = installer.profile().unwrap();
        assert_eq!(installer.version_json(&profile).unwrap()["id"], "1.12.2-forge-14.23.5.2859");
        let libraries = dir.path().join("libraries");
        assert_eq!(installer.extract_maven(&libraries).unwrap().len(), 1);
        let jar =
            libraries.join("net/minecraftforge/forge/1.12.2-14.23.5.2859/forge-1.12.2-14.23.5.2859.jar");
        assert_eq!(fs::read(jar).unwrap(), b"forge");
        installer.extract("data/client.lzma", &dir.path().join("work/client.lzma")).unwrap();
        assert_eq!(fs::read(dir.path().join("work/client.lzma")).unwrap(), b"patches");
        assert_eq!(
            installer.extract("data/missing", &dir.path().join("x")).unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn hostile_maven_entries_are_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = jar(dir.path(), &[("maven/a/ok.jar", b"ok"), ("maven/../../evil.jar", b"evil")]);
        let libraries = dir.path().join("mc").join("libraries");
        let error = InstallerJar::open(&path).unwrap().extract_maven(&libraries).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(!libraries.exists(), "nothing is written");
        assert!(!dir.path().join("evil.jar").exists());
    }

    #[test]
    fn a_broken_jar_is_a_failed_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installer.jar");
        fs::write(&path, b"<html>not found</html>").unwrap();
        assert_eq!(InstallerJar::open(&path).err().map(|e| e.code), Some(ErrorCode::DownloadFailed));
    }
}
