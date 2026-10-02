//! What a build launches with: the player, folders, Java, memory and the
//! build's own options.

use std::path::{Path, PathBuf};

use launcher_shared::branding::{APP_NAME, MS_CLIENT_ID, VERSION};
use serde_json::{Map, Value};

use crate::auth::service::LaunchIdentity;
use crate::builds::service::GPU_MODE_DEFAULT_KEY;
use crate::java::gpu::GpuMode;
use crate::java::memory::{MemoryLimits, parse_memory_value, sanitize_jvm_arguments};
use crate::minecraft::command::LaunchOptions;
use crate::storage::config::ConfigStore;
use crate::storage::versions::Build;

pub const DEFAULT_MAX_RAM_KEY: &str = "default_max_ram_gb";
pub const DEFAULT_SERVER_PORT: u16 = 25565;
/// The build's own account (a profile key): its games start with it, and Play asks no account.
pub const PROFILE_OPTION: &str = "profileKey";

/// The account `options` give the build, when it has its own.
pub fn assigned_profile(options: &Map<String, Value>) -> Option<&str> {
    options.get(PROFILE_OPTION).and_then(Value::as_str).map(str::trim).filter(|key| !key.is_empty())
}
const DEFAULT_RESOLUTION: (u32, u32) = (854, 480);

/// The game folder: the build's `path` (a relative one under the Minecraft folder), else its id.
pub fn game_dir(build: &Build, mc_dir: &Path) -> PathBuf {
    let raw = build.path.as_deref().map(str::trim).filter(|p| !p.is_empty()).unwrap_or(&build.version_id);
    let path = Path::new(raw);
    if path.is_absolute() { path.to_path_buf() } else { mc_dir.join(path) }
}

/// The installed version the build runs: `loader`, else its Minecraft version.
pub fn component_id(build: &Build) -> Option<&str> {
    [build.loader.as_deref(), build.version.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|c| !c.is_empty())
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
        }
        Some(Value::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        _ => false,
    }
}

fn number<T: TryFrom<u64>>(value: Option<&Value>) -> Option<T> {
    let raw = match value? {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    T::try_from(raw).ok()
}

/// `server` as `{host, port}` or a plain host, else `serverHost`/`serverPort`; port 25565 by default.
pub fn server_address(options: &Map<String, Value>) -> Option<(String, u16)> {
    let (host, port) = match options.get("server") {
        Some(Value::Object(server)) => {
            (server.get("host").and_then(Value::as_str).map(str::to_string), number(server.get("port")))
        }
        Some(Value::String(host)) => (Some(host.clone()), None),
        _ => (None, None),
    };
    let host = host
        .or_else(|| options.get("serverHost").and_then(Value::as_str).map(str::to_string))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())?;
    let port = port.or_else(|| number(options.get("serverPort"))).unwrap_or(DEFAULT_SERVER_PORT);
    Some(match host_port(&host) {
        Some((host, typed)) => (host.to_string(), typed),
        None => (host, port),
    })
}

/// `host:port` typed into the host field (`[v6]:port` too); a bare IPv6 host has colons of its own.
fn host_port(host: &str) -> Option<(&str, u16)> {
    let (name, port) = host.rsplit_once(':')?;
    let bracketed = name.starts_with('[') && name.ends_with(']');
    if name.is_empty() || (name.contains(':') && !bracketed) {
        return None;
    }
    Some((name, port.parse().ok()?))
}

/// `resolutionWidth`×`resolutionHeight` when `customResolution` is on (854×480 by default).
pub fn resolution(options: &Map<String, Value>) -> Option<(u32, u32)> {
    truthy(options.get("customResolution")).then(|| {
        (
            number(options.get("resolutionWidth")).unwrap_or(DEFAULT_RESOLUTION.0),
            number(options.get("resolutionHeight")).unwrap_or(DEFAULT_RESOLUTION.1),
        )
    })
}

/// `jvmArguments` as saved: a list, or (hand-edited) one line.
pub(crate) fn jvm_arguments(options: &Map<String, Value>) -> Vec<String> {
    match options.get("jvmArguments") {
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(line)) => launcher_shared::args::words(line),
        _ => Vec::new(),
    }
}

pub struct OptionsInput<'a> {
    pub build: &'a Build,
    pub identity: &'a LaunchIdentity,
    pub mc_dir: &'a Path,
    /// The installed version id (`component_id`).
    pub component: &'a str,
    pub java: PathBuf,
    pub config: &'a ConfigStore,
    pub limits: MemoryLimits,
}

pub use launcher_shared::args::DEFAULT_GC_ARGUMENTS;

/// `arguments` (the memory limit first) with `DEFAULT_GC_ARGUMENTS` after the memory limit, unless
/// the build sets its collector or tunes it itself: then they stay as they are.
pub fn with_default_collector(arguments: Vec<String>) -> Vec<String> {
    let tunes_gc =
        |arg: &String| arg.trim().starts_with("-XX:") && (arg.contains("GC") || arg.contains("G1"));
    if arguments.iter().any(tunes_gc) {
        return arguments;
    }
    let at = arguments.iter().take_while(|a| a.trim().to_ascii_lowercase().starts_with("-xm")).count();
    let mut all = arguments;
    all.splice(at..at, DEFAULT_GC_ARGUMENTS.iter().map(|a| a.to_string()));
    all
}

/// The launch options and GPU mode (the build's, else `gpu_mode_default`).
pub fn launch_options(input: OptionsInput<'_>) -> (LaunchOptions, GpuMode) {
    let OptionsInput { build, identity, mc_dir, component, java, config, limits } = input;
    let options = &build.options;
    let fallback = config
        .get(DEFAULT_MAX_RAM_KEY)
        .as_ref()
        .and_then(parse_memory_value)
        .unwrap_or(limits.recommended_heap_gb);
    let memory = sanitize_jvm_arguments(&jvm_arguments(options), Some(fallback), &limits);
    if memory.changed {
        tracing::info!(
            "Normalized JVM memory arguments of {}: -Xmx{:?}G{}",
            build.name,
            memory.max_gb,
            if memory.removed_initial_heap { ", removed -Xms" } else { "" }
        );
    }
    let config_gpu = config.get_str(GPU_MODE_DEFAULT_KEY);
    let gpu = GpuMode::parse(options.get("gpuMode").and_then(Value::as_str).or(config_gpu.as_deref()));
    let opts = LaunchOptions {
        java,
        username: identity.username.clone(),
        uuid: identity.uuid.clone(),
        access_token: identity.access_token.clone(),
        user_type: "msa".into(),
        xuid: identity.xuid.clone(),
        client_id: Some(identity.client_id.clone().unwrap_or_else(|| MS_CLIENT_ID.to_string())),
        game_dir: game_dir(build, mc_dir),
        natives_dir: mc_dir.join("versions").join(component).join("natives"),
        launcher_name: APP_NAME.to_string(),
        launcher_version: VERSION.to_string(),
        jvm_arguments: with_default_collector(memory.arguments),
        resolution: resolution(options),
        demo: truthy(options.get("demo")),
        server: server_address(options),
        disable_multiplayer: truthy(options.get("disableMultiplayer")),
        disable_chat: truthy(options.get("disableChat")),
    };
    (opts, gpu)
}

/// The Java to run: the build's own `executablePath` when that file exists and is not one of the
/// launcher's runtimes (those are resolved by the runtime service, so a moved or isolated runtime
/// is still found), else `managed`, else `java` from `PATH`.
pub fn resolve_java(build: &Build, managed: Option<PathBuf>, mc_dir: &Path) -> PathBuf {
    let custom =
        build.options.get("executablePath").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty());
    if let Some(custom) = custom {
        let path = PathBuf::from(custom);
        if path.starts_with(mc_dir.join("runtime")) {
            tracing::debug!("{} uses the launcher's Java; resolving it again", build.name);
        } else if path.is_file() {
            return path;
        } else {
            tracing::warn!("Java {custom} of build {} is missing; using the launcher's Java", build.name);
        }
    }
    managed.unwrap_or_else(|| PathBuf::from("java"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::AccountKind;
    use serde_json::json;

    fn identity() -> LaunchIdentity {
        LaunchIdentity {
            username: "Steve".into(),
            uuid: "uuid-1".into(),
            access_token: "offline".into(),
            xuid: None,
            client_id: None,
            kind: AccountKind::Offline,
        }
    }

    fn build(options: Value) -> Build {
        let mut build = Build::new("Aero");
        build.version = Some("1.21.1".into());
        build.loader = Some("1.21.1".into());
        build.options = options.as_object().unwrap().clone();
        build
    }

    #[test]
    fn folders_and_components_come_from_the_build() {
        let mc = Path::new("MC");
        let mut b = build(json!({}));
        assert_eq!(game_dir(&b, mc), mc.join("aero"), "no path: the version id under the Minecraft folder");
        b.path = Some("games/aero".into());
        assert_eq!(game_dir(&b, mc), mc.join("games/aero"));
        let absolute = std::env::temp_dir().join("aero-game");
        b.path = Some(absolute.to_string_lossy().into_owned());
        assert_eq!(game_dir(&b, mc), absolute);
        assert_eq!(component_id(&b), Some("1.21.1"));
        b.loader = Some("  ".into());
        assert_eq!(component_id(&b), Some("1.21.1"), "falls back to the Minecraft version");
        b.version = None;
        assert_eq!(component_id(&b), None);
    }

    #[test]
    fn servers_and_resolutions_read_every_shape() {
        let read = |v: Value| server_address(v.as_object().unwrap());
        assert_eq!(
            read(json!({"server": {"host": "a.example", "port": 25570}})),
            Some(("a.example".into(), 25570))
        );
        assert_eq!(read(json!({"server": "b.example"})), Some(("b.example".into(), 25565)));
        assert_eq!(
            read(json!({"serverHost": "c.example", "serverPort": "25571"})),
            Some(("c.example".into(), 25571))
        );
        assert_eq!(read(json!({"server": {"host": "  "}})), None);
        // A port typed into the host field is the server's port, not a second one.
        assert_eq!(
            read(json!({"server": {"host": "play.x:25570", "port": 25565}})),
            Some(("play.x".into(), 25570))
        );
        assert_eq!(read(json!({"server": "[::1]:25570"})), Some(("[::1]".into(), 25570)));
        assert_eq!(read(json!({"server": "fe80::1"})), Some(("fe80::1".into(), 25565)), "a bare IPv6 host");
        assert_eq!(read(json!({"server": "play.x:abc"})), Some(("play.x:abc".into(), 25565)));
        assert_eq!(read(json!({"serverPort": 1})), None);
        let size = |v: Value| resolution(v.as_object().unwrap());
        assert_eq!(
            size(json!({"customResolution": true, "resolutionWidth": "1280", "resolutionHeight": 720})),
            Some((1280, 720))
        );
        assert_eq!(size(json!({"customResolution": "yes"})), Some((854, 480)));
        assert_eq!(size(json!({"customResolution": false, "resolutionWidth": 1280})), None);
    }

    #[test]
    fn a_build_without_a_collector_of_its_own_gets_the_official_launchers_g1() {
        let plain = vec!["-Xmx6G".to_string(), "-Dx=1".to_string()];
        let with_defaults = with_default_collector(plain);
        assert_eq!(&with_defaults[..1], ["-Xmx6G"], "the memory first");
        assert_eq!(&with_defaults[1..7], DEFAULT_GC_ARGUMENTS);
        assert_eq!(with_defaults.last().map(String::as_str), Some("-Dx=1"), "the build's own after");
        for own in ["-XX:+UseZGC", "-XX:+UseG1GC", "-XX:MaxGCPauseMillis=100", "-XX:G1HeapRegionSize=16M"] {
            let args = vec!["-Xmx6G".to_string(), own.to_string()];
            assert_eq!(with_default_collector(args.clone()), args, "{own}: the player's choice stays alone");
        }
    }

    #[test]
    fn options_combine_the_player_the_build_and_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path().join("config.json"));
        config.set(DEFAULT_MAX_RAM_KEY, json!("6")).unwrap();
        config.set(GPU_MODE_DEFAULT_KEY, json!("igpu")).unwrap();
        let b = build(
            json!({"jvmArguments": ["-XX:+UseG1GC", "-Xms1G"], "server": "mc.example.org", "disableChat": true}),
        );
        let limits = MemoryLimits::from_bytes(Some(16 * crate::java::memory::GIB), None);
        let mc = Path::new("MC");
        let input = OptionsInput {
            build: &b,
            identity: &identity(),
            mc_dir: mc,
            component: "1.21.1",
            java: PathBuf::from("J"),
            config: &config,
            limits,
        };
        let (opts, gpu) = launch_options(input);
        assert_eq!(opts.jvm_arguments, ["-Xmx6G", "-XX:+UseG1GC"]);
        assert_eq!(
            (opts.username.as_str(), opts.access_token.as_str(), opts.user_type.as_str()),
            ("Steve", "offline", "msa")
        );
        assert_eq!(opts.client_id.as_deref(), Some(MS_CLIENT_ID));
        assert_eq!(opts.game_dir, mc.join("aero"));
        assert_eq!(opts.natives_dir, mc.join("versions").join("1.21.1").join("natives"));
        assert_eq!((opts.launcher_name.as_str(), opts.launcher_version.as_str()), (APP_NAME, VERSION));
        assert_eq!(opts.server, Some(("mc.example.org".into(), 25565)));
        assert!(opts.disable_chat && !opts.disable_multiplayer && !opts.demo);
        assert_eq!(opts.java, PathBuf::from("J"));
        assert_eq!(gpu, GpuMode::Igpu, "the config default when the build has none");
        let own = build(json!({"gpuMode": "auto"}));
        let input = OptionsInput {
            build: &own,
            identity: &identity(),
            mc_dir: mc,
            component: "1.21.1",
            java: PathBuf::from("J"),
            config: &config,
            limits,
        };
        assert_eq!(launch_options(input).1, GpuMode::Auto);
    }

    #[test]
    fn java_comes_from_the_build_then_the_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path().join("mc");
        let custom = dir.path().join("jdk").join("bin").join("java.exe");
        std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
        std::fs::write(&custom, b"java").unwrap();
        let managed = Some(mc.join("runtime").join("java-runtime-delta").join("bin").join("java.exe"));
        let with = |path: &Path| build(json!({"executablePath": path.to_string_lossy()}));
        assert_eq!(resolve_java(&with(&custom), managed.clone(), &mc), custom, "the user's own Java");
        let gone = dir.path().join("gone").join("java.exe");
        assert_eq!(
            resolve_java(&with(&gone), managed.clone(), &mc),
            managed.clone().unwrap(),
            "a missing file falls back"
        );
        let old_managed = mc.join("runtime").join("jre-legacy").join("bin").join("java.exe");
        std::fs::create_dir_all(old_managed.parent().unwrap()).unwrap();
        std::fs::write(&old_managed, b"java").unwrap();
        assert_eq!(
            resolve_java(&with(&old_managed), managed.clone(), &mc),
            managed.clone().unwrap(),
            "managed paths follow the runtime service"
        );
        assert_eq!(resolve_java(&build(json!({})), None, &mc), PathBuf::from("java"));
    }
}
