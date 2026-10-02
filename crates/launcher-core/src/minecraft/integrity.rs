//! Whether an installed version is ready to launch. Launches check files by
//! presence and size only; the Components page's Verify also compares every known hash
//! too. An incomplete base version is reported, so it is repaired instead of skipped.

use std::fs;
use std::path::Path;

use super::assets::{AssetIndex, index_path};
use super::library::plan_libraries;
use super::platform::GamePlatform;
use super::version::{VersionInfo, load_merged, read_version_json, version_dir};
use crate::java::runtime::{JavaRuntimes, LEGACY_COMPONENT};
use crate::net::downloader::{ExpectedHash, hash_file};

pub const INSTALLING_MARKER: &str = ".launcher-installing";
pub const INSTALLED_MARKER: &str = ".launcher-installed";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentsCheck {
    pub manifest: bool,
    pub jar: bool,
    pub libraries: bool,
    pub natives: bool,
    pub assets: bool,
    pub java: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionCheck {
    pub valid: bool,
    pub issues: Vec<String>,
    pub components: ComponentsCheck,
    /// The Java runtime the version asks for (known once its manifest reads).
    pub java_component: Option<String>,
}

/// `versions/<id>/<id>.json` exists, parses and names `id`.
pub fn is_version_installed(mc_dir: &Path, id: &str) -> bool {
    read_version_json(mc_dir, id).is_ok()
}

/// Present as the downloader sees it: a file, of the right size when the size is known.
fn present(path: &Path, size: Option<u64>) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && size.is_none_or(|s| s == m.len()))
}

/// `present`, and with `deep` also matching `hash` when one is known.
fn intact(path: &Path, size: Option<u64>, hash: Option<&ExpectedHash>, deep: bool) -> bool {
    present(path, size)
        && (!deep || hash.is_none_or(|h| hash_file(path, h.kind).is_ok_and(|actual| actual == h.hex)))
}

/// Checks a version and everything it inherits; `java` also checks the managed runtime it names
/// (where Mojang has none for this platform, that part passes). `deep` compares every file whose
/// hash is known — slow, for Verify. Blocking.
pub fn check_version(
    mc_dir: &Path,
    id: &str,
    platform: &GamePlatform,
    java: Option<&JavaRuntimes>,
    deep: bool,
) -> VersionCheck {
    let mut check = VersionCheck::default();
    let dir = version_dir(mc_dir, id);
    let interrupted = dir.join(INSTALLING_MARKER).exists() && !dir.join(INSTALLED_MARKER).exists();
    if interrupted {
        check.issues.push("the last install was interrupted".into());
    }
    let info = match load_merged(mc_dir, id).and_then(|json| VersionInfo::from_json(&json)) {
        Ok(info) if info.main_class.is_some() => info,
        Ok(_) => {
            check.issues.push("manifest: no mainClass".into());
            return check;
        }
        Err(e) => {
            check.issues.push(format!("manifest: {}", e.detail));
            return check;
        }
    };
    check.components.manifest = true;
    let component = info.java_component.clone().unwrap_or_else(|| LEGACY_COMPONENT.to_string());
    check.java_component = Some(component.clone());

    let jar = mc_dir.join("versions").join(&info.jar_id).join(format!("{}.jar", info.jar_id));
    // Installs download the jar only when the version names one (or it comes from another version).
    let needs_jar = info.client.is_some() || info.jar_id != info.id;
    let client_hash = info.client.as_ref().and_then(|c| c.sha1.as_deref()).map(ExpectedHash::sha1);
    check.components.jar =
        !needs_jar || intact(&jar, info.client.as_ref().and_then(|c| c.size), client_hash.as_ref(), deep);
    if !check.components.jar {
        check.issues.push(format!("jar: {} is missing or damaged", jar.display()));
    }

    match plan_libraries(&info.libraries, &mc_dir.join("libraries"), "", platform) {
        Ok(plan) => {
            let missing =
                plan.tasks.iter().filter(|t| !intact(&t.dest, t.size, t.hash.as_ref(), deep)).count();
            check.components.libraries = missing == 0;
            if missing > 0 {
                check.issues.push(format!("libraries: {missing} missing or damaged"));
            }
            let unpacked = fs::read_dir(dir.join("natives")).is_ok_and(|mut d| d.next().is_some());
            check.components.natives = plan.natives.is_empty() || unpacked;
            if !check.components.natives {
                check.issues.push("natives: not unpacked".into());
            }
        }
        Err(e) => check.issues.push(format!("libraries: {}", e.detail)),
    }

    let assets_dir = mc_dir.join("assets");
    let objects = match (&info.asset_index_name, &info.asset_index) {
        (Some(name), Some(_)) => match AssetIndex::read(&index_path(&assets_dir, name)) {
            Ok(index) => {
                let missing = index.damaged_objects(&assets_dir, deep);
                if missing > 0 {
                    check.issues.push(format!("assets: {missing} missing or damaged"));
                }
                missing == 0
            }
            Err(e) => {
                check.issues.push(format!("assets: {}", e.detail));
                false
            }
        },
        // Without `assetIndex` installs fetch no assets, so there is nothing to require.
        _ => true,
    };
    let log = info.log_config.as_ref().and_then(|file| file.id.as_ref().map(|name| (file, name))).is_none_or(
        |(file, name)| {
            let hash = file.sha1.as_deref().map(ExpectedHash::sha1);
            intact(&assets_dir.join("log_configs").join(name), file.size, hash.as_ref(), deep)
        },
    );
    if !log {
        check.issues.push("assets: the log config is missing".into());
    }
    check.components.assets = objects && log;

    check.components.java = match java {
        None => true,
        Some(java) => java.unavailable(&component) || java.is_complete(&component, deep),
    };
    if !check.components.java {
        check.issues.push(format!("java: {component} is missing or damaged"));
    }

    let c = &check.components;
    check.valid = !interrupted && c.manifest && c.jar && c.libraries && c.natives && c.assets && c.java;
    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha1::{Digest, Sha1};
    use std::path::PathBuf;

    struct Layout {
        _tmp: tempfile::TempDir,
        mc: PathBuf,
        object: PathBuf,
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A fully installed version `1.0`, laid out by hand.
    fn installed() -> Layout {
        let tmp = tempfile::tempdir().unwrap();
        let mc = tmp.path().join("mc");
        let hash = hex::encode(Sha1::digest(b"obj"));
        let sha = |bytes: &[u8]| hex::encode(Sha1::digest(bytes));
        let json = json!({
            "id": "1.0", "mainClass": "M", "assets": "1",
            "assetIndex": {"id": "1", "url": "https://x.example/1.json"},
            "downloads": {"client": {"url": "https://x.example/c.jar", "size": 6, "sha1": sha(b"client")}},
            "logging": {"client": {"file": {"id": "log.xml", "url": "https://x.example/l.xml", "size": 3, "sha1": sha(b"log")}}},
            "libraries": [
                {"name": "a:lib:1", "downloads": {"artifact": {"path": "a/lib/1/lib-1.jar", "url": "https://x.example/lib.jar", "size": 3, "sha1": sha(b"lib")}}},
                {"name": "n:nat:1", "natives": {"windows": "natives", "osx": "natives", "linux": "natives"},
                 "downloads": {"classifiers": {"natives": {"path": "n/nat/1/nat-1-natives.jar", "url": "https://x.example/n.jar", "size": 1}}}}
            ]
        });
        let versions = mc.join("versions").join("1.0");
        write(&versions.join("1.0.json"), json.to_string().as_bytes());
        write(&versions.join("1.0.jar"), b"client");
        write(&versions.join("natives").join("x.dll"), b"x");
        write(&versions.join(INSTALLED_MARKER), b"");
        write(&mc.join("libraries/a/lib/1/lib-1.jar"), b"lib");
        write(&mc.join("libraries/n/nat/1/nat-1-natives.jar"), b"n");
        let index = json!({"objects": {"a/b.ogg": {"hash": hash, "size": 3}}});
        write(&mc.join("assets/indexes/1.json"), index.to_string().as_bytes());
        let object = mc.join("assets/objects").join(&hash[..2]).join(&hash);
        write(&object, b"obj");
        write(&mc.join("assets/log_configs/log.xml"), b"log");
        Layout { _tmp: tmp, mc, object }
    }

    /// Breaks one part of an installed layout.
    type Damage = fn(&Layout);

    fn check(layout: &Layout) -> VersionCheck {
        check_version(&layout.mc, "1.0", &GamePlatform::current(), None, false)
    }

    #[test]
    fn a_complete_version_is_valid() {
        let layout = installed();
        let result = check(&layout);
        assert!(result.valid, "{:?}", result.issues);
        assert_eq!(result.java_component.as_deref(), Some(LEGACY_COMPONENT));
        assert!(is_version_installed(&layout.mc, "1.0"));
        assert!(!is_version_installed(&layout.mc, "2.0"));
    }

    #[test]
    fn each_missing_part_is_named() {
        let cases: [(&str, Damage); 5] = [
            ("jar", |l| fs::write(l.mc.join("versions/1.0/1.0.jar"), b"short").unwrap()),
            ("libraries", |l| fs::remove_file(l.mc.join("libraries/a/lib/1/lib-1.jar")).unwrap()),
            ("natives", |l| fs::remove_dir_all(l.mc.join("versions/1.0/natives")).unwrap()),
            ("assets", |l| fs::remove_file(&l.object).unwrap()),
            ("log", |l| fs::remove_file(l.mc.join("assets/log_configs/log.xml")).unwrap()),
        ];
        for (part, damage) in cases {
            let layout = installed();
            damage(&layout);
            let result = check(&layout);
            assert!(!result.valid, "{part}");
            let c = &result.components;
            let flag = match part {
                "jar" => c.jar,
                "libraries" => c.libraries,
                "natives" => c.natives,
                _ => c.assets,
            };
            assert!(!flag && c.manifest, "{part}: {c:?}");
            assert!(!result.issues.is_empty(), "{part}");
        }
    }

    #[test]
    fn an_interrupted_install_is_invalid() {
        let layout = installed();
        let versions = layout.mc.join("versions/1.0");
        fs::remove_file(versions.join(INSTALLED_MARKER)).unwrap();
        fs::write(versions.join(INSTALLING_MARKER), b"").unwrap();
        let result = check(&layout);
        assert!(!result.valid);
        assert!(result.components.jar && result.components.libraries, "the files themselves are fine");
        assert!(result.issues.iter().any(|i| i.contains("interrupted")), "{:?}", result.issues);
    }

    #[test]
    fn a_broken_manifest_stops_the_check() {
        let layout = installed();
        fs::write(layout.mc.join("versions/1.0/1.0.json"), "{ broken").unwrap();
        let result = check(&layout);
        assert_eq!((result.valid, result.components.manifest), (false, false));
        assert!(!is_version_installed(&layout.mc, "1.0"));
    }

    #[test]
    fn a_deep_check_catches_same_size_damage() {
        let cases: [(&str, Damage); 4] = [
            ("jar", |l| fs::write(l.mc.join("versions/1.0/1.0.jar"), b"CLIENT").unwrap()),
            ("libraries", |l| fs::write(l.mc.join("libraries/a/lib/1/lib-1.jar"), b"LIB").unwrap()),
            ("assets", |l| fs::write(&l.object, b"OBJ").unwrap()),
            ("log", |l| fs::write(l.mc.join("assets/log_configs/log.xml"), b"LOG").unwrap()),
        ];
        let deep = |layout: &Layout| check_version(&layout.mc, "1.0", &GamePlatform::current(), None, true);
        assert!(deep(&installed()).valid, "an intact version passes the deep check");
        for (part, damage) in cases {
            let layout = installed();
            damage(&layout);
            assert!(check(&layout).valid, "{part}: the same size passes the quick check");
            let result = deep(&layout);
            assert!(!result.valid && !result.issues.is_empty(), "{part}: {:?}", result.issues);
        }
    }
}
