//! Typed launcher settings over `ConfigStore`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use launcher_shared::recent::{RECENT_DEFAULT, RECENT_MOST};
use launcher_shared::{
    AppError, AppResult, ClickSound, ErrorCode, GameStartAction, SUPPORTED_LANGS, SettingUpdate,
    SettingsSnapshot, WindowSize,
};
use serde_json::json;

use crate::builds::service::{GPU_MODE_DEFAULT_KEY, default_gpu_mode};
use crate::java::memory::parse_memory_value;
use crate::launch::options::DEFAULT_MAX_RAM_KEY;
use crate::paths::{LauncherPaths, PathEnv, default_minecraft_dir, is_protected_dir, probe_writable};
use crate::storage::config::{ConfigStore, yes_no};

/// What the launcher does once the game it started runs (`GameStartAction`).
pub const ON_GAME_START_KEY: &str = "on_game_start";
/// The switch it replaced: "yes" closed the launcher.
const CLOSE_ON_GAME_KEY: &str = "close_launcher_on_game";

/// What the launcher does once the game it started runs: the user's choice, else what the old
/// switch said.
pub fn game_start_action(config: &ConfigStore) -> GameStartAction {
    match config.get_str(ON_GAME_START_KEY).as_deref().and_then(GameStartAction::from_config_str) {
        Some(action) => action,
        None if config.get_bool(CLOSE_ON_GAME_KEY, false) => GameStartAction::Close,
        None => GameStartAction::Nothing,
    }
}

pub const MINECRAFT_DIR_KEY: &str = "minecraft_game_dir";
pub const WINDOW_SIZE_KEY: &str = "window_size";
/// How many builds Home's «Продовжити гру» shows.
pub const HOME_RECENT_KEY: &str = "home_recent_builds";

pub fn detect_system_lang(locale: Option<&str>) -> String {
    match locale {
        Some(l) if l.to_lowercase().starts_with("uk") => "uk_UA".to_string(),
        _ => "en_US".to_string(),
    }
}

pub struct SettingsService {
    pub env: PathEnv,
    pub paths: LauncherPaths,
    pub config: Arc<ConfigStore>,
    system_lang: String,
    /// Changes saved so far; taken with the change, so their snapshots are numbered in order.
    revision: Mutex<u64>,
    /// The last number given, for the snapshots read meanwhile.
    last: AtomicU64,
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

impl SettingsService {
    pub fn new(env: PathEnv, paths: LauncherPaths, config: Arc<ConfigStore>, system_lang: String) -> Self {
        Self { env, paths, config, system_lang, revision: Mutex::new(0), last: AtomicU64::new(0) }
    }

    pub fn lang(&self) -> String {
        self.config
            .get_str("lang")
            .filter(|l| SUPPORTED_LANGS.contains(&l.as_str()))
            .unwrap_or_else(|| self.system_lang.clone())
    }

    pub fn snapshot(&self) -> SettingsSnapshot {
        self.snapshot_at(self.last.load(Ordering::SeqCst))
    }

    fn snapshot_at(&self, revision: u64) -> SettingsSnapshot {
        let c = &self.config;
        SettingsSnapshot {
            revision,
            lang: self.lang(),
            auto_update: c.get_bool("auto_update", true),
            include_beta_updates: c.get_bool("include_beta_updates", false),
            on_game_start: game_start_action(c),
            ask_profile_on_launch: c.get_bool("ask_profile_on_launch", false),
            compact_sidebar: c.get_bool("compact_sidebar", true),
            click_sound_enabled: c.get_bool("ui_click_sound_enabled", true),
            click_sound: ClickSound::from_config_str(&c.get_str("ui_click_sound").unwrap_or_default()),
            minecraft_dir: self.paths.minecraft_dir.to_string_lossy().into_owned(),
            minecraft_dir_is_default: self.paths.minecraft_dir_is_default,
            default_minecraft_dir: default_minecraft_dir(&self.env, &self.paths.app_state_dir)
                .to_string_lossy()
                .into_owned(),
            default_max_ram_gb: c.get(DEFAULT_MAX_RAM_KEY).as_ref().and_then(parse_memory_value),
            gpu_mode_default: default_gpu_mode(c).to_string(),
            window_size: c
                .get_str(WINDOW_SIZE_KEY)
                .and_then(|raw| WindowSize::parse(&raw))
                .unwrap_or_default()
                .as_config_str(),
            home_recent_builds: c
                .get_u64(HOME_RECENT_KEY)
                .map_or(RECENT_DEFAULT, |n| n.min(u64::from(RECENT_MOST)) as u8),
        }
    }

    pub fn apply(&self, update: SettingUpdate) -> AppResult<SettingsSnapshot> {
        let invalid = |what: &str| AppError::new(ErrorCode::InvalidInput, what.to_string());
        let (key, value) = match update {
            SettingUpdate::Lang(lang) => {
                if !SUPPORTED_LANGS.contains(&lang.as_str()) {
                    return Err(invalid("unsupported language").with_param("lang", lang));
                }
                ("lang", Some(json!(lang)))
            }
            SettingUpdate::AutoUpdate(v) => ("auto_update", Some(json!(yes_no(v)))),
            SettingUpdate::IncludeBetaUpdates(v) => ("include_beta_updates", Some(json!(yes_no(v)))),
            SettingUpdate::OnGameStart(action) => (ON_GAME_START_KEY, Some(json!(action.as_config_str()))),
            SettingUpdate::AskProfileOnLaunch(v) => ("ask_profile_on_launch", Some(json!(yes_no(v)))),
            SettingUpdate::CompactSidebar(v) => ("compact_sidebar", Some(json!(yes_no(v)))),
            SettingUpdate::ClickSoundEnabled(v) => ("ui_click_sound_enabled", Some(json!(yes_no(v)))),
            SettingUpdate::ClickSound(s) => ("ui_click_sound", Some(json!(s.as_config_str()))),
            SettingUpdate::DefaultMaxRamGb(Some(0)) => {
                return Err(invalid("the memory limit must be at least 1 GB"));
            }
            SettingUpdate::DefaultMaxRamGb(gb) => (DEFAULT_MAX_RAM_KEY, gb.map(|gb| json!(gb))),
            SettingUpdate::GpuModeDefault(mode) => {
                if !["auto", "igpu", "dgpu"].contains(&mode.as_str()) {
                    return Err(invalid("unknown GPU mode").with_param("mode", mode));
                }
                (GPU_MODE_DEFAULT_KEY, Some(json!(mode)))
            }
            SettingUpdate::WindowSize(raw) => match WindowSize::parse(&raw) {
                Some(size) => (WINDOW_SIZE_KEY, Some(json!(size.as_config_str()))),
                None => return Err(invalid("unknown window size").with_param("size", raw)),
            },
            SettingUpdate::HomeRecentBuilds(count) if count > RECENT_MOST => {
                return Err(invalid("too many recent builds").with_param("count", count.to_string()));
            }
            SettingUpdate::HomeRecentBuilds(count) => (HOME_RECENT_KEY, Some(json!(count))),
        };
        let mut revision = self.revision.lock().unwrap_or_else(|e| e.into_inner());
        match value {
            Some(value) => self.config.set(key, value),
            None => self.config.delete(key),
        }
        .map_err(io_err)?;
        *revision += 1;
        self.last.store(*revision, Ordering::SeqCst);
        Ok(self.snapshot_at(*revision))
    }

    /// Validates and stores the Minecraft directory. Returns `true` when a restart is needed.
    pub fn save_minecraft_dir(&self, raw: &str) -> AppResult<bool> {
        let raw = raw.trim();
        let state = &self.paths.app_state_dir;
        let default = default_minecraft_dir(&self.env, state);
        if raw.is_empty() {
            self.config.delete(MINECRAFT_DIR_KEY).map_err(io_err)?;
            return Ok(self.paths.minecraft_dir != default);
        }
        let candidate = PathBuf::from(raw);
        let candidate = if candidate.is_absolute() { candidate } else { state.join(candidate) };
        let shown = candidate.to_string_lossy().into_owned();
        if is_protected_dir(&self.env, &candidate) {
            return Err(AppError::new(ErrorCode::InvalidDirectoryPath, "protected program directory")
                .with_param("path", shown));
        }
        probe_writable(&candidate).map_err(|e| {
            AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string()).with_param("path", shown)
        })?;

        if candidate == default {
            self.config.delete(MINECRAFT_DIR_KEY).map_err(io_err)?;
        } else {
            let stored =
                relative_to(&candidate, state).unwrap_or_else(|| candidate.to_string_lossy().into_owned());
            self.config.set(MINECRAFT_DIR_KEY, json!(stored)).map_err(io_err)?;
        }
        Ok(candidate != self.paths.minecraft_dir)
    }
}

fn relative_to(path: &Path, base: &Path) -> Option<String> {
    path.strip_prefix(base)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(|rel| rel.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Os;
    use launcher_shared::{ClickSound, ErrorCode};

    fn service(home: &Path) -> SettingsService {
        let env = PathEnv { os: Os::Linux, home: home.to_path_buf(), ..PathEnv::default() };
        let paths = LauncherPaths::resolve(&env, None);
        paths.ensure_dirs().unwrap();
        let config = Arc::new(ConfigStore::open(paths.app_state_dir.join("config.json")));
        SettingsService::new(env, paths, config, "uk_UA".into())
    }

    #[test]
    fn detects_system_language() {
        assert_eq!(detect_system_lang(Some("uk-UA")), "uk_UA");
        assert_eq!(detect_system_lang(Some("UK")), "uk_UA");
        assert_eq!(detect_system_lang(Some("en-GB")), "en_US");
        assert_eq!(detect_system_lang(None), "en_US");
    }

    #[test]
    fn snapshot_defaults() {
        let home = tempfile::tempdir().unwrap();
        let s = service(home.path()).snapshot();
        assert_eq!(s.lang, "uk_UA");
        assert!(s.auto_update && s.compact_sidebar && s.click_sound_enabled);
        assert!(!s.include_beta_updates && !s.ask_profile_on_launch);
        assert_eq!(s.click_sound, ClickSound::GateLatchClick);
        assert!(s.minecraft_dir_is_default);
        assert_eq!(s.default_minecraft_dir, s.minecraft_dir, "the default is the folder in use");
        assert_eq!(s.home_recent_builds, 5);
    }

    #[test]
    fn home_shows_up_to_ten_recent_builds_or_none() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        assert_eq!(svc.apply(SettingUpdate::HomeRecentBuilds(0)).unwrap().home_recent_builds, 0);
        assert_eq!(svc.config.get_u64("home_recent_builds"), Some(0));
        assert_eq!(svc.apply(SettingUpdate::HomeRecentBuilds(10)).unwrap().home_recent_builds, 10);
        assert_eq!(svc.apply(SettingUpdate::HomeRecentBuilds(11)).unwrap_err().code, ErrorCode::InvalidInput);
        svc.config.set("home_recent_builds", json!(400)).unwrap();
        assert_eq!(svc.snapshot().home_recent_builds, 10, "a hand-edited count is held to the most");
    }

    #[test]
    fn each_change_numbers_its_snapshot_after_the_last() {
        // Two quick switches are saved side by side: the window keeps the later answer, whichever
        // arrives last, by its number.
        let home = tempfile::tempdir().unwrap();
        let svc = Arc::new(service(home.path()));
        let before = svc.snapshot().revision;
        let first = svc.apply(SettingUpdate::CompactSidebar(false)).unwrap();
        let second = svc.apply(SettingUpdate::AutoUpdate(false)).unwrap();
        assert!(before < first.revision && first.revision < second.revision);
        assert_eq!(svc.snapshot().revision, second.revision);
        assert!(second.replaces(&first) && !first.replaces(&second));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let svc = svc.clone();
                std::thread::spawn(move || svc.apply(SettingUpdate::CompactSidebar(i % 2 == 0)).unwrap())
            })
            .collect();
        let mut answers: Vec<SettingsSnapshot> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        answers.sort_by_key(|s| s.revision);
        let last = answers.last().unwrap();
        assert_eq!(*last, svc.snapshot(), "the highest number tells what is saved");
    }

    #[test]
    fn nothing_follows_the_game_start_until_the_user_chooses() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        assert_eq!(svc.snapshot().on_game_start, GameStartAction::Nothing);
        let snap = svc.apply(SettingUpdate::OnGameStart(GameStartAction::Tray)).unwrap();
        assert_eq!(snap.on_game_start, GameStartAction::Tray);
        assert_eq!(svc.config.get_str("on_game_start").as_deref(), Some("tray"));
        assert_eq!(game_start_action(&svc.config), GameStartAction::Tray);
    }

    #[test]
    fn the_old_close_on_game_switch_still_closes() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        svc.config.set("close_launcher_on_game", json!("yes")).unwrap();
        assert_eq!(svc.snapshot().on_game_start, GameStartAction::Close);
        svc.apply(SettingUpdate::OnGameStart(GameStartAction::Nothing)).unwrap();
        assert_eq!(
            svc.snapshot().on_game_start,
            GameStartAction::Nothing,
            "the choice wins over the old switch"
        );
    }

    #[test]
    fn apply_persists_yes_no_and_validates_lang() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        let snap = svc.apply(SettingUpdate::AskProfileOnLaunch(true)).unwrap();
        assert!(snap.ask_profile_on_launch);
        assert_eq!(svc.config.get_str("ask_profile_on_launch").as_deref(), Some("yes"));
        svc.apply(SettingUpdate::ClickSound(ClickSound::PlasticBubbleClick)).unwrap();
        assert_eq!(svc.config.get_str("ui_click_sound").as_deref(), Some("plastic_bubble_click"));
        assert_eq!(svc.apply(SettingUpdate::Lang("de_DE".into())).unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(svc.apply(SettingUpdate::Lang("en_US".into())).unwrap().lang, "en_US");
    }

    #[test]
    fn save_minecraft_dir_stores_relative_absolute_and_default() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        let inside = svc.paths.app_state_dir.join("mc");
        assert!(svc.save_minecraft_dir(inside.to_str().unwrap()).unwrap());
        assert_eq!(svc.config.get_str("minecraft_game_dir").as_deref(), Some("mc"));
        let outside = home.path().join("elsewhere");
        svc.save_minecraft_dir(outside.to_str().unwrap()).unwrap();
        assert_eq!(svc.config.get_str("minecraft_game_dir"), Some(outside.to_string_lossy().into_owned()));
        let default = svc.paths.minecraft_dir.clone();
        assert!(!svc.save_minecraft_dir(default.to_str().unwrap()).unwrap());
        assert_eq!(svc.config.get("minecraft_game_dir"), None);
        assert!(!svc.save_minecraft_dir("   ").unwrap());
    }

    #[test]
    fn save_minecraft_dir_rejects_unwritable_and_protected() {
        let home = tempfile::tempdir().unwrap();
        let mut svc = service(home.path());
        let file = home.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let err = svc.save_minecraft_dir(file.join("sub").to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, ErrorCode::DirectoryCreateFailed);
        let exe_dir = home.path().join("Program Files").join("Launcher");
        svc.env.exe_dir = Some(exe_dir.clone());
        let err = svc.save_minecraft_dir(exe_dir.join("data").to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidDirectoryPath);
        assert_eq!(svc.config.get("minecraft_game_dir"), None);
    }

    #[test]
    fn memory_and_gpu_defaults_are_stored_like_the_original() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        let snap = svc.snapshot();
        assert_eq!((snap.default_max_ram_gb, snap.gpu_mode_default.as_str()), (None, "dgpu"));
        assert_eq!(svc.apply(SettingUpdate::DefaultMaxRamGb(Some(6))).unwrap().default_max_ram_gb, Some(6));
        assert_eq!(svc.config.get("default_max_ram_gb"), Some(json!(6)));
        svc.config.set("default_max_ram_gb", json!("-Xmx4096M")).unwrap();
        assert_eq!(svc.snapshot().default_max_ram_gb, Some(4), "values the original wrote still read");
        assert_eq!(
            svc.apply(SettingUpdate::DefaultMaxRamGb(Some(0))).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(svc.apply(SettingUpdate::DefaultMaxRamGb(None)).unwrap().default_max_ram_gb, None);
        assert_eq!(svc.config.get("default_max_ram_gb"), None);
        assert_eq!(svc.apply(SettingUpdate::GpuModeDefault("igpu".into())).unwrap().gpu_mode_default, "igpu");
        assert_eq!(svc.config.get_str("gpu_mode_default").as_deref(), Some("igpu"));
        let err = svc.apply(SettingUpdate::GpuModeDefault("rtx".into())).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(svc.config.get_str("gpu_mode_default").as_deref(), Some("igpu"));
    }

    #[test]
    fn window_size_is_validated_and_stored() {
        let home = tempfile::tempdir().unwrap();
        let svc = service(home.path());
        assert_eq!(svc.snapshot().window_size, "1366x800");
        assert_eq!(
            svc.apply(SettingUpdate::WindowSize(" FULLSCREEN ".into())).unwrap().window_size,
            "fullscreen"
        );
        assert_eq!(svc.config.get_str("window_size").as_deref(), Some("fullscreen"));
        assert_eq!(svc.apply(SettingUpdate::WindowSize("1366x800".into())).unwrap().window_size, "1366x800");
        let err = svc.apply(SettingUpdate::WindowSize("100x100".into())).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        svc.config.set("window_size", json!("junk")).unwrap();
        assert_eq!(svc.snapshot().window_size, "1366x800", "junk reads as the default");
    }
}
