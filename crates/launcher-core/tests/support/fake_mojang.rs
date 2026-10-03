//! A local Mojang for install tests: version list, version JSONs, libraries, assets and Java
//! runtimes served by `fake_files`, every URL pointing back at it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::sync::Mutex;

use launcher_core::loaders::LoaderEndpoints;
use launcher_core::minecraft::library::library_path;
use launcher_core::minecraft::manifest::MojangEndpoints;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

use super::fake_files::{self, FileServer, Served};

pub const JAVA_COMPONENT: &str = "java-runtime-delta";
pub const JAVA_BYTES: &[u8] = b"#!java launcher";
pub const PLATFORM_KEYS: [&str; 8] = [
    "gamecore",
    "linux",
    "linux-i386",
    "mac-os",
    "mac-os-arm64",
    "windows-arm64",
    "windows-x64",
    "windows-x86",
];

pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

/// The runtime's launcher as Mojang names it on this OS.
pub fn java_file() -> &'static str {
    if cfg!(windows) { "bin/java.exe" } else { "bin/java" }
}

pub fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

pub fn zip_entries(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        zip.start_file(name.as_str(), options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// `libraries/`-relative path of Maven `coords`, with `/`.
pub fn maven_path(coords: &str) -> String {
    library_path(coords).unwrap().to_string_lossy().replace('\\', "/")
}

/// How a fake Forge or NeoForge installer is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerStyle {
    /// `spec` 1: a server-only processor, then a client one that reads `{MINECRAFT_JAR}` and
    /// `{BINPATCH}` and writes `{MC_SLIM}` and `{PATCHED}` (checked by SHA-1).
    Processors,
    /// `spec` 0 without processors; the loader jar ships in `maven/` with an empty URL (Forge 1.12.2).
    MavenOnly,
    /// `install` + `versionInfo`; the universal jar lies in the installer's root (Forge 1.7.10).
    Legacy,
}

/// What the fake patcher writes to `{PATCHED}`.
pub const PATCHED_BYTES: &[u8] = b"patched client";
pub const PROCESSOR_MAIN: &str = "net.fake.installer.Patcher";

/// What `add_installer` published.
pub struct FakeInstaller {
    /// The id the launcher installs it as.
    pub id: String,
    /// The installer's `.sha1` on the server (overwrite it to break the checksum).
    pub sidecar: String,
    /// Under `libraries/`: what the client processor writes (`Processors`).
    pub patched_path: String,
    /// Under `libraries/`: the loader jar the installer ships (`MavenOnly`, `Legacy`).
    pub loader_path: String,
}

pub fn lzma_bytes(raw: &[u8]) -> Vec<u8> {
    let mut packed = Vec::new();
    lzma_rs::lzma_compress(&mut Cursor::new(raw), &mut packed).unwrap();
    packed
}

pub enum RuntimeEntry {
    File { bytes: Vec<u8>, executable: bool, lzma: bool },
    Dir,
    Link(&'static str),
}

/// What `add_vanilla` published, for assertions.
pub struct FakeVersion {
    pub id: String,
    pub client: Vec<u8>,
    /// Under `libraries/`.
    pub library_path: &'static str,
    pub library: Vec<u8>,
    /// A library whose rules never match.
    pub skipped_library_path: &'static str,
    /// (hash, bytes) of every asset object.
    pub objects: Vec<(String, Vec<u8>)>,
    pub log_config: Vec<u8>,
}

/// Per loader kind: its game list and its loader list.
type LoaderLists = BTreeMap<String, (Vec<Value>, Vec<Value>)>;

/// Forge's and NeoForge's build lists.
#[derive(Default)]
struct ForgeLists {
    forge: Vec<String>,
    promos: Map<String, Value>,
    neoforge: Vec<String>,
}

pub struct FakeMojang {
    pub server: FileServer,
    pub endpoints: MojangEndpoints,
    versions: Mutex<Vec<Value>>,
    runtimes: Mutex<Map<String, Value>>,
    loader_lists: Mutex<LoaderLists>,
    forge_lists: Mutex<ForgeLists>,
}

impl FakeMojang {
    pub async fn start() -> FakeMojang {
        let server = fake_files::start().await;
        let endpoints = MojangEndpoints {
            version_manifest: server.url("mc/version_manifest_v2.json"),
            resources: server.url("resources"),
            libraries: server.url("maven"),
            java_runtimes: server.url("java/all.json"),
        };
        let fake = FakeMojang {
            server,
            endpoints,
            versions: Mutex::default(),
            runtimes: Mutex::default(),
            loader_lists: Mutex::default(),
            forge_lists: Mutex::default(),
        };
        fake.publish_manifest();
        fake.publish_runtimes();
        fake
    }

    /// Serves `bytes` at `path`; returns the URL.
    pub fn file(&self, path: &str, bytes: Vec<u8>) -> String {
        self.server.put(path, Served { body: bytes, ..Served::default() });
        self.server.url(path)
    }

    /// `{url, sha1, size}` of `bytes` served at `path`.
    pub fn entry(&self, path: &str, bytes: &[u8]) -> Value {
        json!({"url": self.file(path, bytes.to_vec()), "sha1": sha1_hex(bytes), "size": bytes.len()})
    }

    fn publish_manifest(&self) {
        let versions = self.versions.lock().unwrap().clone();
        let latest = versions.first().map(|v| v["id"].clone()).unwrap_or(Value::Null);
        let manifest = json!({"latest": {"release": latest, "snapshot": null}, "versions": versions});
        self.file("mc/version_manifest_v2.json", manifest.to_string().into_bytes());
    }

    fn publish_runtimes(&self) {
        let runtimes = Value::Object(self.runtimes.lock().unwrap().clone());
        let all: Map<String, Value> =
            PLATFORM_KEYS.iter().map(|key| (key.to_string(), runtimes.clone())).collect();
        self.file("java/all.json", serde_json::to_vec(&all).unwrap());
    }

    /// Lists `json` (a complete version JSON) in the version list.
    pub fn add_version_json(&self, json: &Value) {
        let id = json["id"].as_str().unwrap().to_string();
        let bytes = serde_json::to_vec_pretty(json).unwrap();
        let sha1 = sha1_hex(&bytes);
        let url = self.file(&format!("v1/packages/{sha1}/{id}.json"), bytes);
        let listed = json!({"id": id, "type": "release", "url": url, "sha1": sha1,
                            "releaseTime": json.get("releaseTime").cloned().unwrap_or(Value::Null)});
        self.versions.lock().unwrap().insert(0, listed);
        self.publish_manifest();
    }

    /// A small but complete vanilla version: a library, one for another OS, an old-style natives
    /// jar, three asset names over two objects, a log config, the client jar and Java
    /// `java-runtime-delta`.
    pub fn add_vanilla(&self, id: &str) -> FakeVersion {
        let client = format!("client of {id}").into_bytes();
        let library = b"core library".to_vec();
        let natives =
            zip_bytes(&[("lwjgl64.dll", "native code"), ("META-INF/MANIFEST.MF", "Manifest-Version: 1.0")]);
        let log_config = b"<Configuration/>".to_vec();
        let (icon, sound) = (b"png!".to_vec(), b"ogg sound".to_vec());
        let objects = vec![(sha1_hex(&icon), icon), (sha1_hex(&sound), sound)];
        for (hash, bytes) in &objects {
            self.file(&format!("resources/{}/{hash}", &hash[..2]), bytes.clone());
        }
        let index = json!({"objects": {
            "icons/icon_16x16.png": {"hash": objects[0].0, "size": objects[0].1.len()},
            "minecraft/sounds/a.ogg": {"hash": objects[1].0, "size": objects[1].1.len()},
            "minecraft/sounds/a_copy.ogg": {"hash": objects[1].0, "size": objects[1].1.len()}
        }});
        let mut asset_index = self.entry(&format!("indexes/{id}.json"), &serde_json::to_vec(&index).unwrap());
        asset_index["id"] = json!(id);
        let mut core = self.entry("libs/core.jar", &library);
        core["path"] = json!("com/example/core/1.0/core-1.0.jar");
        let mut other = self.entry("libs/other.jar", b"never downloaded");
        other["path"] = json!("com/example/other/1.0/other-1.0.jar");
        let mut native = self.entry("libs/natives.jar", &natives);
        native["path"] = json!("org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives.jar");
        let mut log = self.entry("logging/client-1.12.xml", &log_config);
        log["id"] = json!("client-1.12.xml");
        let version = json!({
            "id": id, "type": "release", "mainClass": "net.minecraft.client.main.Main",
            "releaseTime": "2024-08-08T12:24:45+00:00",
            "assets": id, "assetIndex": asset_index,
            "downloads": {"client": self.entry(&format!("clients/{id}.jar"), &client)},
            "javaVersion": {"component": JAVA_COMPONENT, "majorVersion": 21},
            "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "file": log, "type": "log4j2-xml"}},
            "arguments": {
                "game": ["--username", "${auth_player_name}", "--version", "${version_name}", "--gameDir", "${game_directory}",
                         "--assetsDir", "${assets_root}", "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}",
                         "--accessToken", "${auth_access_token}", "--clientId", "${clientid}", "--xuid", "${auth_xuid}",
                         "--userType", "${user_type}", "--versionType", "${version_type}"],
                "jvm": ["-Djava.library.path=${natives_directory}", "-Dminecraft.launcher.brand=${launcher_name}", "-cp", "${classpath}"]
            },
            "libraries": [
                {"name": "com.example:core:1.0", "downloads": {"artifact": core}},
                {"name": "com.example:other:1.0", "downloads": {"artifact": other},
                 "rules": [{"action": "allow", "os": {"name": "no-such-os"}}]},
                {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
                 "natives": {"windows": "natives", "osx": "natives", "linux": "natives"},
                 "extract": {"exclude": ["META-INF/"]},
                 "downloads": {"classifiers": {"natives": native}}}
            ]
        });
        self.add_version_json(&version);
        if !self.has_runtime(JAVA_COMPONENT) {
            self.add_default_runtime(JAVA_COMPONENT);
        }
        FakeVersion {
            id: id.to_string(),
            client,
            library_path: "com/example/core/1.0/core-1.0.jar",
            library,
            skipped_library_path: "com/example/other/1.0/other-1.0.jar",
            objects,
            log_config,
        }
    }

    pub fn has_runtime(&self, component: &str) -> bool {
        self.runtimes.lock().unwrap().contains_key(component)
    }

    /// Publishes `component` for every platform with a manifest listing `files`.
    pub fn add_runtime(&self, component: &str, version: &str, files: &[(&str, RuntimeEntry)]) {
        let mut listed = Map::new();
        for (path, entry) in files {
            let value = match entry {
                RuntimeEntry::Dir => json!({"type": "directory"}),
                RuntimeEntry::Link(target) => json!({"type": "link", "target": target}),
                RuntimeEntry::File { bytes, executable, lzma } => {
                    let mut downloads =
                        json!({"raw": self.entry(&format!("java/{component}/raw/{path}"), bytes)});
                    if *lzma {
                        downloads["lzma"] =
                            self.entry(&format!("java/{component}/lzma/{path}"), &lzma_bytes(bytes));
                    }
                    json!({"type": "file", "executable": executable, "downloads": downloads})
                }
            };
            listed.insert(path.to_string(), value);
        }
        let manifest = serde_json::to_vec(&json!({"files": listed})).unwrap();
        let entry = json!([{
            "availability": {"group": 1, "progress": 100},
            "manifest": self.entry(&format!("java/{component}/manifest.json"), &manifest),
            "version": {"name": version, "released": "2024-04-16T00:00:00+00:00"}
        }]);
        self.runtimes.lock().unwrap().insert(component.to_string(), entry);
        self.publish_runtimes();
    }

    /// Lists `component` with no builds (as Mojang does for some platforms).
    pub fn add_runtime_without_builds(&self, component: &str) {
        self.runtimes.lock().unwrap().insert(component.to_string(), json!([]));
        self.publish_runtimes();
    }

    /// `bin/java[.exe]` (lzma), `lib/modules` (raw), `release` (lzma), a directory and a link.
    pub fn add_default_runtime(&self, component: &str) {
        self.add_runtime(
            component,
            "21.0.3",
            &[
                ("bin", RuntimeEntry::Dir),
                (
                    java_file(),
                    RuntimeEntry::File { bytes: JAVA_BYTES.to_vec(), executable: true, lzma: true },
                ),
                (
                    "lib/modules",
                    RuntimeEntry::File { bytes: vec![7u8; 3000], executable: false, lzma: false },
                ),
                (
                    "release",
                    RuntimeEntry::File {
                        bytes: b"JAVA_VERSION=\"21.0.3\"".to_vec(),
                        executable: false,
                        lzma: true,
                    },
                ),
                ("bin/java-link", RuntimeEntry::Link("java")),
            ],
        );
    }

    /// Fabric- or Quilt-style meta (`kind` = `fabric` | `quilt`): Minecraft `mc` in the game list,
    /// loader `lv` in the loader list (Quilt lists no `stable` flag), and the profile JSON of
    /// `lv` for `mc` with one Maven library served here.
    pub fn add_loader(&self, kind: &str, mc: &str, lv: &str, stable: bool) {
        let fabric = kind == "fabric";
        let root = if fabric { "fabric/v2" } else { "quilt/v3" };
        let (group, artifact) =
            if fabric { ("net.fabricmc", "fabric-loader") } else { ("org.quiltmc", "quilt-loader") };
        let jar = format!("{kind} loader {lv}").into_bytes();
        let path = format!("{}/{artifact}/{lv}/{artifact}-{lv}.jar", group.replace('.', "/"));
        let entry = self.entry(&format!("{kind}-maven/{path}"), &jar);
        let main_class = if fabric {
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        } else {
            "org.quiltmc.loader.impl.launch.knot.KnotClient"
        };
        let profile = json!({
            "id": format!("{kind}-loader-{lv}-{mc}"),
            "inheritsFrom": mc,
            "type": "release",
            "mainClass": main_class,
            "arguments": {"game": [], "jvm": []},
            "libraries": [{
                "name": format!("{group}:{artifact}:{lv}"),
                "url": self.server.url(&format!("{kind}-maven/")),
                "sha1": entry["sha1"],
                "size": entry["size"]
            }]
        });
        self.file(
            &format!("{root}/versions/loader/{mc}/{lv}/profile/json"),
            serde_json::to_vec(&profile).unwrap(),
        );
        let mut lists = self.loader_lists.lock().unwrap();
        let (games, loaders) = lists.entry(kind.to_string()).or_default();
        if !games.iter().any(|g| g["version"] == mc) {
            games.insert(0, json!({"version": mc, "stable": true}));
        }
        loaders.insert(
            0,
            if fabric { json!({"version": lv, "stable": stable}) } else { json!({"version": lv}) },
        );
        self.file(&format!("{root}/versions/game"), serde_json::to_vec(&*games).unwrap());
        self.file(&format!("{root}/versions/loader"), serde_json::to_vec(&*loaders).unwrap());
    }

    /// Lists Forge build `lv` for Minecraft `mc` in `maven-metadata.xml`.
    pub fn list_forge(&self, mc: &str, lv: &str) {
        let mut lists = self.forge_lists.lock().unwrap();
        lists.forge.push(format!("{mc}-{lv}"));
        let versions: String = lists.forge.iter().map(|v| format!("<version>{v}</version>")).collect();
        let xml = format!(
            "<metadata><groupId>net.minecraftforge</groupId><versioning><versions>{versions}</versions></versioning></metadata>"
        );
        self.file("forge-maven/net/minecraftforge/forge/maven-metadata.xml", xml.into_bytes());
    }

    /// Makes `lv` Forge's recommended build for `mc`.
    pub fn recommend_forge(&self, mc: &str, lv: &str) {
        let mut lists = self.forge_lists.lock().unwrap();
        lists.promos.insert(format!("{mc}-recommended"), json!(lv));
        let promotions = json!({"homepage": "https://files.minecraftforge.net/", "promos": lists.promos});
        self.file("forge-files/promotions_slim.json", promotions.to_string().into_bytes());
    }

    /// Lists NeoForge build `lv` (its version API lists oldest first).
    pub fn list_neoforge(&self, lv: &str) {
        let mut lists = self.forge_lists.lock().unwrap();
        lists.neoforge.push(lv.to_string());
        let list = json!({"isSnapshot": false, "versions": lists.neoforge});
        self.file(
            "neoforge-maven/api/maven/versions/releases/net/neoforged/neoforge",
            list.to_string().into_bytes(),
        );
    }

    /// A Forge (`kind` = `forge`) or NeoForge (`neoforge`) build `lv` for Minecraft `mc`: listed,
    /// with its installer (and `.sha1`) and the libraries it needs served here.
    pub fn add_installer(&self, kind: &str, mc: &str, lv: &str, style: InstallerStyle) -> FakeInstaller {
        self.add_installer_claiming(kind, mc, lv, style, mc)
    }

    /// `add_installer` whose profile and version JSON say it is for Minecraft `claimed`.
    pub fn add_installer_claiming(
        &self,
        kind: &str,
        mc: &str,
        lv: &str,
        style: InstallerStyle,
        claimed: &str,
    ) -> FakeInstaller {
        let neo = kind == "neoforge";
        let (root, coords, id) = if neo {
            ("neoforge-maven/releases", format!("net.neoforged:neoforge:{lv}"), format!("neoforge-{lv}"))
        } else {
            ("forge-maven", format!("net.minecraftforge:forge:{mc}-{lv}"), format!("{mc}-forge-{lv}"))
        };
        if neo {
            self.list_neoforge(lv);
        } else {
            self.list_forge(mc, lv);
        }
        let library = |coords: &str, bytes: &[u8]| -> Value {
            let path = maven_path(coords);
            let mut artifact = self.entry(&format!("{root}/{path}"), bytes);
            artifact["path"] = json!(path);
            json!({"name": coords, "downloads": {"artifact": artifact}})
        };
        let loader_path = maven_path(&coords);
        // A real jar: one known by name alone counts only while it opens.
        let loader_jar = zip_bytes(&[("META-INF/MANIFEST.MF", &format!("{kind} {lv}"))]);
        let mut libraries = vec![library("net.fake:boot:1.0", b"boot")];
        let version = |libraries: Vec<Value>| {
            json!({
                "id": id, "inheritsFrom": claimed, "type": "release",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "arguments": {"game": ["--launchTarget", "forgeclient"],
                              "jvm": ["-p", "${library_directory}/net/fake/boot/1.0/boot-1.0.jar"]},
                "libraries": libraries
            })
        };
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let profile = match style {
            InstallerStyle::Processors => {
                let manifest = format!("Manifest-Version: 1.0\r\nMain-Class: {PROCESSOR_MAIN}\r\n");
                let patcher = zip_bytes(&[("META-INF/MANIFEST.MF", manifest.as_str())]);
                entries.push(("version.json".into(), serde_json::to_vec(&version(libraries)).unwrap()));
                entries.push(("data/client.lzma".into(), b"binary patches".to_vec()));
                json!({
                    "spec": 1, "minecraft": claimed, "json": "/version.json", "version": id,
                    "data": {
                        "BINPATCH": {"client": "/data/client.lzma", "server": "/data/server.lzma"},
                        "PATCHED": {"client": format!("[{coords}:client]"), "server": format!("[{coords}:server]")},
                        "PATCHED_SHA": {"client": format!("'{}'", sha1_hex(PATCHED_BYTES)), "server": "''"},
                        "MC_SLIM": {"client": format!("[net.minecraft:client:{mc}-fake:slim]"),
                                    "server": format!("[net.minecraft:server:{mc}-fake:slim]")}
                    },
                    "processors": [
                        {"sides": ["server"], "jar": "net.fake:patcher:1.0", "args": ["--task", "SERVER_ONLY"]},
                        {"jar": "net.fake:patcher:1.0", "classpath": ["net.fake:patcher-lib:1.0"],
                         "args": ["--clean", "{MINECRAFT_JAR}", "--apply", "{BINPATCH}", "--slim", "{MC_SLIM}",
                                  "--output", "{PATCHED}"],
                         "outputs": {"{PATCHED}": "{PATCHED_SHA}"}}
                    ],
                    "libraries": [library("net.fake:patcher:1.0", &patcher), library("net.fake:patcher-lib:1.0", b"patcher lib")]
                })
            }
            InstallerStyle::MavenOnly => {
                libraries.push(json!({"name": coords, "downloads": {"artifact": {
                    "path": loader_path, "url": "", "sha1": sha1_hex(&loader_jar), "size": loader_jar.len()}}}));
                entries.push(("version.json".into(), serde_json::to_vec(&version(libraries)).unwrap()));
                entries.push((format!("maven/{loader_path}"), loader_jar.clone()));
                json!({"spec": 0, "minecraft": claimed, "json": "/version.json", "path": coords,
                       "data": {}, "processors": [], "libraries": []})
            }
            InstallerStyle::Legacy => {
                let universal = format!("forge-{mc}-{lv}-universal.jar");
                entries.push((universal.clone(), loader_jar.clone()));
                let repo = self.server.url(&format!("{root}/"));
                json!({
                    "install": {"profileName": "Forge", "target": id, "path": coords, "filePath": universal,
                                "minecraft": claimed},
                    "versionInfo": {
                        "id": format!("{mc}-Forge{lv}"), "inheritsFrom": claimed, "type": "release",
                        "mainClass": "net.minecraft.launchwrapper.Launch",
                        "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --tweakClass cpw.mods.fml.common.launcher.FMLTweaker",
                        "libraries": [{"name": coords, "url": repo}, {"name": "net.fake:boot:1.0", "url": repo}]
                    }
                })
            }
        };
        entries.push(("install_profile.json".into(), serde_json::to_vec(&profile).unwrap()));
        let installer = zip_entries(&entries);
        let path = format!("{root}/{}", maven_path(&format!("{coords}:installer")));
        self.file(&path, installer.clone());
        let sidecar = format!("{path}.sha1");
        self.file(&sidecar, sha1_hex(&installer).into_bytes());
        FakeInstaller { id, sidecar, patched_path: maven_path(&format!("{coords}:client")), loader_path }
    }

    pub fn loader_endpoints(&self) -> LoaderEndpoints {
        LoaderEndpoints {
            fabric: self.server.url("fabric/v2"),
            quilt: self.server.url("quilt/v3"),
            forge: self.server.url("forge-maven"),
            forge_promotions: self.server.url("forge-files/promotions_slim.json"),
            neoforge: self.server.url("neoforge-maven"),
        }
    }
}
