//! The command that starts Minecraft (MLL `get_minecraft_command`), with two fixes over the
//! original: `${clientid}` and `${auth_xuid}` get real values, and versions that know Quick Play
//! join servers with it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use launcher_shared::{AppError, AppResult, ErrorCode};
use regex::Regex;
use serde_json::{Map, Value};

use super::library::{library_path, natives_classifier};
use super::platform::GamePlatform;
use super::rules::{Features, rules_pass};
use super::version::VersionInfo;
use crate::paths::Os;
use crate::safe_path::safe_relative;

/// Quick Play arrived with 1.20, released on this day.
const QUICK_PLAY_SINCE: &str = "2023-06-07";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub java: PathBuf,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    /// `${user_type}`; the original always passes `msa`.
    pub user_type: String,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    pub game_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub launcher_name: String,
    pub launcher_version: String,
    /// Already sanitised by `java::memory`.
    pub jvm_arguments: Vec<String>,
    pub resolution: Option<(u32, u32)>,
    pub demo: bool,
    pub server: Option<(String, u16)>,
    pub disable_multiplayer: bool,
    pub disable_chat: bool,
}

pub fn classpath_separator(os: Os) -> &'static str {
    if os == Os::Windows { ";" } else { ":" }
}

/// A `downloads.*` entry's own `path`, else the Maven path of `name`.
fn entry_path(entry: Option<&Value>, name: &str) -> Option<PathBuf> {
    match entry.and_then(|e| e.get("path")).and_then(Value::as_str) {
        Some(raw) => safe_relative(raw),
        None => library_path(name),
    }
}

/// The libraries that apply here — the same files installs download — without repeats, then the
/// client jar.
pub fn classpath(mc_dir: &Path, info: &VersionInfo, platform: &GamePlatform) -> Vec<PathBuf> {
    let libraries = mc_dir.join("libraries");
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for library in &info.libraries {
        if !rules_pass(library.get("rules"), platform, &Features::default()) {
            continue;
        }
        let Some(name) = library.get("name").and_then(Value::as_str) else { continue };
        let downloads = library.get("downloads");
        let main = match (downloads, downloads.and_then(|d| d.get("artifact"))) {
            (_, Some(artifact)) => entry_path(Some(artifact), name),
            (None, None) if library.get("natives").is_none() => library_path(name),
            _ => None,
        };
        let native = natives_classifier(library, platform).and_then(|classifier| {
            let entry = downloads.and_then(|d| d.get("classifiers")).and_then(|c| c.get(&classifier));
            entry_path(entry, &format!("{name}:{classifier}"))
        });
        for relative in [main, native].into_iter().flatten() {
            let path = libraries.join(relative);
            if seen.insert(path.clone()) {
                entries.push(path);
            }
        }
    }
    entries.push(mc_dir.join("versions").join(&info.jar_id).join(format!("{}.jar", info.jar_id)));
    entries
}

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z0-9_]+)\}").expect("a valid pattern"));

/// `${name}` placeholders from `values`; unknown ones stay as they are.
fn substitute(text: &str, values: &HashMap<&str, String>) -> String {
    PLACEHOLDER
        .replace_all(text, |caps: &regex::Captures| {
            values.get(&caps[1]).cloned().unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

/// Modern `arguments.jvm`/`arguments.game` items that pass their rules, substituted.
fn arguments(
    list: &[Value],
    platform: &GamePlatform,
    features: &Features,
    values: &HashMap<&str, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for item in list {
        match item {
            Value::String(text) => out.push(substitute(text, values)),
            Value::Object(rule) => {
                if !rules_pass(rule.get("compatibilityRules"), platform, features)
                    || !rules_pass(rule.get("rules"), platform, features)
                {
                    continue;
                }
                match rule.get("value") {
                    Some(Value::String(text)) => out.push(substitute(text, values)),
                    Some(Value::Array(texts)) => {
                        out.extend(texts.iter().filter_map(Value::as_str).map(|t| substitute(t, values)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// `x.y[.z]` as numbers; snapshots and pre-releases are `None`.
pub(crate) fn release_number(version: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if !(2..=3).contains(&parts.len())
        || !parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let number = |i: usize| parts.get(i).and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((number(0), number(1), number(2)))
}

/// Whether the game joins servers with `--quickPlayMultiplayer`: its arguments
/// name Quick Play, or its Minecraft version is 1.20+, or — for snapshots and pre-releases — its
/// release date is on or after 1.20's.
pub fn supports_quick_play(
    json: &Map<String, Value>,
    info: &VersionInfo,
    minecraft_version: Option<&str>,
) -> bool {
    let named = json
        .get("arguments")
        .and_then(|a| a.get("game"))
        .and_then(Value::as_array)
        .is_some_and(|args| args.iter().any(|a| a.to_string().contains("quickPlayMultiplayer")));
    if named {
        return true;
    }
    match minecraft_version.and_then(release_number) {
        Some(number) => number >= (1, 20, 0),
        None => info
            .release_time
            .as_deref()
            .and_then(|t| t.get(..10))
            .is_some_and(|date| date >= QUICK_PLAY_SINCE),
    }
}

/// Off for every log4j 2.10+ (a second line against Log4Shell, CVE-2021-44228).
pub const NO_LOOKUPS: &str = "-Dlog4j2.formatMsgNoLookups=true";

/// Log4Shell: lookups off, and Mojang's log config for every version that names one (for
/// 1.7–1.17 it is the patched one that closes the hole); the original never passed it.
fn log_arguments(mc_dir: &Path, info: &VersionInfo) -> Vec<String> {
    let mut arguments = vec![NO_LOOKUPS.to_string()];
    let id = info.log_config.as_ref().and_then(|f| f.id.as_deref());
    // The id is a plain file name: `VersionInfo` refuses any other when it reads the version.
    if let (Some(id), Some(argument)) = (id, &info.log_argument) {
        let file = mc_dir.join("assets").join("log_configs").join(id);
        if file.is_file() {
            arguments.push(argument.replace("${path}", &file.to_string_lossy()));
        }
    }
    arguments
}

/// `[java, jvm arguments…, main class, game arguments…]` for the merged version `json`.
pub fn build_command(
    mc_dir: &Path,
    json: &Map<String, Value>,
    minecraft_version: Option<&str>,
    opts: &LaunchOptions,
    platform: &GamePlatform,
) -> AppResult<Vec<String>> {
    let info = VersionInfo::from_json(json)?;
    let main_class = info.main_class.clone().ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput, format!("version {}: no mainClass", info.id))
            .with_param("version", &info.id)
    })?;
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let separator = classpath_separator(platform.os);
    let cp = classpath(mc_dir, &info, platform).iter().map(|p| text(p)).collect::<Vec<_>>().join(separator);
    let (width, height) = opts.resolution.unwrap_or((854, 480));
    let values: HashMap<&str, String> = HashMap::from([
        ("natives_directory", text(&opts.natives_dir)),
        ("launcher_name", opts.launcher_name.clone()),
        ("launcher_version", opts.launcher_version.clone()),
        ("classpath", cp.clone()),
        ("auth_player_name", opts.username.clone()),
        ("version_name", info.id.clone()),
        ("game_directory", text(&opts.game_dir)),
        ("assets_root", text(&mc_dir.join("assets"))),
        ("assets_index_name", info.asset_index_name.clone().unwrap_or_else(|| info.id.clone())),
        ("auth_uuid", opts.uuid.clone()),
        ("auth_access_token", opts.access_token.clone()),
        ("auth_session", opts.access_token.clone()),
        ("user_type", opts.user_type.clone()),
        ("version_type", info.kind.clone().unwrap_or_else(|| "release".into())),
        ("user_properties", "{}".into()),
        ("resolution_width", width.to_string()),
        ("resolution_height", height.to_string()),
        ("game_assets", text(&mc_dir.join("assets").join("virtual").join("legacy"))),
        ("library_directory", text(&mc_dir.join("libraries"))),
        ("classpath_separator", separator.into()),
        ("clientid", opts.client_id.clone().unwrap_or_default()),
        ("auth_xuid", opts.xuid.clone().unwrap_or_else(|| "0".into())),
    ]);
    let features =
        Features { custom_resolution: opts.resolution.is_some(), demo: opts.demo, ..Features::default() };
    let mut command = vec![text(&opts.java)];
    command.extend(opts.jvm_arguments.iter().cloned());
    match json.get("arguments").and_then(|a| a.get("jvm")).and_then(Value::as_array) {
        Some(jvm) => command.extend(arguments(jvm, platform, &features, &values)),
        None => {
            command.extend([format!("-Djava.library.path={}", text(&opts.natives_dir)), "-cp".into(), cp])
        }
    }
    command.extend(log_arguments(mc_dir, &info));
    command.push(main_class);
    match json.get("minecraftArguments").and_then(Value::as_str) {
        Some(legacy) => {
            command.extend(legacy.split(' ').map(|a| substitute(a, &values)));
            if let Some((w, h)) = opts.resolution {
                command.extend(["--width".into(), w.to_string(), "--height".into(), h.to_string()]);
            }
            if opts.demo {
                command.push("--demo".into());
            }
        }
        None => {
            let game = json.get("arguments").and_then(|a| a.get("game")).and_then(Value::as_array);
            command.extend(arguments(game.map_or(&[][..], Vec::as_slice), platform, &features, &values));
        }
    }
    let quick_play = opts.server.as_ref().filter(|_| supports_quick_play(json, &info, minecraft_version));
    if let Some((host, port)) = &opts.server
        && quick_play.is_none()
    {
        command.extend(["--server".into(), host.clone(), "--port".into(), port.to_string()]);
    }
    if opts.disable_multiplayer {
        command.push("--disableMultiplayer".into());
    }
    if opts.disable_chat {
        command.push("--disableChat".into());
    }
    if let Some((host, port)) = quick_play {
        command.extend(["--quickPlayMultiplayer".into(), format!("{host}:{port}")]);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::platform::GameArch;
    use serde_json::json;

    fn windows() -> GamePlatform {
        GamePlatform { os: Os::Windows, arch: GameArch::X64, os_version: "10.0".into() }
    }

    fn opts() -> LaunchOptions {
        LaunchOptions {
            java: PathBuf::from("java"),
            username: "Steve".into(),
            uuid: "uuid-1".into(),
            access_token: "token-1".into(),
            user_type: "msa".into(),
            xuid: Some("2535".into()),
            client_id: Some("client-9".into()),
            game_dir: PathBuf::from("G"),
            natives_dir: PathBuf::from("N"),
            launcher_name: "Launcher".into(),
            launcher_version: "0.1.0".into(),
            jvm_arguments: vec!["-Xmx4G".into()],
            ..LaunchOptions::default()
        }
    }

    fn modern() -> Value {
        json!({
            "id": "1.21.1", "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": "17",
            "releaseTime": "2024-08-08T12:24:45+00:00",
            "libraries": [
                {"name": "com.mojang:a:1", "downloads": {"artifact": {"path": "com/mojang/a/1/a-1.jar", "url": "https://x.example/a.jar"}}},
                {"name": "com.mojang:a:1", "downloads": {"artifact": {"path": "com/mojang/a/1/a-1.jar", "url": "https://x.example/a.jar"}}},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-macos", "downloads": {"artifact": {"path": "m.jar", "url": "https://x.example/m.jar"}},
                 "rules": [{"action": "allow", "os": {"name": "osx"}}]},
                {"name": "net.fabricmc:intermediary:1.21.1", "url": "https://maven.fabricmc.net/"}
            ],
            "arguments": {
                "game": ["--username", "${auth_player_name}", "--version", "${version_name}", "--gameDir", "${game_directory}",
                         "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}", "--accessToken", "${auth_access_token}",
                         "--clientId", "${clientid}", "--xuid", "${auth_xuid}", "--userType", "${user_type}", "--versionType", "${version_type}",
                         {"rules": [{"action": "allow", "features": {"has_custom_resolution": true}}],
                          "value": ["--width", "${resolution_width}", "--height", "${resolution_height}"]},
                         {"rules": [{"action": "allow", "features": {"is_quick_play_multiplayer": true}}],
                          "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"]}],
                "jvm": [{"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]},
                        {"rules": [{"action": "allow", "os": {"name": "windows"}}],
                         "value": "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"},
                        "-Djava.library.path=${natives_directory}", "-Dminecraft.launcher.brand=${launcher_name}", "-cp", "${classpath}"]
            }
        })
    }

    fn legacy() -> Value {
        json!({
            "id": "1.12.2", "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": "1.12",
            "releaseTime": "2017-09-18T08:39:46+00:00",
            "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userType ${user_type} --versionType ${version_type}",
            "libraries": [
                {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
                 "natives": {"windows": "natives-windows", "linux": "natives-linux", "osx": "natives-osx"},
                 "downloads": {"classifiers": {"natives-windows": {
                     "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar",
                     "url": "https://x.example/n.jar"}}}}
            ]
        })
    }

    fn text(path: PathBuf) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_modern_command_has_everything_in_order() {
        let mc = Path::new("MC");
        let cmd =
            build_command(mc, modern().as_object().unwrap(), Some("1.21.1"), &opts(), &windows()).unwrap();
        assert_eq!(
            cmd[..6],
            [
                "java",
                "-Xmx4G",
                "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump",
                "-Djava.library.path=N",
                "-Dminecraft.launcher.brand=Launcher",
                "-cp",
            ]
        );
        assert!(!cmd.contains(&"-XstartOnFirstThread".to_string()), "macOS-only");
        let libraries = mc.join("libraries");
        let expected_cp = [
            text(libraries.join("com").join("mojang").join("a").join("1").join("a-1.jar")),
            text(
                libraries
                    .join("net")
                    .join("fabricmc")
                    .join("intermediary")
                    .join("1.21.1")
                    .join("intermediary-1.21.1.jar"),
            ),
            text(mc.join("versions").join("1.21.1").join("1.21.1.jar")),
        ];
        assert_eq!(cmd[6], expected_cp.join(";"));
        assert_eq!(cmd[7], NO_LOOKUPS, "log4j lookups are off before the main class");
        assert_eq!(cmd[8], "net.minecraft.client.main.Main");
        assert_eq!(
            cmd[9..],
            [
                "--username",
                "Steve",
                "--version",
                "1.21.1",
                "--gameDir",
                "G",
                "--assetIndex",
                "17",
                "--uuid",
                "uuid-1",
                "--accessToken",
                "token-1",
                "--clientId",
                "client-9",
                "--xuid",
                "2535",
                "--userType",
                "msa",
                "--versionType",
                "release",
            ]
        );
    }

    #[test]
    fn resolution_and_quick_play_are_added_for_new_versions() {
        let options = LaunchOptions {
            resolution: Some((1280, 720)),
            server: Some(("mc.example.org".into(), 25565)),
            ..opts()
        };
        let cmd = build_command(
            Path::new("MC"),
            modern().as_object().unwrap(),
            Some("1.21.1"),
            &options,
            &windows(),
        )
        .unwrap();
        let joined = cmd.join(" ");
        assert!(joined.contains("--width 1280 --height 720"), "{joined}");
        assert!(joined.ends_with("--quickPlayMultiplayer mc.example.org:25565"), "{joined}");
        assert!(!joined.contains("--server") && !joined.contains("${quickPlayMultiplayer}"), "{joined}");
    }

    #[test]
    fn a_legacy_command_uses_minecraft_arguments() {
        let options = LaunchOptions {
            resolution: Some((1280, 720)),
            demo: true,
            server: Some(("mc.example.org".into(), 25566)),
            xuid: None,
            ..opts()
        };
        let mc = Path::new("MC");
        let cmd =
            build_command(mc, legacy().as_object().unwrap(), Some("1.12.2"), &options, &windows()).unwrap();
        assert_eq!(cmd[..4], ["java", "-Xmx4G", "-Djava.library.path=N", "-cp"]);
        let natives = mc
            .join("libraries")
            .join("org")
            .join("lwjgl")
            .join("lwjgl")
            .join("lwjgl-platform")
            .join("2.9.4-nightly-20150209")
            .join("lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar");
        assert_eq!(
            cmd[4],
            [text(natives), text(mc.join("versions").join("1.12.2").join("1.12.2.jar"))].join(";")
        );
        assert_eq!(cmd[5], NO_LOOKUPS, "log4j lookups are off before the main class");
        assert_eq!(cmd[6], "net.minecraft.client.main.Main");
        let assets = text(mc.join("assets"));
        assert_eq!(
            cmd[7..],
            [
                "--username",
                "Steve",
                "--version",
                "1.12.2",
                "--gameDir",
                "G",
                "--assetsDir",
                assets.as_str(),
                "--assetIndex",
                "1.12",
                "--uuid",
                "uuid-1",
                "--accessToken",
                "token-1",
                "--userType",
                "msa",
                "--versionType",
                "release",
                "--width",
                "1280",
                "--height",
                "720",
                "--demo",
                "--server",
                "mc.example.org",
                "--port",
                "25566",
            ]
        );
    }

    #[test]
    fn quick_play_follows_the_version() {
        let bare = |release: &str| json!({"id": "x", "mainClass": "M", "releaseTime": release});
        let info = |json: &Value| VersionInfo::from_json(json.as_object().unwrap()).unwrap();
        let old = bare("2024-10-01T00:00:00+00:00");
        // A Fabric profile for 1.16 carries its own, recent releaseTime: the Minecraft version decides.
        assert!(!supports_quick_play(old.as_object().unwrap(), &info(&old), Some("1.16.5")));
        assert!(supports_quick_play(old.as_object().unwrap(), &info(&old), Some("1.20.1")));
        assert!(supports_quick_play(old.as_object().unwrap(), &info(&old), Some("1.21")));
        let pre = bare("2023-05-16T11:00:00+00:00");
        assert!(!supports_quick_play(pre.as_object().unwrap(), &info(&pre), Some("1.20-pre1")));
        let snapshot = bare("2023-08-01T00:00:00+00:00");
        assert!(supports_quick_play(snapshot.as_object().unwrap(), &info(&snapshot), Some("23w31a")));
        assert!(!supports_quick_play(pre.as_object().unwrap(), &info(&pre), None));
        let named = modern();
        assert!(
            supports_quick_play(named.as_object().unwrap(), &info(&named), Some("1.16.5")),
            "the arguments say so"
        );
    }

    #[test]
    fn unknown_placeholders_stay_and_a_main_class_is_required() {
        let values = HashMap::from([("auth_player_name", "Steve".to_string())]);
        assert_eq!(substitute("${auth_player_name} ${nope}", &values), "Steve ${nope}");
        assert_eq!(classpath_separator(Os::Linux), ":");
        let bare = json!({"id": "x"});
        let err =
            build_command(Path::new("MC"), bare.as_object().unwrap(), None, &opts(), &windows()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    fn with_logging(mut version: Value) -> Value {
        version["logging"] = json!({"client": {
            "argument": "-Dlog4j.configurationFile=${path}",
            "file": {"id": "client-1.12.xml", "url": "https://x.example/l.xml", "sha1": "12", "size": 7},
            "type": "log4j2-xml"}});
        version
    }

    fn before_main(cmd: &[String]) -> &[String] {
        let main = cmd.iter().position(|a| a == "net.minecraft.client.main.Main").unwrap();
        &cmd[..main]
    }

    #[test]
    fn old_versions_get_mojangs_patched_log_config() {
        let mc = tempfile::tempdir().unwrap();
        let config = mc.path().join("assets").join("log_configs").join("client-1.12.xml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "<Configuration/>").unwrap();
        let version = with_logging(legacy());
        let cmd = build_command(mc.path(), version.as_object().unwrap(), Some("1.12.2"), &opts(), &windows())
            .unwrap();
        let wanted = format!("-Dlog4j.configurationFile={}", text(config));
        assert!(before_main(&cmd).contains(&wanted), "{cmd:?}");
    }

    #[test]
    fn lookups_are_off_for_every_version() {
        for (version, mc) in [(modern(), "1.21.1"), (legacy(), "1.12.2")] {
            let cmd =
                build_command(Path::new("MC"), version.as_object().unwrap(), Some(mc), &opts(), &windows())
                    .unwrap();
            assert!(before_main(&cmd).iter().any(|a| a == NO_LOOKUPS), "{mc}: {cmd:?}");
        }
    }

    #[test]
    fn a_missing_log_config_file_adds_no_argument() {
        let mc = tempfile::tempdir().unwrap();
        let version = with_logging(legacy());
        let cmd = build_command(mc.path(), version.as_object().unwrap(), Some("1.12.2"), &opts(), &windows())
            .unwrap();
        assert!(!cmd.iter().any(|a| a.starts_with("-Dlog4j.configurationFile")), "{cmd:?}");
    }

    #[test]
    fn a_log_config_id_cannot_leave_its_folder() {
        let mut version = with_logging(legacy());
        version["logging"]["client"]["file"]["id"] = json!("../../evil.xml");
        let refused =
            build_command(Path::new("MC"), version.as_object().unwrap(), Some("1.12.2"), &opts(), &windows());
        assert_eq!(refused.unwrap_err().code, ErrorCode::InvalidInput, "the version itself is refused");
    }
}
