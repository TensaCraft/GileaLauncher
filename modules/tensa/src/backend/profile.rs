//! The settings a server build takes from the server: what it runs, and its
//! server address, graphics card and Java arguments — the server's defaults at install, then only
//! the fields the server forces at every sync.

use std::collections::HashSet;

use launcher_core::storage::versions::Build;
use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};
use serde_json::{Value, json};

use super::pack::{Pack, truthy};

/// The names a server may give the fields it forces, and the field each means.
const FIELD_ALIASES: [(&str, &str); 24] = [
    ("minecraft", "minecraft_version"),
    ("minecraftversion", "minecraft_version"),
    ("minecraft_version", "minecraft_version"),
    ("version", "minecraft_version"),
    ("loader", "loader_id"),
    ("loaderid", "loader_id"),
    ("loader_id", "loader_id"),
    ("loaderversion", "loader_version"),
    ("loader_version", "loader_version"),
    ("server", "server"),
    ("serverhost", "server_host"),
    ("server_host", "server_host"),
    ("serverport", "server_port"),
    ("server_port", "server_port"),
    ("gpu", "gpu_preference"),
    ("gpumode", "gpu_preference"),
    ("gpu_mode", "gpu_preference"),
    ("gpupreference", "gpu_preference"),
    ("gpu_preference", "gpu_preference"),
    ("image", "image"),
    ("icon", "image"),
    ("jvm", "jvm_arguments"),
    ("jvmarguments", "jvm_arguments"),
    ("jvm_arguments", "jvm_arguments"),
];

fn invalid(pack: &Pack, what: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("the server build {} {what}", pack.id))
        .with_param("pack", &pack.id)
}

/// The kind of loader the build runs now: from its component id, else its client.
fn loader_key(build: &Build) -> Option<String> {
    let loader = build.loader.as_deref().unwrap_or_default().to_lowercase();
    let key = if loader.starts_with("fabric-loader-") {
        "fabric"
    } else if loader.starts_with("quilt-loader-") {
        "quilt"
    } else if loader.starts_with("neoforge-") {
        "neoforge"
    } else if loader.contains("-forge-") || loader.starts_with("forge-") {
        "forge"
    } else {
        let client = build.client.as_deref().unwrap_or_default().to_lowercase();
        if ["minecraft", "fabric", "forge", "neoforge", "quilt"].contains(&client.as_str()) {
            return Some(client);
        }
        return (!loader.is_empty()).then_some(loader);
    };
    Some(key.to_string())
}

/// The fields `pack` forces at every sync.
fn forced_fields(pack: &Pack) -> HashSet<&'static str> {
    pack.forced_fields
        .iter()
        .filter_map(|raw| {
            let key = raw.trim().to_lowercase().replace(['-', '.'], "_");
            FIELD_ALIASES.iter().find(|(alias, _)| *alias == key).map(|(_, field)| *field)
        })
        .collect()
}

/// A port from the server, else `fallback` when it is none or out of range.
fn port_of(value: Option<&Value>, fallback: u16) -> u16 {
    let number = match value {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    number.filter(|p| (1..=65535).contains(p)).map_or(fallback, |p| p as u16)
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// The server's Java arguments: set, or cleared by an empty list; nothing when it names none.
fn apply_jvm(build: &mut Build, pack: &Pack) {
    match &pack.jvm_arguments {
        Some(arguments) if !arguments.is_empty() => {
            build.options.insert("jvmArguments".into(), json!(arguments));
        }
        Some(_) => {
            build.options.remove("jvmArguments");
        }
        None => {}
    }
}

/// The server address the server forces: its host and port, or the build's where it forces
/// only the other; no host is no server.
fn apply_forced_server(build: &mut Build, pack: &Pack, fields: &HashSet<&str>) {
    let client = pack.raw.get("client").cloned().unwrap_or(Value::Null);
    let existing = build.options.get("server").and_then(Value::as_object).cloned().unwrap_or_default();
    let all = fields.contains("server");
    let host = if all || fields.contains("server_host") {
        text(client.get("server_host"))
    } else {
        text(existing.get("host"))
    };
    if host.is_empty() {
        build.options.remove("server");
        return;
    }
    let fallback = port_of(existing.get("port"), 25565);
    let port_value =
        if all || fields.contains("server_port") { client.get("server_port") } else { existing.get("port") };
    build.options.insert("server".into(), json!({"host": host, "port": port_of(port_value, fallback)}));
}

fn apply_forced(build: &mut Build, pack: &Pack) {
    let fields = forced_fields(pack);
    if fields.is_empty() {
        return;
    }
    if fields.contains("minecraft_version") {
        build.version = pack.minecraft.clone().or(build.version.take());
    }
    if fields.contains("loader_version") {
        build.loader_version = pack.loader_version.clone().or(build.loader_version.take());
    }
    if fields.contains("image") {
        build.image = pack.image.clone();
    }
    if fields.contains("gpu_preference")
        && let Some(mode) = pack.gpu.as_deref().and_then(gpu_mode)
    {
        build.options.insert("gpuMode".into(), json!(mode));
    }
    if fields.contains("jvm_arguments") {
        apply_jvm(build, pack);
    }
    if fields.contains("server") || fields.contains("server_host") || fields.contains("server_port") {
        apply_forced_server(build, pack, &fields);
    }
}

/// The mod loader the server build runs.
pub fn loader_kind(pack: &Pack) -> AppResult<LoaderKind> {
    pack.loader
        .as_deref()
        .and_then(LoaderKind::from_client)
        .ok_or_else(|| invalid(pack, "names no mod loader the launcher runs"))
}

/// The build no longer runs what the server names: another Minecraft, loader or loader version.
pub fn loader_changed(build: &Build, pack: &Pack) -> bool {
    build.version != pack.minecraft
        || build.loader_version != pack.loader_version
        || loader_key(build) != pack.loader
}

/// A graphics card preference as the build stores it: `discrete` is `dgpu`, `integrated` `igpu`.
pub fn gpu_mode(raw: &str) -> Option<String> {
    let text = raw.trim().to_lowercase();
    let mode = match text.as_str() {
        "" => return None,
        "discrete" => "dgpu",
        "integrated" => "igpu",
        other => other,
    };
    Some(mode.to_string())
}

/// A new build's settings from the server build: what it runs and the server's defaults.
pub fn apply_install(build: &mut Build, pack: &Pack) -> AppResult<LoaderKind> {
    let minecraft = pack.minecraft.clone().ok_or_else(|| invalid(pack, "names no Minecraft version"))?;
    let kind = loader_kind(pack)?;
    build.version = Some(minecraft);
    build.loader_version = pack.loader_version.clone();
    build.force_update = pack.force_update;
    build.image = pack.image.clone();
    merge(build, pack, true);
    Ok(kind)
}

/// The server's settings merged into `build`: all its defaults when `api_defaults` (an install),
/// else only the fields it forces.
pub fn merge(build: &mut Build, pack: &Pack, api_defaults: bool) {
    if api_defaults || build.image.is_none() {
        build.image = pack.image.clone().or(build.image.take());
    }
    build.force_update = pack.force_update;
    if !api_defaults {
        apply_forced(build, pack);
        return;
    }
    if let Some((host, port)) = &pack.server
        && !build.options.contains_key("server")
    {
        build.options.insert("server".into(), json!({"host": host, "port": port}));
    }
    if let Some(mode) = pack.gpu.as_deref().and_then(gpu_mode) {
        build.options.insert("gpuMode".into(), json!(mode));
    }
    let mut options = pack.options.clone();
    if let Some(server) = options.remove("server")
        && truthy(&server)
        && !build.options.contains_key("server")
    {
        build.options.insert("server".into(), server);
    }
    build.options.extend(options);
    apply_jvm(build, pack);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pack(client: Value) -> Pack {
        Pack::from_value(&json!({"client": client})).unwrap()
    }

    fn server_build() -> Pack {
        pack(json!({
            "id": "aero", "minecraft_version": "1.21.1", "loader_id": "fabric", "loader_version": "0.16.9",
            "image": "logo.png", "server_host": "play.example", "server_port": 25570, "gpu_preference": "discrete",
            "options": {"server": {"host": "other"}, "resolution": "1920x1080"},
            "jvm_arguments": ["-XX:+UseG1GC"], "force_update_endpoint": "u"
        }))
    }

    #[test]
    fn install_settings_take_the_server_defaults_but_keep_a_players_server() {
        let mut fresh = Build::new("Aero");
        assert_eq!(apply_install(&mut fresh, &server_build()).unwrap(), LoaderKind::Fabric);
        assert_eq!(
            (fresh.version.as_deref(), fresh.loader_version.as_deref()),
            (Some("1.21.1"), Some("0.16.9"))
        );
        assert!(fresh.force_update);
        assert_eq!(fresh.image.as_deref(), Some("logo.png"));
        assert_eq!(fresh.options.get("server"), Some(&json!({"host": "play.example", "port": 25570})));
        assert_eq!(fresh.options.get("gpuMode"), Some(&json!("dgpu")));
        assert_eq!(fresh.options.get("resolution"), Some(&json!("1920x1080")));
        assert_eq!(fresh.options.get("jvmArguments"), Some(&json!(["-XX:+UseG1GC"])));

        let mut own = Build::new("Aero");
        own.options.insert("server".into(), json!({"host": "mine", "port": 25565}));
        apply_install(&mut own, &server_build()).unwrap();
        assert_eq!(
            own.options.get("server"),
            Some(&json!({"host": "mine", "port": 25565})),
            "a player's server stays"
        );
    }

    #[test]
    fn an_install_needs_minecraft_and_a_known_loader() {
        let mut b = Build::new("Aero");
        let no_minecraft = pack(json!({"id": "a", "loader_id": "fabric"}));
        assert_eq!(apply_install(&mut b, &no_minecraft).unwrap_err().code, ErrorCode::InvalidInput);
        let no_loader = pack(json!({"id": "a", "minecraft_version": "1.21.1"}));
        assert_eq!(apply_install(&mut b, &no_loader).unwrap_err().code, ErrorCode::InvalidInput);
        let odd = pack(json!({"id": "a", "minecraft_version": "1.21.1", "loader": "liteloader"}));
        assert_eq!(loader_kind(&odd).unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(
            loader_kind(&pack(json!({"id": "a", "loader": "NeoForge"}))).unwrap(),
            LoaderKind::NeoForge
        );
    }

    #[test]
    fn a_sync_changes_only_the_forced_settings() {
        let mut b = Build::new("Aero");
        b.version = Some("1.20.1".into());
        b.image = Some("mine.png".into());
        b.options.insert("server".into(), json!({"host": "mine", "port": 25565}));
        b.options.insert("gpuMode".into(), json!("igpu"));
        b.options.insert("jvmArguments".into(), json!(["-Xss4m"]));
        let mut forcing = server_build();
        forcing.forced_fields = vec!["JVM".into(), "icon".into()];
        merge(&mut b, &forcing, false);
        assert_eq!(b.options.get("jvmArguments"), Some(&json!(["-XX:+UseG1GC"])));
        assert_eq!(b.image.as_deref(), Some("logo.png"));
        assert_eq!(b.options.get("server"), Some(&json!({"host": "mine", "port": 25565})));
        assert_eq!(b.options.get("gpuMode"), Some(&json!("igpu")));
        assert_eq!(b.version.as_deref(), Some("1.20.1"), "not forced");
        assert!(b.force_update);
        assert!(!b.options.contains_key("resolution"), "the server's other defaults are for installs");

        let mut quiet = Build::new("Aero");
        quiet.image = Some("mine.png".into());
        merge(&mut quiet, &server_build(), false);
        assert_eq!(quiet.image.as_deref(), Some("mine.png"), "an image of its own stays unless forced");
        let mut bare = Build::new("Aero");
        merge(&mut bare, &server_build(), false);
        assert_eq!(bare.image.as_deref(), Some("logo.png"), "a build without an image takes the server's");

        let mut versions = Build::new("Aero");
        let mut forced = server_build();
        forced.forced_fields = vec!["minecraft-version".into(), "loader.version".into(), "gpu".into()];
        merge(&mut versions, &forced, false);
        assert_eq!(
            (versions.version.as_deref(), versions.loader_version.as_deref()),
            (Some("1.21.1"), Some("0.16.9"))
        );
        assert_eq!(versions.options.get("gpuMode"), Some(&json!("dgpu")));
    }

    #[test]
    fn a_forced_server_with_no_host_is_removed_and_a_bad_port_falls_back() {
        let mut b = Build::new("Aero");
        b.options.insert("server".into(), json!({"host": "mine", "port": 25570}));
        let mut port_only = pack(json!({"id": "a", "server_port": "x"}));
        port_only.forced_fields = vec!["server_port".into()];
        merge(&mut b, &port_only, false);
        assert_eq!(
            b.options.get("server"),
            Some(&json!({"host": "mine", "port": 25570})),
            "the old port is the fallback"
        );
        let mut hosted = pack(json!({"id": "a", "server_host": "play.example", "server_port": 70000}));
        hosted.forced_fields = vec!["serverhost".into()];
        merge(&mut b, &hosted, false);
        assert_eq!(b.options.get("server"), Some(&json!({"host": "play.example", "port": 25570})));
        let mut gone = pack(json!({"id": "a"}));
        gone.forced_fields = vec!["server".into()];
        merge(&mut b, &gone, false);
        assert!(!b.options.contains_key("server"), "no host, no server");
    }

    #[test]
    fn jvm_arguments_are_set_or_cleared() {
        let mut b = Build::new("Aero");
        b.options.insert("jvmArguments".into(), json!(["-Xss4m"]));
        let mut cleared = pack(json!({"id": "a", "jvm_arguments": []}));
        cleared.forced_fields = vec!["jvm_arguments".into()];
        merge(&mut b, &cleared, false);
        assert!(!b.options.contains_key("jvmArguments"));
        b.options.insert("jvmArguments".into(), json!(["-Xss4m"]));
        let mut silent = pack(json!({"id": "a"}));
        silent.forced_fields = vec!["jvm_arguments".into()];
        merge(&mut b, &silent, false);
        assert_eq!(
            b.options.get("jvmArguments"),
            Some(&json!(["-Xss4m"])),
            "a server that says nothing changes nothing"
        );
    }

    #[test]
    fn a_changed_loader_or_version_is_seen() {
        let fabric = server_build();
        let mut b = Build::new("Aero");
        b.version = Some("1.21.1".into());
        b.loader_version = Some("0.16.9".into());
        b.loader = Some("fabric-loader-0.16.9-1.21.1".into());
        assert!(!loader_changed(&b, &fabric));
        b.loader_version = Some("0.16.10".into());
        assert!(loader_changed(&b, &fabric));
        b.loader_version = Some("0.16.9".into());
        b.version = Some("1.21".into());
        assert!(loader_changed(&b, &fabric));
        b.version = Some("1.21.1".into());
        b.loader = Some("neoforge-21.1.77".into());
        assert!(loader_changed(&b, &fabric));
        let named = |loader: &str, client: &str, kind: &str| {
            let mut b = Build::new("x");
            b.version = Some("1.21.1".into());
            b.loader_version = Some("1".into());
            b.loader = Some(loader.into());
            b.client = Some(client.into());
            let p = pack(
                json!({"id": "a", "minecraft_version": "1.21.1", "loader": kind, "loader_version": "1"}),
            );
            !loader_changed(&b, &p)
        };
        assert!(named("quilt-loader-1-1.21.1", "", "quilt"));
        assert!(named("1.21.1-forge-1", "", "forge"));
        assert!(named("forge-1", "", "forge"));
        assert!(named("1.21.1", "Fabric", "fabric"), "the client names it when the component does not");
    }

    #[test]
    fn gpu_preferences_map_like_the_original() {
        for (raw, mode) in [
            ("discrete", "dgpu"),
            (" Integrated ", "igpu"),
            ("auto", "auto"),
            ("dgpu", "dgpu"),
            ("Other", "other"),
        ] {
            assert_eq!(gpu_mode(raw).as_deref(), Some(mode), "{raw}");
        }
        assert_eq!(gpu_mode("  "), None);
    }
}
