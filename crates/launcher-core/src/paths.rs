//! Directory layout per OS. Pure functions over `PathEnv` for testability.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use launcher_shared::branding::APP_NAME;
use serde_json::json;

use crate::storage::json::{JsonRead, read_json_object, write_json_file};

const POINTER_FILE: &str = "storage.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
    #[default]
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Other => "other",
        }
    }
}

/// Inputs of the layout computation, captured once from the real process environment.
#[derive(Debug, Clone, Default)]
pub struct PathEnv {
    pub os: Os,
    pub home: PathBuf,
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_cache_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    /// `LAUNCHER_APP_BASE` (absolute only).
    pub app_base_override: Option<PathBuf>,
    /// Repository root in debug builds; enables `.dev` mode.
    pub dev_root: Option<PathBuf>,
    /// Directory of the running executable (protected-dir checks).
    pub exe_dir: Option<PathBuf>,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl PathEnv {
    pub fn from_system() -> Self {
        let dev_root = if cfg!(debug_assertions) && std::env::var("LAUNCHER_DEV").as_deref() != Ok("0") {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
            root.join("Cargo.toml").is_file().then(|| std::path::absolute(&root).unwrap_or(root))
        } else {
            None
        };
        Self {
            os: Os::current(),
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            local_app_data: env_path("LOCALAPPDATA"),
            app_data: env_path("APPDATA"),
            xdg_config_home: env_path("XDG_CONFIG_HOME"),
            xdg_data_home: env_path("XDG_DATA_HOME"),
            xdg_cache_home: env_path("XDG_CACHE_HOME"),
            xdg_state_home: env_path("XDG_STATE_HOME"),
            app_base_override: env_path("LAUNCHER_APP_BASE").filter(|p| p.is_absolute()),
            dev_root,
            exe_dir: std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
        }
    }
}

fn xdg(value: &Option<PathBuf>, home: &Path, fallback: &[&str]) -> PathBuf {
    match value {
        Some(p) if p.is_absolute() => p.clone(),
        _ => fallback.iter().fold(home.to_path_buf(), |acc, part| acc.join(part)),
    }
}

fn windows_local(env: &PathEnv) -> PathBuf {
    env.local_app_data
        .clone()
        .or_else(|| env.app_data.clone())
        .unwrap_or_else(|| env.home.join("AppData").join("Local"))
}

pub fn default_app_state_dir(env: &PathEnv) -> PathBuf {
    match env.os {
        Os::Windows => windows_local(env).join(APP_NAME),
        Os::Linux => xdg(&env.xdg_config_home, &env.home, &[".config"]).join(APP_NAME),
        Os::MacOs => env.home.join("Library").join("Application Support").join(APP_NAME),
        Os::Other => env.home.join(format!(".{APP_NAME}")),
    }
}

/// Default Minecraft root for the given state dir. A relocated state dir keeps Minecraft inside it.
pub fn default_minecraft_dir(env: &PathEnv, app_state: &Path) -> PathBuf {
    if env.dev_root.is_some() || app_state != default_app_state_dir(env) {
        return app_state.join("minecraft");
    }
    match env.os {
        Os::Windows => {
            env.app_data.clone().unwrap_or_else(|| env.home.join("AppData").join("Roaming")).join(APP_NAME)
        }
        Os::Linux => xdg(&env.xdg_data_home, &env.home, &[".local", "share"]).join(APP_NAME),
        Os::MacOs | Os::Other => app_state.join("minecraft"),
    }
}

pub fn default_cache_dir(env: &PathEnv) -> PathBuf {
    match env.os {
        Os::Windows => windows_local(env).join(APP_NAME).join("cache"),
        Os::Linux => xdg(&env.xdg_cache_home, &env.home, &[".cache"]).join(APP_NAME),
        Os::MacOs => env.home.join("Library").join("Caches").join(APP_NAME),
        Os::Other => env.home.join(format!(".{APP_NAME}")).join("cache"),
    }
}

pub fn default_log_dir(env: &PathEnv) -> PathBuf {
    match env.os {
        Os::Windows => windows_local(env).join(APP_NAME),
        Os::Linux => xdg(&env.xdg_state_home, &env.home, &[".local", "state"]).join(APP_NAME),
        Os::MacOs => env.home.join("Library").join("Logs").join(APP_NAME),
        Os::Other => env.home.join(format!(".{APP_NAME}")).join("logs"),
    }
}

fn pointer_path(env: &PathEnv) -> PathBuf {
    default_app_state_dir(env).join(POINTER_FILE)
}

pub fn read_pointer(env: &PathEnv) -> Option<PathBuf> {
    match read_json_object(&pointer_path(env)) {
        Ok(JsonRead::Object(map)) => {
            map.get("app_state_dir").and_then(|v| v.as_str()).map(PathBuf::from).filter(|p| p.is_absolute())
        }
        _ => None,
    }
}

/// Persists (or clears, for `None`/default) the relocated state dir. Never touches disk in dev mode.
pub fn write_pointer(env: &PathEnv, dir: Option<&Path>) -> io::Result<()> {
    if env.dev_root.is_some() {
        return Ok(());
    }
    let path = pointer_path(env);
    match dir {
        Some(d) if d != default_app_state_dir(env) => {
            write_json_file(&path, &json!({ "app_state_dir": d.to_string_lossy() }), 2)
        }
        _ => match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// True when `path` lies inside an installed (read-only) program directory.
pub fn is_protected_dir(env: &PathEnv, path: &Path) -> bool {
    let Some(exe_dir) = &env.exe_dir else { return false };
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let exe_lower = lower(exe_dir);
    let inside_exe_dir = lower(path).starts_with(&exe_lower);
    let installed = exe_dir.components().any(|c| match c {
        Component::Normal(s) => {
            let s = s.to_string_lossy().to_lowercase();
            s == "windowsapps" || s == "program files" || s == "program files (x86)" || s.ends_with(".app")
        }
        _ => false,
    });
    inside_exe_dir && installed
}

/// Creates the directory and verifies a real file can be created, flushed and removed in it.
pub fn probe_writable(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".launcher-write-test-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&probe)?;
        file.write_all(&[0])?;
        file.sync_all()
    })();
    let _ = fs::remove_file(&probe);
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherPaths {
    pub app_state_dir: PathBuf,
    pub minecraft_dir: PathBuf,
    pub games_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub dev_mode: bool,
    pub minecraft_dir_is_default: bool,
    /// The configured `minecraft_game_dir` was unusable and was ignored.
    pub rejected_minecraft_override: bool,
}

impl LauncherPaths {
    pub fn resolve(env: &PathEnv, minecraft_override: Option<&str>) -> LauncherPaths {
        let dev_mode = env.dev_root.is_some();
        let (app_state_dir, cache_dir, log_dir) = if let Some(root) = &env.dev_root {
            let base = root.join(".dev");
            (base.clone(), base.join("cache"), base)
        } else {
            // `LAUNCHER_APP_BASE` is a portable mode: everything, including the cache, lives there.
            let portable = env.app_base_override.clone().filter(|p| !is_protected_dir(env, p));
            let base = portable
                .clone()
                .or_else(|| read_pointer(env).filter(|p| !is_protected_dir(env, p)))
                .unwrap_or_else(|| default_app_state_dir(env));
            let cache = if portable.is_some() { base.join("cache") } else { default_cache_dir(env) };
            (base.clone(), cache, base)
        };
        let default_mc = default_minecraft_dir(env, &app_state_dir);

        let mut rejected = false;
        let minecraft_dir = match minecraft_override.map(str::trim).filter(|v| !v.is_empty()) {
            Some(raw) => {
                let candidate = PathBuf::from(raw);
                let candidate =
                    if candidate.is_absolute() { candidate } else { app_state_dir.join(candidate) };
                if is_protected_dir(env, &candidate) {
                    rejected = true;
                    default_mc.clone()
                } else {
                    candidate
                }
            }
            None => default_mc.clone(),
        };

        LauncherPaths {
            games_dir: minecraft_dir.join("games"),
            minecraft_dir_is_default: minecraft_dir == default_mc,
            minecraft_dir,
            app_state_dir,
            cache_dir,
            log_dir,
            dev_mode,
            rejected_minecraft_override: rejected,
        }
    }

    pub fn ensure_dirs(&self) -> io::Result<()> {
        for dir in [&self.app_state_dir, &self.minecraft_dir, &self.games_dir, &self.cache_dir] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: Os, home: &Path) -> PathEnv {
        PathEnv { os, home: home.to_path_buf(), ..PathEnv::default() }
    }

    #[test]
    fn windows_layout_uses_local_and_roaming_appdata() {
        let home = tempfile::tempdir().unwrap();
        let mut e = env(Os::Windows, home.path());
        e.local_app_data = Some(home.path().join("Local"));
        e.app_data = Some(home.path().join("Roaming"));
        let p = LauncherPaths::resolve(&e, None);
        assert_eq!(p.app_state_dir, home.path().join("Local").join(APP_NAME));
        assert_eq!(p.minecraft_dir, home.path().join("Roaming").join(APP_NAME));
        assert_eq!(p.games_dir, p.minecraft_dir.join("games"));
        assert_eq!(p.cache_dir, home.path().join("Local").join(APP_NAME).join("cache"));
        assert!(p.minecraft_dir_is_default && !p.dev_mode);
    }

    #[test]
    fn linux_layout_honours_only_absolute_xdg_vars() {
        let home = tempfile::tempdir().unwrap();
        let mut e = env(Os::Linux, home.path());
        e.xdg_config_home = Some(PathBuf::from("relative/config"));
        e.xdg_data_home = Some(home.path().join("data"));
        let p = LauncherPaths::resolve(&e, None);
        assert_eq!(p.app_state_dir, home.path().join(".config").join(APP_NAME));
        assert_eq!(p.minecraft_dir, home.path().join("data").join(APP_NAME));
        assert_eq!(p.cache_dir, home.path().join(".cache").join(APP_NAME));
    }

    #[test]
    fn macos_layout_keeps_minecraft_under_state() {
        let home = tempfile::tempdir().unwrap();
        let p = LauncherPaths::resolve(&env(Os::MacOs, home.path()), None);
        let state = home.path().join("Library").join("Application Support").join(APP_NAME);
        assert_eq!(p.app_state_dir, state);
        assert_eq!(p.minecraft_dir, state.join("minecraft"));
    }

    #[test]
    fn pointer_relocates_state_and_minecraft() {
        let home = tempfile::tempdir().unwrap();
        let e = env(Os::Linux, home.path());
        let custom = home.path().join("games").join(APP_NAME);
        write_pointer(&e, Some(&custom)).unwrap();
        assert_eq!(read_pointer(&e), Some(custom.clone()));
        let p = LauncherPaths::resolve(&e, None);
        assert_eq!(p.app_state_dir, custom);
        assert_eq!(p.minecraft_dir, custom.join("minecraft"));
        write_pointer(&e, None).unwrap();
        assert_eq!(read_pointer(&e), None);
    }

    #[test]
    fn pointer_ignores_invalid_or_relative_values() {
        let home = tempfile::tempdir().unwrap();
        let e = env(Os::Linux, home.path());
        let ptr = default_app_state_dir(&e).join("storage.json");
        std::fs::create_dir_all(ptr.parent().unwrap()).unwrap();
        for bad in ["", "null", "[]", "{\"app_state_dir\": 5}", "{\"app_state_dir\": \"relative\"}"] {
            std::fs::write(&ptr, bad).unwrap();
            assert_eq!(read_pointer(&e), None, "{bad}");
        }
    }

    #[test]
    fn minecraft_override_relative_and_absolute() {
        let home = tempfile::tempdir().unwrap();
        let e = env(Os::Linux, home.path());
        let rel = LauncherPaths::resolve(&e, Some("mc-data"));
        assert_eq!(rel.minecraft_dir, rel.app_state_dir.join("mc-data"));
        assert!(!rel.minecraft_dir_is_default);
        let abs_dir = home.path().join("elsewhere");
        let abs = LauncherPaths::resolve(&e, Some(abs_dir.to_str().unwrap()));
        assert_eq!(abs.minecraft_dir, abs_dir);
    }

    #[test]
    fn protected_minecraft_override_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let mut e = env(Os::Windows, home.path());
        let exe_dir = home.path().join("Program Files").join(APP_NAME);
        e.exe_dir = Some(exe_dir.clone());
        let p = LauncherPaths::resolve(&e, Some(exe_dir.join("data").to_str().unwrap()));
        assert!(p.rejected_minecraft_override && p.minecraft_dir_is_default);
    }

    #[test]
    fn dev_mode_uses_repo_dot_dev_and_ignores_pointer() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let mut e = env(Os::Windows, home.path());
        e.dev_root = Some(repo.path().to_path_buf());
        write_pointer(&e, Some(&home.path().join("x"))).unwrap();
        let p = LauncherPaths::resolve(&e, None);
        assert!(p.dev_mode);
        assert_eq!(p.app_state_dir, repo.path().join(".dev"));
        assert_eq!(p.minecraft_dir, repo.path().join(".dev").join("minecraft"));
        assert!(
            !default_app_state_dir(&PathEnv { dev_root: None, ..e.clone() }).join("storage.json").exists()
        );
    }

    #[test]
    fn resolves_unicode_home() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("Іван Петренко");
        let mut e = env(Os::Windows, &home);
        e.local_app_data = Some(home.join("AppData").join("Local"));
        let p = LauncherPaths::resolve(&e, None);
        p.ensure_dirs().unwrap();
        probe_writable(&p.app_state_dir).unwrap();
        assert!(p.app_state_dir.to_string_lossy().contains("Іван Петренко"));
    }

    #[test]
    fn probe_writable_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        probe_writable(dir.path()).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn app_base_override_keeps_the_cache_inside() {
        let home = tempfile::tempdir().unwrap();
        let base = home.path().join("portable");
        let mut e = env(Os::Windows, home.path());
        e.local_app_data = Some(home.path().join("Local"));
        e.app_base_override = Some(base.clone());
        let p = LauncherPaths::resolve(&e, None);
        assert_eq!(p.app_state_dir, base);
        assert_eq!(p.cache_dir, base.join("cache"));
        assert_eq!(p.minecraft_dir, base.join("minecraft"));
    }
}
