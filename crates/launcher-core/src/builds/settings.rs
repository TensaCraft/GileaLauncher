//! The build settings page against the build record: memory and JVM arguments,
//! Java, GPU, the quick-join server and the icon, kept in the original's format.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use launcher_shared::{AppError, AppResult, BuildSettingsDto, BuildSettingsUpdate, ErrorCode};
use serde_json::{Map, Value, json};

use launcher_shared::args;

use crate::java::memory::parse_memory_gb;
use crate::launch::options::{PROFILE_OPTION, assigned_profile, jvm_arguments, server_address};
use crate::storage::versions::Build;

/// The largest icon a build takes.
pub const ICON_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// A record's icon as the page shows it: a URL as it is, raw base64 (the original's format) as a
/// `data:` URL of the type its bytes start with.
pub fn image_src(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    if raw.starts_with("data:") || raw.starts_with("https://") || raw.starts_with("http://") {
        return Some(raw.to_string());
    }
    let kind = if raw.starts_with("/9j/") {
        "jpeg"
    } else if raw.starts_with("R0lGOD") {
        "gif"
    } else if raw.starts_with("UklGR") {
        "webp"
    } else {
        "png"
    };
    Some(format!("data:image/{kind};base64,{raw}"))
}

/// A build's own memory limit (`-Xmx`, GB) and its other JVM arguments.
pub fn split_jvm_arguments(options: &Map<String, Value>) -> (Option<u64>, Vec<String>) {
    let mut max = None;
    let mut rest: Vec<String> = Vec::new();
    for argument in jvm_arguments(options) {
        let argument = argument.trim().to_string();
        if argument.is_empty() {
            continue;
        }
        let lower = argument.to_ascii_lowercase();
        if lower.starts_with("-xmx") {
            if let Some(gb) = parse_memory_gb(&argument) {
                max = Some(gb);
            }
        } else if !lower.starts_with("-xms") {
            rest.push(argument);
        }
    }
    (max, rest)
}

/// `-Xmx<n>G` first (when the build has its own limit), then the words of the lines the player
/// typed (quotes keep blanks) without other `-Xmx`/`-Xms`. Repeats stay: `--add-opens` comes once
/// per package.
pub fn compose_jvm_arguments(max_ram_gb: Option<u64>, custom: &[String]) -> Vec<String> {
    let mut arguments: Vec<String> = max_ram_gb.map(|gb| format!("-Xmx{gb}G")).into_iter().collect();
    for argument in custom.iter().flat_map(|line| args::words(line)) {
        let lower = argument.to_ascii_lowercase();
        if !lower.starts_with("-xmx") && !lower.starts_with("-xms") {
            arguments.push(argument);
        }
    }
    arguments
}

/// Empty (no port) or a port 1–65535; anything else is `invalid_input` (`invalid_port`).
pub fn parse_port(raw: &str) -> AppResult<Option<u16>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u16>().ok().filter(|port| *port > 0).map(Some).ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput, format!("invalid port {raw:?}"))
            .with_param("error", "invalid_port")
    })
}

/// `auto`, `igpu` or `dgpu`; anything else is the system's default.
fn gpu_mode(raw: &str) -> &'static str {
    crate::java::gpu::GpuMode::parse(Some(raw)).as_str()
}

/// The page's view of `build`. Its `executablePath` in `<mc_dir>/runtime` is the launcher's own
/// Java (new builds record it), so it shows as automatic; `auto_java` is that Java.
pub fn settings_of(build: &Build, mc_dir: &Path, auto_java: Option<PathBuf>) -> BuildSettingsDto {
    let options = &build.options;
    let (max_ram_gb, jvm_arguments) = split_jvm_arguments(options);
    let java_path = options
        .get("executablePath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty() && !Path::new(path).starts_with(mc_dir.join("runtime")))
        .map(str::to_string);
    let (server_host, server_port) = match server_address(options) {
        Some((host, port)) => (host, Some(port)),
        None => (String::new(), None),
    };
    BuildSettingsDto {
        key: build.key.clone(),
        name: build.name.clone(),
        image: image_src(build.image.as_deref()),
        component: build.loader.clone().filter(|c| !c.trim().is_empty()),
        java_path,
        auto_java: auto_java.map(|path| path.to_string_lossy().into_owned()),
        gpu_mode: gpu_mode(options.get("gpuMode").and_then(Value::as_str).unwrap_or_default()).to_string(),
        max_ram_gb,
        jvm_arguments,
        server_host,
        server_port,
        profile: assigned_profile(options).map(str::to_string),
    }
}

/// Writes the page's memory, arguments, Java, GPU and server into `build` in the original's format.
/// The name, the component and the icon are the service's (they need other builds and the disk).
pub fn apply_options(build: &mut Build, update: &BuildSettingsUpdate) -> AppResult<()> {
    let port = parse_port(&update.server_port)?;
    let options = &mut build.options;
    let arguments = compose_jvm_arguments(update.max_ram_gb, &update.jvm_arguments);
    if arguments.is_empty() {
        options.remove("jvmArguments");
    } else {
        options.insert("jvmArguments".into(), json!(arguments));
    }
    match update.java_path.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => options.insert("executablePath".into(), json!(path)),
        None => options.remove("executablePath"),
    };
    options.insert("gpuMode".into(), json!(gpu_mode(&update.gpu_mode)));
    options.remove("serverHost");
    options.remove("serverPort");
    let host = update.server_host.trim();
    if host.is_empty() {
        options.remove("server");
    } else {
        let mut server = json!({"host": host});
        if let Some(port) = port {
            server["port"] = json!(port);
        }
        options.insert("server".into(), server);
    }
    match update.profile.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => options.insert(PROFILE_OPTION.into(), json!(key)),
        None => options.remove(PROFILE_OPTION),
    };
    Ok(())
}

/// A picked icon as the record keeps it (raw base64, like the original): PNG, JPEG, GIF or WebP of
/// up to 2 MiB.
pub fn read_icon(path: &Path) -> AppResult<String> {
    let bad = |why: &str| {
        AppError::new(ErrorCode::InvalidInput, format!("{}: {why}", path.display())).with_param("error", why)
    };
    let size = fs::metadata(path)
        .map(|m| if m.is_file() { m.len() } else { u64::MAX })
        .map_err(|_| bad("no such file"))?;
    if size > ICON_MAX_BYTES {
        return Err(bad("not an image of up to 2 MB"));
    }
    let bytes = fs::read(path).map_err(|e| bad(&e.to_string()))?;
    let image = bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF8")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()));
    if !image {
        return Err(bad("not a PNG, JPEG, GIF or WebP image"));
    }
    Ok(STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn build(options: Value) -> Build {
        let mut build = Build::new("Aero");
        build.version = Some("1.21.1".into());
        build.loader = Some("1.21.1".into());
        build.options = options.as_object().unwrap().clone();
        build
    }

    fn update() -> BuildSettingsUpdate {
        BuildSettingsUpdate { name: "Aero".into(), gpu_mode: "dgpu".into(), ..BuildSettingsUpdate::default() }
    }

    #[test]
    fn the_build_s_own_account_round_trips() {
        let mut b = build(json!({"profileKey": "Alex"}));
        assert_eq!(settings_of(&b, Path::new("mc"), None).profile.as_deref(), Some("Alex"));
        apply_options(&mut b, &BuildSettingsUpdate { profile: Some(" Steve ".into()), ..update() }).unwrap();
        assert_eq!(b.options["profileKey"], json!("Steve"));
        apply_options(&mut b, &update()).unwrap();
        assert!(!b.options.contains_key("profileKey"), "no account of its own: Play asks as usual");
        assert_eq!(settings_of(&b, Path::new("mc"), None).profile, None);
    }

    #[test]
    fn icons_become_urls_the_page_can_show() {
        assert_eq!(image_src(None), None);
        assert_eq!(image_src(Some("  ")), None);
        assert_eq!(
            image_src(Some("iVBORw0KGgoAAA")).as_deref(),
            Some("data:image/png;base64,iVBORw0KGgoAAA")
        );
        assert_eq!(image_src(Some("/9j/4AAQ")).as_deref(), Some("data:image/jpeg;base64,/9j/4AAQ"));
        assert_eq!(image_src(Some("R0lGODlh")).as_deref(), Some("data:image/gif;base64,R0lGODlh"));
        assert_eq!(image_src(Some("UklGRiQA")).as_deref(), Some("data:image/webp;base64,UklGRiQA"));
        for url in ["https://cdn.example/pack.png", "data:image/png;base64,AAAA"] {
            assert_eq!(image_src(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn memory_and_arguments_round_trip() {
        let options = json!({"jvmArguments": ["-Xms1G", "-Xmx6G", "-XX:+UseG1GC", " "]});
        assert_eq!(
            split_jvm_arguments(options.as_object().unwrap()),
            (Some(6), vec!["-XX:+UseG1GC".to_string()])
        );
        let line = json!({"jvmArguments": "-Xmx2G -Dx=1"});
        assert_eq!(split_jvm_arguments(line.as_object().unwrap()), (Some(2), vec!["-Dx=1".to_string()]));
        let custom = ["-XX:+UseG1GC".to_string(), "-Xmx9G".into(), " ".into(), "-Dx=1".into()];
        assert_eq!(compose_jvm_arguments(Some(4), &custom), ["-Xmx4G", "-XX:+UseG1GC", "-Dx=1"]);
        assert_eq!(
            compose_jvm_arguments(None, &custom),
            ["-XX:+UseG1GC", "-Dx=1"],
            "no -Xmx: the default applies"
        );
        assert!(compose_jvm_arguments(None, &[]).is_empty());
    }

    #[test]
    fn a_pasted_line_of_arguments_is_split_into_words() {
        let custom =
            ["-Xmx4G -XX:+UseG1GC".to_string(), "--add-opens java.base/java.lang=ALL-UNNAMED".into()];
        assert_eq!(
            compose_jvm_arguments(Some(6), &custom),
            ["-Xmx6G", "-XX:+UseG1GC", "--add-opens", "java.base/java.lang=ALL-UNNAMED"],
            "the memory field keeps the limit; nothing else on the line is lost"
        );
    }

    #[test]
    fn quoted_arguments_keep_their_spaces() {
        let custom = [r#"-Dname="a b" -Dx=1"#.to_string()];
        assert_eq!(compose_jvm_arguments(None, &custom), ["-Dname=a b", "-Dx=1"]);
        let line = json!({"jvmArguments": r#"-Xmx2G "-Dpath=C:\My Games" -Dx=1"#});
        assert_eq!(
            split_jvm_arguments(line.as_object().unwrap()),
            (Some(2), vec![r"-Dpath=C:\My Games".to_string(), "-Dx=1".to_string()])
        );
    }

    #[test]
    fn repeated_add_opens_are_kept() {
        let custom = ["--add-opens a/b=ALL-UNNAMED".to_string(), "--add-opens c/d=ALL-UNNAMED".into()];
        let composed = compose_jvm_arguments(None, &custom);
        assert_eq!(composed, ["--add-opens", "a/b=ALL-UNNAMED", "--add-opens", "c/d=ALL-UNNAMED"]);
        let stored = json!({"jvmArguments": composed});
        assert_eq!(split_jvm_arguments(stored.as_object().unwrap()).1.len(), 4);
    }

    #[test]
    fn the_page_reads_the_record() {
        let mc = Path::new("MC");
        let mut b = build(json!({
            "jvmArguments": ["-Xmx4G", "-Dfoo=1"],
            "executablePath": "C:/jdk/bin/java.exe",
            "gpuMode": "igpu",
            "server": {"host": "play.example", "port": 25570}
        }));
        let settings = settings_of(&b, mc, Some(PathBuf::from("MC/runtime/java-runtime-delta/bin/java.exe")));
        assert_eq!(
            (
                settings.max_ram_gb,
                settings.jvm_arguments.as_slice(),
                settings.java_path.as_deref(),
                settings.gpu_mode.as_str()
            ),
            (Some(4), ["-Dfoo=1".to_string()].as_slice(), Some("C:/jdk/bin/java.exe"), "igpu")
        );
        assert_eq!((settings.server_host.as_str(), settings.server_port), ("play.example", Some(25570)));
        assert!(settings.auto_java.is_some());
        b.options.insert(
            "executablePath".into(),
            json!(
                mc.join("runtime").join("java-runtime-delta").join("bin").join("java.exe").to_string_lossy()
            ),
        );
        b.options.insert("serverHost".into(), json!("old.example"));
        b.options.remove("server");
        let settings = settings_of(&b, mc, None);
        assert_eq!(settings.java_path, None, "the launcher's own Java counts as automatic");
        assert_eq!((settings.server_host.as_str(), settings.server_port), ("old.example", Some(25565)));
    }

    #[test]
    fn the_page_writes_the_record() {
        let mut b = build(json!({"serverHost": "old", "serverPort": 1, "gpuMode": "dgpu"}));
        let mut u = update();
        u.max_ram_gb = Some(3);
        u.jvm_arguments = vec!["-XX:+UseG1GC".into()];
        u.java_path = Some("C:/jdk/bin/java.exe".into());
        u.gpu_mode = "auto".into();
        u.server_host = " play.example ".into();
        u.server_port = "25570".into();
        apply_options(&mut b, &u).unwrap();
        assert_eq!(b.options["jvmArguments"], json!(["-Xmx3G", "-XX:+UseG1GC"]));
        assert_eq!(b.options["executablePath"], json!("C:/jdk/bin/java.exe"));
        assert_eq!(b.options["gpuMode"], json!("auto"));
        assert_eq!(b.options["server"], json!({"host": "play.example", "port": 25570}));
        assert!(!b.options.contains_key("serverHost") && !b.options.contains_key("serverPort"));
        let mut cleared = update();
        cleared.gpu_mode = "rtx".into();
        apply_options(&mut b, &cleared).unwrap();
        for key in ["jvmArguments", "executablePath", "server"] {
            assert!(!b.options.contains_key(key), "{key}");
        }
        let system = crate::java::gpu::platform_default(crate::paths::Os::current()).as_str();
        assert_eq!(b.options["gpuMode"], json!(system), "an unknown mode is the system's default");
        let mut no_port = update();
        no_port.server_host = "play.example".into();
        apply_options(&mut b, &no_port).unwrap();
        assert_eq!(b.options["server"], json!({"host": "play.example"}));
        for bad in ["abc", "0", "70000", "-1"] {
            let mut u = update();
            u.server_host = "play.example".into();
            u.server_port = bad.into();
            assert_eq!(apply_options(&mut b, &u).unwrap_err().code, ErrorCode::InvalidInput, "{bad}");
        }
    }

    #[test]
    fn icons_must_be_small_images() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("icon.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(read_icon(&png).unwrap(), "iVBORw0KGgpyZXN0");
        let text = dir.path().join("icon.txt");
        fs::write(&text, b"hello").unwrap();
        assert_eq!(read_icon(&text).unwrap_err().code, ErrorCode::InvalidInput);
        let big = dir.path().join("big.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(ICON_MAX_BYTES as usize + 1, 0);
        fs::write(&big, bytes).unwrap();
        assert_eq!(read_icon(&big).unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(read_icon(&dir.path().join("missing.png")).unwrap_err().code, ErrorCode::InvalidInput);
    }
}
