//! Command implementations.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::profile::{BuildSpec, KNOWN_MODULES};
use crate::templates;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace").to_path_buf()
}

pub fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

pub fn run(cmd: &mut Command) -> Result<()> {
    let line = shown(cmd);
    println!("> {line}");
    let status = cmd.status().with_context(|| format!("failed to start {line}"))?;
    if !status.success() {
        bail!("command failed ({status}): {line}");
    }
    Ok(())
}

/// A command as it is shown: the program and its arguments, never its environment (on Unix the
/// debug form lists the variables set for it, the CurseForge key among them).
fn shown(cmd: &Command) -> String {
    let mut line = format!("{:?}", cmd.get_program());
    for arg in cmd.get_args() {
        line.push_str(&format!(" {arg:?}"));
    }
    line
}

pub fn trunk(spec: &BuildSpec, serve: bool, release: bool) -> Command {
    let mut cmd = Command::new("trunk");
    cmd.current_dir(root().join("crates").join("launcher-ui")).arg(if serve { "serve" } else { "build" });
    if release {
        cmd.arg("--release");
    }
    cmd.arg("--no-default-features");
    if !spec.modules.is_empty() {
        cmd.args(["--features", &spec.features_arg()]);
    }
    clean_update_env(&mut cmd);
    // The interface never sees the CurseForge key (only the module's backend reads it).
    cmd.env_remove(API_KEY_VAR);
    cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
    cmd
}

/// The variable that carries the CurseForge API key into a build (`modules/curseforge`).
pub const API_KEY_VAR: &str = "CURSEFORGE_API_KEY";
/// The developer's own copy of the key, in `.dev/` (ignored by git).
pub const DEV_API_KEY_FILE: &str = "curseforge-api-key";

/// The CurseForge key a developer keeps in `.dev/` for `cargo xtask dev` and `build`.
pub fn dev_api_key(root: &Path) -> Option<String> {
    let key = std::fs::read_to_string(root.join(".dev").join(DEV_API_KEY_FILE)).ok()?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// A `LAUNCHER_UPDATE_API`/`LAUNCHER_VERSION` left in the shell must never reach a build: only the profile
/// (and `build_versioned_app`) may set them.
pub fn clean_update_env(cmd: &mut Command) {
    cmd.env_remove("LAUNCHER_UPDATE_API").env_remove("LAUNCHER_VERSION");
}

fn cargo_app(spec: &BuildSpec, subcommand: &str, release: bool) -> Command {
    let mut cmd = Command::new(cargo());
    cmd.current_dir(root()).args([subcommand, "-p", "launcher-app", "--no-default-features"]);
    if release {
        cmd.arg("--release");
    }
    // Builds must embed the frontend (Tauri `custom-protocol`); `run` keeps loading the dev server.
    let mut features = if subcommand == "build" { vec!["custom-protocol".to_string()] } else { Vec::new() };
    features.extend(spec.app_features());
    if !features.is_empty() {
        cmd.args(["--features", &features.join(",")]);
    }
    clean_update_env(&mut cmd);
    // A developer's own key, unless the shell already sets one.
    if std::env::var_os(API_KEY_VAR).is_none()
        && let Some(key) = dev_api_key(&root())
    {
        cmd.env(API_KEY_VAR, key);
    }
    cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
    cmd
}

/// Release-like app build (cargo profile `mock`) with `LAUNCHER_VERSION`, returning the executable.
pub fn build_versioned_app(spec: &BuildSpec, version: &str) -> Result<PathBuf> {
    let mut frontend = trunk(spec, false, false);
    frontend.env("LAUNCHER_VERSION", version);
    run(&mut frontend)?;
    let mut features = vec!["custom-protocol".to_string()];
    features.extend(spec.app_features());
    let mut app = Command::new(cargo());
    app.current_dir(root())
        .args(["build", "-p", "launcher-app", "--no-default-features", "--profile", "mock", "--features"])
        .arg(features.join(","));
    clean_update_env(&mut app);
    app.envs(spec.env.iter().map(|(k, v)| (k, v))).env("LAUNCHER_VERSION", version);
    run(&mut app)?;
    let exe = root().join("target").join("mock").join(if cfg!(windows) {
        "launcher-app.exe"
    } else {
        "launcher-app"
    });
    if !exe.is_file() {
        bail!("the build finished but {} is missing", exe.display());
    }
    Ok(exe)
}

fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("trunk did not start on port {port} within {timeout:?}")
}

pub fn dev(spec: &BuildSpec) -> Result<()> {
    let mut server = trunk(spec, true, false).spawn().context("failed to start trunk serve")?;
    let result =
        wait_for_port(1420, Duration::from_secs(300)).and_then(|_| run(&mut cargo_app(spec, "run", false)));
    let _ = server.kill();
    result
}

/// A release build must not trust a server on this machine (a profile whose updates come from
/// the local mock).
pub fn refuse_unsafe_release(spec: &BuildSpec, release: bool) -> Result<()> {
    if release && spec.local_updates() {
        bail!(
            "profile '{}' takes its updates from this machine ({}); it is for testing and cannot be built with --release",
            spec.name,
            spec.update_api()
        );
    }
    Ok(())
}

pub fn build(spec: &BuildSpec, release: bool) -> Result<()> {
    refuse_unsafe_release(spec, release)?;
    run(&mut trunk(spec, false, release))?;
    run(&mut cargo_app(spec, "build", release))
}

/// The frontend the app embeds, built once for clippy and the tests.
pub fn frontend_for_tests() -> Result<()> {
    let spec = crate::profile::load_profile(&root(), "standard")?;
    run(&mut trunk(&spec, false, false))
}

/// Modules the app does not build by default: `check` and `test` give each its own clippy and
/// tests with all its features, and clippy the app and the UI with them.
pub const EXTRA_MODULES: &[&str] = &["tensa", "diagnostics"];

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// `crate/mod-<id>` for each of `EXTRA_MODULES`, comma-separated.
fn extra_features(krate: &str) -> String {
    EXTRA_MODULES.iter().map(|m| format!("{krate}/mod-{m}")).collect::<Vec<_>>().join(",")
}

/// Each of `EXTRA_MODULES`' own tests, with all its features.
fn module_tests() -> Vec<Vec<String>> {
    EXTRA_MODULES.iter().map(|m| words(&format!("test -p module-{m} --features backend,ui"))).collect()
}

/// What `check` runs after building the frontend, in order (cargo's arguments).
/// What `check` runs: the lint, then the tests.
pub fn check_steps() -> Vec<Vec<String>> {
    let mut steps = lint_steps();
    steps.extend(test_steps());
    steps
}

/// fmt and clippy: the app and the UI with every module, the UI for wasm too, and each module that
/// is built by no profile (`lint`; CI runs it once, the tests on every system).
pub fn lint_steps() -> Vec<Vec<String>> {
    let mut steps = vec![
        words("fmt --all -- --check"),
        words("clippy --workspace --all-targets -- -D warnings"),
        words(&format!(
            "clippy -p launcher-app --features {} --all-targets -- -D warnings",
            extra_features("launcher-app")
        )),
        words(&format!(
            "clippy -p launcher-ui -p ui-kit --target wasm32-unknown-unknown --features {} -- -D warnings",
            extra_features("launcher-ui")
        )),
    ];
    steps.extend(
        EXTRA_MODULES.iter().map(|m| {
            words(&format!("clippy -p module-{m} --features backend,ui --all-targets -- -D warnings"))
        }),
    );
    steps
}

/// What `test` runs after building the frontend: the workspace, then the modules it leaves out.
pub fn test_steps() -> Vec<Vec<String>> {
    let mut steps = vec![words("test --workspace")];
    steps.extend(module_tests());
    steps
}

fn cargo_steps(steps: Vec<Vec<String>>) -> Result<()> {
    let r = root();
    for step in steps {
        run(Command::new(cargo()).current_dir(&r).args(step))?;
    }
    Ok(())
}

pub fn test() -> Result<()> {
    frontend_for_tests()?;
    cargo_steps(test_steps())
}

pub fn check() -> Result<()> {
    frontend_for_tests()?;
    cargo_steps(check_steps())
}

pub fn lint() -> Result<()> {
    frontend_for_tests()?;
    cargo_steps(lint_steps())
}

/// The app's icons for every system from `path` (`crates/launcher-app/icons`: ico, png, icns);
/// `macos`, when given, makes icon.icns. The phone icons tauri also writes are not the app's.
pub fn icons(path: &Path, macos: Option<&Path>) -> Result<()> {
    let icons = root().join("crates/launcher-app/icons");
    let tauri_icon = |source: &Path, out: &Path| {
        run(Command::new(cargo())
            .current_dir(root())
            .args(["tauri", "icon"])
            .arg(source)
            .arg("--output")
            .arg(out))
    };
    tauri_icon(path, &icons)?;
    if let Some(macos) = macos {
        let scratch = root().join("target/icons-macos");
        tauri_icon(macos, &scratch)?;
        std::fs::copy(scratch.join("icon.icns"), icons.join("icon.icns"))
            .context("copying the macOS icon")?;
        let _ = std::fs::remove_dir_all(&scratch);
    }
    for phone in ["android", "ios"] {
        let _ = std::fs::remove_dir_all(icons.join(phone));
    }
    Ok(())
}

pub fn profiles() -> Result<()> {
    for entry in std::fs::read_dir(root().join("build-profiles"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let spec = crate::profile::load_profile(&root(), &name)?;
            println!("{name}: [{}]", spec.modules.join(", "));
        }
    }
    println!("known modules: {}", KNOWN_MODULES.join(", "));
    Ok(())
}

pub fn new_module(id: &str) -> Result<()> {
    templates::validate_id(id)?;
    let dir = root().join("modules").join(id);
    if dir.exists() {
        bail!("{} already exists", dir.display());
    }
    for (rel, body) in templates::render(id) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("template paths have parents"))?;
        std::fs::write(&path, body)?;
    }
    println!("Created {}", dir.display());
    println!("Register it:");
    println!(
        "  1. Cargo.toml: members += \"modules/{id}\"; [workspace.dependencies] module-{id} = {{ path = \"modules/{id}\" }}"
    );
    println!(
        "  2. crates/launcher-app/Cargo.toml + crates/launcher-ui/Cargo.toml: feature mod-{id} and optional dependency (backend/ui)"
    );
    println!(
        "  3. crates/launcher-app/src/modules.rs + crates/launcher-ui/src/modules.rs: push the module under #[cfg(feature = \"mod-{id}\")]"
    );
    println!(
        "  4. xtask/src/profile.rs: add \"{id}\" to KNOWN_MODULES; add it to the build profiles that need it"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_shown_command_carries_no_environment() {
        // On Unix a command's debug form lists the variables set for it: the API key among them.
        let mut cmd = std::process::Command::new("cargo");
        cmd.args(["build", "-p", "launcher-app"]).env("CURSEFORGE_API_KEY", "secret-key");
        let line = super::shown(&cmd);
        assert!(!line.contains("secret-key"), "{line}");
        assert!(line.contains("cargo") && line.contains("launcher-app"), "{line}");
    }

    use super::*;

    fn spec(modules: &[&str]) -> BuildSpec {
        BuildSpec {
            name: "t".into(),
            modules: modules.iter().map(|m| m.to_string()).collect(),
            env: Vec::new(),
        }
    }

    fn features_of(cmd: &Command) -> Option<String> {
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        args.iter().position(|a| a == "--features").map(|i| args[i + 1].clone())
    }

    /// The modules the app turns on by default (`mod-<id>` in its default features).
    fn default_modules() -> Vec<String> {
        let manifest: toml::Table =
            toml::from_str(&std::fs::read_to_string(root().join("crates/launcher-app/Cargo.toml")).unwrap())
                .unwrap();
        manifest["features"]["default"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f.as_str()?.strip_prefix("mod-").map(str::to_string))
            .collect()
    }

    #[test]
    fn check_runs_every_module_s_tests_and_clippy() {
        let defaults = default_modules();
        for module in crate::profile::discover_modules(&root()).unwrap() {
            assert!(
                defaults.contains(&module) || EXTRA_MODULES.contains(&module.as_str()),
                "module {module} is built by no check"
            );
        }
        let steps: Vec<String> = check_steps().iter().map(|s| s.join(" ")).collect();
        for module in EXTRA_MODULES {
            let crate_name = format!("module-{module}");
            for step in [
                format!("clippy -p {crate_name} --features backend,ui --all-targets -- -D warnings"),
                format!("test -p {crate_name} --features backend,ui"),
            ] {
                assert!(
                    steps.contains(&step),
                    "missing: cargo {step}
{steps:#?}"
                );
            }
            let feature = format!("launcher-app/mod-{module}");
            assert!(steps.iter().any(|s| s.starts_with("clippy -p launcher-app") && s.contains(&feature)));
            let ui = format!("launcher-ui/mod-{module}");
            assert!(steps.iter().any(|s| s.contains("wasm32-unknown-unknown") && s.contains(&ui)));
        }
        assert!(EXTRA_MODULES.contains(&"tensa") && EXTRA_MODULES.contains(&"diagnostics"));
    }

    #[test]
    fn check_is_lint_then_the_tests() {
        // CI runs the lint once (Linux) and the tests on every system: together they are `check`.
        let lint = lint_steps();
        assert!(lint.iter().all(|s| s[0] != "test"), "{lint:?}");
        assert!(lint.iter().any(|s| s.join(" ").starts_with("fmt --all")));
        let mut both = lint;
        both.extend(test_steps());
        assert_eq!(both, check_steps());
    }

    #[test]
    fn release_builds_have_no_devtools() {
        let manifest: toml::Table =
            toml::from_str(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap()).unwrap();
        let tauri = &manifest["workspace"]["dependencies"]["tauri"];
        let features: Vec<&str> = tauri
            .get("features")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
            .collect();
        assert!(!features.contains(&"devtools"), "the inspector must stay out of builds for players");
    }

    #[test]
    fn the_interface_plays_no_media_itself() {
        // WebKitGTK plays media through GStreamer, which the AppImage does not carry: the first
        // sound would hang the window. Sounds go through the launcher (`play_click`).
        let banned =
            ["HtmlAudioElement", "HtmlMediaElement", "HtmlVideoElement", "AudioContext", "<audio", "<video"];
        let mut found = Vec::new();
        let mut dirs = vec![root().join("crates/launcher-ui/src"), root().join("crates/ui-kit/src")];
        dirs.extend(
            std::fs::read_dir(root().join("modules")).unwrap().flatten().map(|m| m.path().join("src")),
        );
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for word in banned.iter().filter(|w| text.contains(**w)) {
                        found.push(format!("{}: {word}", path.display()));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "the interface plays media itself:
{}",
            found.join(
                "
"
            )
        );
        let manifest = std::fs::read_to_string(root().join("crates/ui-kit/Cargo.toml")).unwrap();
        assert!(!manifest.contains("HtmlAudioElement"), "ui-kit asks web-sys for audio elements");
    }

    #[test]
    fn the_developer_s_curseforge_key_reaches_only_the_backend() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dev_api_key(tmp.path()), None, "no key file");
        std::fs::create_dir_all(tmp.path().join(".dev")).unwrap();
        std::fs::write(tmp.path().join(".dev").join(DEV_API_KEY_FILE), "  key-from-file \r\n").unwrap();
        assert_eq!(dev_api_key(tmp.path()).as_deref(), Some("key-from-file"));
        std::fs::write(tmp.path().join(".dev").join(DEV_API_KEY_FILE), "\n").unwrap();
        assert_eq!(dev_api_key(tmp.path()), None, "a blank file is no key");
        let ui = trunk(&spec(&["curseforge"]), false, false);
        assert!(
            ui.get_envs().any(|(k, v)| k == API_KEY_VAR && v.is_none()),
            "the interface never builds with the key"
        );
    }

    #[test]
    fn curseforge_comes_with_modrinth() {
        // Built everywhere Modrinth is; a build without the CurseForge key simply shows no CurseForge.
        assert!(default_modules().contains(&"curseforge".to_string()));
        for name in ["standard", "full", "mock-updates"] {
            let spec = crate::profile::load_profile(&root(), name).unwrap();
            assert!(spec.modules.contains(&"curseforge".to_string()), "{name}");
        }
        assert!(!EXTRA_MODULES.contains(&"curseforge"), "checked with the default modules");
    }

    #[test]
    fn app_builds_embed_the_frontend() {
        let build = cargo_app(&spec(&["backups"]), "build", true);
        assert_eq!(features_of(&build).as_deref(), Some("custom-protocol,mod-backups"));
        let core = cargo_app(&spec(&[]), "build", false);
        assert_eq!(features_of(&core).as_deref(), Some("custom-protocol"));
    }

    fn with_local_updates(mut spec: BuildSpec) -> BuildSpec {
        spec.env.push(("LAUNCHER_UPDATE_API".into(), "http://127.0.0.1:1430".into()));
        spec
    }

    #[test]
    fn release_builds_refuse_loopback_update_profiles() {
        let mock = with_local_updates(spec(&[]));
        assert!(refuse_unsafe_release(&mock, true).is_err());
        assert!(refuse_unsafe_release(&mock, false).is_ok(), "a test build of it is fine");
        assert!(refuse_unsafe_release(&spec(&["backups"]), true).is_ok());
    }

    #[test]
    fn only_loopback_profiles_trust_the_local_update_server() {
        let mock = with_local_updates(spec(&["backups"]));
        assert_eq!(
            features_of(&cargo_app(&mock, "build", false)).as_deref(),
            Some("custom-protocol,mod-backups,mock-updates")
        );
        assert_eq!(features_of(&cargo_app(&mock, "run", false)).as_deref(), Some("mod-backups,mock-updates"));
        assert_eq!(
            features_of(&trunk(&mock, false, false)).as_deref(),
            Some("mod-backups"),
            "the UI has no such feature"
        );
        assert_eq!(
            features_of(&cargo_app(&spec(&["backups"]), "build", true)).as_deref(),
            Some("custom-protocol,mod-backups")
        );
    }

    #[test]
    fn the_app_embeds_its_ui_unless_told_otherwise() {
        let manifest: toml::Table =
            toml::from_str(&std::fs::read_to_string(root().join("crates/launcher-app/Cargo.toml")).unwrap())
                .unwrap();
        let default: Vec<&str> =
            manifest["features"]["default"].as_array().unwrap().iter().filter_map(|f| f.as_str()).collect();
        assert!(default.contains(&"custom-protocol"), "a plain cargo build must not load the dev server");
        assert!(!default.contains(&"mock-updates"));
    }

    #[test]
    fn dev_run_keeps_the_dev_server() {
        let run = cargo_app(&spec(&["backups"]), "run", false);
        assert_eq!(features_of(&run).as_deref(), Some("mod-backups"));
    }

    fn env_of(cmd: &Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs().find(|(k, _)| *k == key).map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn builds_never_inherit_update_overrides_from_the_shell() {
        for cmd in [cargo_app(&spec(&[]), "build", true), trunk(&spec(&[]), false, true)] {
            assert_eq!(env_of(&cmd, "LAUNCHER_UPDATE_API"), Some(None));
            assert_eq!(env_of(&cmd, "LAUNCHER_VERSION"), Some(None));
        }
        let mut mock = spec(&[]);
        mock.env.push(("LAUNCHER_UPDATE_API".into(), "http://127.0.0.1:1430".into()));
        let api = env_of(&cargo_app(&mock, "build", false), "LAUNCHER_UPDATE_API");
        assert_eq!(api, Some(Some("http://127.0.0.1:1430".into())));
    }
}
