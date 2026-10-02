//! Libraries of a version: where each lives under `libraries/`, what to download and which
//! natives jars to unpack (MLL `install_libraries`).

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::Value;

use super::platform::GamePlatform;
use super::rules::{Features, rules_pass};
use crate::net::downloader::{DownloadTask, ExpectedHash};
use crate::safe_path::safe_relative;

/// `group:artifact:version[:classifier…][@ext]` → `group/…/artifact/version/artifact-version[-classifier…].ext`
/// (MLL `get_library_path`); `None` when a coordinate is empty or not a plain name.
pub fn library_path(name: &str) -> Option<PathBuf> {
    let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coords.split(':').collect();
    let [group, artifact, version, extra @ ..] = parts.as_slice() else { return None };
    let suffix: String = extra.iter().map(|part| format!("-{part}")).collect();
    let file = format!("{artifact}-{version}{suffix}.{ext}");
    let mut components: Vec<&str> = group.split('.').collect();
    components.extend([*artifact, *version, file.as_str()]);
    let plain =
        |part: &&str| !part.is_empty() && *part != "." && *part != ".." && !part.contains(['/', '\\']);
    if !components.iter().all(plain) {
        return None;
    }
    safe_relative(&components.join("/"))
}

/// The natives classifier of an old-style library for this platform (`natives` map, `${arch}`).
pub fn natives_classifier(library: &Value, platform: &GamePlatform) -> Option<String> {
    let raw = library.get("natives")?.get(platform.rule_os())?.as_str()?;
    let classifier = raw.replace("${arch}", platform.natives_arch());
    (!classifier.is_empty()).then_some(classifier)
}

/// A natives jar and the entry prefixes not to unpack from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativesJar {
    pub jar: PathBuf,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryPlan {
    pub tasks: Vec<DownloadTask>,
    pub natives: Vec<NativesJar>,
}

fn strongest_hash(entry: &Value) -> Option<ExpectedHash> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).filter(|v| !v.is_empty());
    text("sha512")
        .map(ExpectedHash::sha512)
        .or_else(|| text("sha256").map(ExpectedHash::sha256))
        .or_else(|| text("sha1").map(ExpectedHash::sha1))
}

/// A `{url, sha1, size}` entry as a download into `dest`; `None` without a URL.
fn entry_task(entry: &Value, dest: PathBuf) -> Option<DownloadTask> {
    let url = entry.get("url").and_then(Value::as_str).filter(|u| !u.is_empty())?;
    let mut task = DownloadTask::new(url, dest);
    if let Some(size) = entry.get("size").and_then(Value::as_u64) {
        task = task.size(size);
    }
    if let Some(hash) = strongest_hash(entry) {
        task = task.hash(hash);
    }
    Some(task)
}

fn unsafe_path(raw: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("unsafe library path {raw:?} in version metadata"))
        .with_param("path", raw)
}

/// The Maven path of `name` under `libraries`.
fn maven_dest(name: &str, libraries: &Path) -> AppResult<(PathBuf, PathBuf)> {
    let relative = library_path(name).ok_or_else(|| unsafe_path(name))?;
    Ok((libraries.join(&relative), relative))
}

/// Where a `downloads.*` entry goes: its own `path`, else the Maven path of `name`.
fn entry_dest(entry: &Value, name: &str, libraries: &Path) -> AppResult<PathBuf> {
    match entry.get("path").and_then(Value::as_str) {
        Some(raw) => safe_relative(raw).map(|p| libraries.join(p)).ok_or_else(|| unsafe_path(raw)),
        None => maven_dest(name, libraries).map(|(dest, _)| dest),
    }
}

fn maven_url(repo: &str, relative: &Path) -> String {
    let parts: Vec<String> =
        relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    format!("{}/{}", repo.trim_end_matches('/'), parts.join("/"))
}

/// Downloads and natives jars of the libraries that apply to `platform`. A library with
/// `downloads.artifact` uses it; one without any `downloads` comes from its Maven `url` (or
/// `default_repo`) with the strongest hash it names; natives come from `downloads.classifiers`
/// or, failing that, from Maven.
pub fn plan_libraries(
    libraries: &[Value],
    libraries_dir: &Path,
    default_repo: &str,
    platform: &GamePlatform,
) -> AppResult<LibraryPlan> {
    let mut plan = LibraryPlan::default();
    for library in libraries {
        if !rules_pass(library.get("rules"), platform, &Features::default()) {
            continue;
        }
        let Some(name) = library.get("name").and_then(Value::as_str) else { continue };
        let repo =
            library.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).unwrap_or(default_repo);
        let downloads = library.get("downloads");
        match downloads.and_then(|d| d.get("artifact")) {
            Some(artifact) => {
                let dest = entry_dest(artifact, name, libraries_dir)?;
                plan.tasks.extend(entry_task(artifact, dest));
            }
            None if downloads.is_none() && library.get("natives").is_none() => {
                let (dest, relative) = maven_dest(name, libraries_dir)?;
                let mut task = DownloadTask::new(maven_url(repo, &relative), dest);
                if let Some(size) = library.get("size").and_then(Value::as_u64) {
                    task = task.size(size);
                }
                if let Some(hash) = strongest_hash(library) {
                    task = task.hash(hash);
                }
                plan.tasks.push(task);
            }
            None => {}
        }
        let Some(classifier) = natives_classifier(library, platform) else { continue };
        let native_name = format!("{name}:{classifier}");
        let classified = downloads.and_then(|d| d.get("classifiers")).and_then(|c| c.get(&classifier));
        let task = match classified {
            Some(entry) => entry_task(entry, entry_dest(entry, &native_name, libraries_dir)?),
            None => {
                let (dest, relative) = maven_dest(&native_name, libraries_dir)?;
                Some(DownloadTask::new(maven_url(repo, &relative), dest))
            }
        };
        let Some(task) = task else { continue };
        let exclude = library
            .pointer("/extract/exclude")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        plan.natives.push(NativesJar { jar: task.dest.clone(), exclude });
        plan.tasks.push(task);
    }
    Ok(plan)
}

/// Unpacks a natives jar into `dir`. Entries under an `exclude` prefix, directories and anything
/// that would land outside `dir` are skipped; a file already there with the same size is kept (a
/// running game may hold it open). Returns how many files were written.
pub fn extract_natives(jar: &Path, dir: &Path, exclude: &[String]) -> AppResult<usize> {
    let zip_error =
        |e: zip::result::ZipError| AppError::new(ErrorCode::Io, format!("{}: {e}", jar.display()));
    let io_error = |e: io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", jar.display()));
    let mut archive =
        zip::ZipArchive::new(BufReader::new(File::open(jar).map_err(io_error)?)).map_err(zip_error)?;
    fs::create_dir_all(dir).map_err(io_error)?;
    let mut written = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        if entry.is_dir() || exclude.iter().any(|prefix| entry.name().starts_with(prefix.as_str())) {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            tracing::warn!("Skipping unsafe entry {:?} in {}", entry.name(), jar.display());
            continue;
        };
        let target = dir.join(relative);
        if fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == entry.size()) {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut out = File::create(&target).map_err(io_error)?;
        io::copy(&mut entry, &mut out).map_err(io_error)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::platform::GameArch;
    use crate::paths::Os;
    use serde_json::json;
    use std::io::Write;

    fn windows() -> GamePlatform {
        GamePlatform { os: Os::Windows, arch: GameArch::X64, os_version: "10.0".into() }
    }

    fn slashes(path: Option<PathBuf>) -> Option<String> {
        path.map(|p| p.to_string_lossy().replace('\\', "/"))
    }

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, text) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn maven_names_become_paths() {
        assert_eq!(
            slashes(library_path("org.ow2.asm:asm:9.7.1")).as_deref(),
            Some("org/ow2/asm/asm/9.7.1/asm-9.7.1.jar")
        );
        assert_eq!(
            slashes(library_path("org.lwjgl:lwjgl:3.3.3:natives-windows")).as_deref(),
            Some("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")
        );
        assert_eq!(
            slashes(library_path("de.oceanlabs.mcp:mcp_config:1.20.1-20230612.114412@zip")).as_deref(),
            Some("de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412.zip")
        );
        for bad in ["only:two", "..:x:1", "a:..:1", "a:b:../../x", "a..b:c:1", "a:b:1@x/y"] {
            assert_eq!(library_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn natives_classifiers_follow_the_platform() {
        let lib = json!({"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
                         "natives": {"windows": "natives-windows-${arch}", "linux": "natives-linux"}});
        assert_eq!(natives_classifier(&lib, &windows()).as_deref(), Some("natives-windows-64"));
        let mac = GamePlatform { os: Os::MacOs, arch: GameArch::X64, os_version: String::new() };
        assert_eq!(natives_classifier(&lib, &mac), None);
    }

    #[test]
    fn a_plan_covers_artifacts_maven_libraries_and_natives() {
        let dir = Path::new("L");
        let libraries = vec![
            json!({"name": "com.example:core:1.0", "downloads": {"artifact": {
                "path": "com/example/core/1.0/core-1.0.jar", "url": "https://libs.example/core.jar",
                "sha1": "aa", "size": 10}}}),
            json!({"name": "com.example:mac:1.0", "downloads": {"artifact": {"path": "m.jar", "url": "https://libs.example/m.jar"}},
                   "rules": [{"action": "allow", "os": {"name": "osx"}}]}),
            json!({"name": "net.fabricmc:intermediary:1.21.1", "url": "https://maven.fabricmc.net/",
                   "sha1": "bb", "sha512": "cc", "size": 5}),
            json!({"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4", "natives": {"windows": "natives-windows"},
                   "extract": {"exclude": ["META-INF/"]},
                   "downloads": {"classifiers": {"natives-windows": {
                       "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-windows.jar",
                       "url": "https://libs.example/n.jar", "sha1": "dd", "size": 7}}}}),
            json!({"name": "only-classifiers:lib:1", "downloads": {"classifiers": {}}}),
        ];
        let plan = plan_libraries(&libraries, dir, "https://libraries.minecraft.net", &windows()).unwrap();
        let urls: Vec<&str> = plan.tasks.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://libs.example/core.jar",
                "https://maven.fabricmc.net/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
                "https://libs.example/n.jar",
            ]
        );
        let core = dir.join("com").join("example").join("core").join("1.0").join("core-1.0.jar");
        assert_eq!((plan.tasks[0].dest.clone(), plan.tasks[0].size), (core, Some(10)));
        assert_eq!(plan.tasks[1].hash, Some(ExpectedHash::sha512("cc")), "the strongest hash");
        assert_eq!(plan.tasks[1].size, Some(5));
        let natives = NativesJar { jar: plan.tasks[2].dest.clone(), exclude: vec!["META-INF/".into()] };
        assert_eq!(plan.natives, [natives]);
    }

    #[test]
    fn unsafe_library_paths_are_refused() {
        let libraries = vec![json!({"name": "a:b:1", "downloads": {"artifact": {
            "path": "../../evil.jar", "url": "https://x.example/e.jar"}}})];
        let err = plan_libraries(&libraries, Path::new("L"), "https://libraries.minecraft.net", &windows())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let bad_name = vec![json!({"name": "a:b:../../x"})];
        assert!(plan_libraries(&bad_name, Path::new("L"), "https://r.example", &windows()).is_err());
    }

    #[test]
    fn natives_are_unpacked_without_excluded_or_escaping_entries() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("natives.jar");
        write_zip(
            &jar,
            &[
                ("lwjgl64.dll", "dll"),
                ("META-INF/MANIFEST.MF", "m"),
                ("sub/libx.so", "so"),
                ("../evil.txt", "e"),
            ],
        );
        let out = dir.path().join("out").join("natives");
        let exclude = vec!["META-INF/".to_string()];
        assert_eq!(extract_natives(&jar, &out, &exclude).unwrap(), 2);
        assert_eq!(fs::read(out.join("lwjgl64.dll")).unwrap(), b"dll");
        assert!(out.join("sub").join("libx.so").is_file());
        assert!(!out.join("META-INF").exists());
        assert!(!dir.path().join("out").join("evil.txt").exists());
        assert_eq!(extract_natives(&jar, &out, &exclude).unwrap(), 0, "present files are kept");
    }
}
