//! First-run setup wizard logic.

use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::branding::APP_NAME;
use launcher_shared::{
    AppError, AppResult, ErrorCode, SUPPORTED_LANGS, SetupPlan, SetupPreview, SetupState, Text,
};
use serde_json::json;

use crate::paths::{
    LauncherPaths, PathEnv, default_app_state_dir, default_minecraft_dir, is_protected_dir, probe_writable,
    write_pointer,
};
use crate::settings::entry;
use crate::storage::config::ConfigStore;

pub const SETUP_COMPLETED_KEY: &str = "setup_wizard_completed";
pub const SETUP_VERSION_KEY: &str = "setup_wizard_version";
pub const SETUP_VERSION: u64 = 1;

const MINECRAFT_DIR_KEY: &str = "minecraft_game_dir";
const BACKUPS_DIR_KEY: &str = "world_backups_dir";

const STATE_FILES: &[&str] = &["config.json", "profiles.json", "versions.json", "profile-token.key"];

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = display(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

pub fn should_open(config: &ConfigStore, issues: &[Text]) -> bool {
    !issues.is_empty() || !config.get_bool(SETUP_COMPLETED_KEY, false)
}

fn dir_issue(env: &PathEnv, dir: &Path, protected_key: &str, unwritable_key: &str) -> Option<Text> {
    let shown = display(dir);
    if is_protected_dir(env, dir) {
        return Some(Text::key(protected_key).param("path", shown));
    }
    let error = if dir.exists() && !dir.is_dir() {
        Some("not a directory".to_string())
    } else {
        match dir.ancestors().find(|p| p.exists()) {
            Some(existing) if existing == dir => probe_writable(existing).err().map(|e| e.to_string()),
            // A folder still to make needs only that a folder can be made there: Windows lets anyone
            // make folders in `C:\` but not files.
            Some(existing) if existing.is_dir() => probe_folder(existing).err().map(|e| e.to_string()),
            Some(_) => Some("not a directory".to_string()),
            None => Some("path does not exist".to_string()),
        }
    };
    error.map(|e| Text::key(unwritable_key).param("path", shown).param("error", e))
}

/// A folder can be made in `parent` (one is made and removed).
fn probe_folder(parent: &Path) -> std::io::Result<()> {
    let probe = parent.join(format!(".launcher-folder-test-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&probe)?;
    fs::remove_dir(&probe)
}

pub fn storage_issues(env: &PathEnv, paths: &LauncherPaths) -> Vec<Text> {
    [
        dir_issue(
            env,
            &paths.app_state_dir,
            "setup_issue_app_state_protected",
            "setup_issue_app_state_unwritable",
        ),
        dir_issue(
            env,
            &paths.minecraft_dir,
            "setup_issue_minecraft_protected",
            "setup_issue_minecraft_unwritable",
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A non-empty foreign folder gets a `Launcher` subfolder so user folders are never littered.
pub fn rebase_app_state_dir(dir: &Path) -> PathBuf {
    let named_app = dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(APP_NAME));
    let has_state = STATE_FILES.iter().any(|f| dir.join(f).is_file());
    let non_empty = fs::read_dir(dir).map(|mut it| it.next().is_some()).unwrap_or(false);
    if non_empty && !named_app && !has_state { dir.join(APP_NAME) } else { dir.to_path_buf() }
}

pub fn setup_state(env: &PathEnv, paths: &LauncherPaths, config: &ConfigStore, lang: &str) -> SetupState {
    let issues = storage_issues(env, paths);
    let minecraft = default_minecraft_dir(env, &paths.app_state_dir);
    SetupState {
        should_open: should_open(config, &issues),
        lang: lang.to_string(),
        app_state_dir: display(&paths.app_state_dir),
        default_app_state_dir: display(&default_app_state_dir(env)),
        derived_minecraft_dir: display(&minecraft),
        derived_backups_dir: display(&minecraft.join("backups").join("worlds")),
        issues,
    }
}

fn absolute_dir(raw: &str) -> AppResult<PathBuf> {
    let raw = raw.trim();
    let requested = PathBuf::from(raw);
    if raw.is_empty() || !requested.is_absolute() {
        return Err(
            AppError::new(ErrorCode::InvalidDirectoryPath, "path must be absolute").with_param("path", raw)
        );
    }
    Ok(requested)
}

/// The folders `apply_setup` would use for `dir` (same rebasing and derivation rules).
pub fn preview(env: &PathEnv, dir: &str) -> AppResult<SetupPreview> {
    let target = rebase_app_state_dir(&absolute_dir(dir)?);
    let minecraft = default_minecraft_dir(env, &target);
    Ok(SetupPreview {
        app_state_dir: display(&target),
        backups_dir: display(&minecraft.join("backups").join("worlds")),
        minecraft_dir: display(&minecraft),
        issue: dir_issue(env, &target, "setup_issue_app_state_protected", "setup_issue_app_state_unwritable"),
    })
}

/// `preview` as the wizard shows it: the launcher-data folder in use keeps its Minecraft and
/// backups folders (`apply_setup` leaves their overrides), so those are shown.
pub fn preview_current(
    env: &PathEnv,
    paths: &LauncherPaths,
    config: &ConfigStore,
    dir: &str,
) -> AppResult<SetupPreview> {
    let derived = preview(env, dir)?;
    if !same_path(Path::new(&derived.app_state_dir), &paths.app_state_dir) {
        return Ok(derived);
    }
    let minecraft = paths.minecraft_dir.clone();
    let backups = config
        .get_str(BACKUPS_DIR_KEY)
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| minecraft.join("backups").join("worlds"));
    Ok(SetupPreview {
        app_state_dir: derived.app_state_dir,
        minecraft_dir: display(&minecraft),
        backups_dir: display(&backups),
        issue: derived.issue,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    pub app_state_dir: PathBuf,
    pub restart_required: bool,
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

pub fn apply_setup(
    env: &PathEnv,
    paths: &LauncherPaths,
    config: &ConfigStore,
    plan: &SetupPlan,
) -> AppResult<SetupOutcome> {
    let target = rebase_app_state_dir(&absolute_dir(&plan.app_state_dir)?);
    // The wizard's choices are checked before anything is created or moved.
    let chosen = plan.updates.iter().cloned().map(entry).collect::<AppResult<Vec<_>>>()?;
    if is_protected_dir(env, &target) {
        return Err(AppError::new(ErrorCode::InvalidDirectoryPath, "protected program directory")
            .with_param("path", display(&target)));
    }
    probe_writable(&target).map_err(|e| {
        AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string()).with_param("path", display(&target))
    })?;

    let changed = !same_path(&target, &paths.app_state_dir);
    let moved_config;
    let cfg: &ConfigStore = if changed {
        for name in STATE_FILES {
            let src = paths.app_state_dir.join(name);
            let dst = target.join(name);
            if src.is_file() && !dst.exists() {
                fs::copy(&src, &dst).map_err(io_err)?;
            }
        }
        write_pointer(env, Some(&target)).map_err(io_err)?;
        moved_config = ConfigStore::open(target.join("config.json"));
        &moved_config
    } else {
        config
    };

    let mut entries = vec![
        (SETUP_COMPLETED_KEY.to_string(), json!("yes")),
        (SETUP_VERSION_KEY.to_string(), json!(SETUP_VERSION)),
    ];
    if SUPPORTED_LANGS.contains(&plan.lang.as_str()) {
        entries.push(("lang".to_string(), json!(plan.lang)));
    }
    let mut cleared = Vec::new();
    for (key, value) in chosen {
        match value {
            Some(value) => entries.push((key.to_string(), value)),
            None => cleared.push(key),
        }
    }
    cfg.set_many(entries).map_err(io_err)?;
    for key in cleared {
        if cfg.contains(key) {
            cfg.delete(key).map_err(io_err)?;
        }
    }
    // A new launcher-data folder brings Minecraft and backups along with it. The same folder keeps
    // the player's own folders: the wizard may have opened only because their drive is away.
    if changed {
        for key in [MINECRAFT_DIR_KEY, BACKUPS_DIR_KEY] {
            if cfg.contains(key) {
                cfg.delete(key).map_err(io_err)?;
            }
        }
    }
    Ok(SetupOutcome { app_state_dir: target, restart_required: changed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Os;
    use launcher_shared::{CardPlay, SettingUpdate};

    fn linux_env(home: &Path) -> PathEnv {
        PathEnv { os: Os::Linux, home: home.to_path_buf(), ..PathEnv::default() }
    }

    fn open_config(paths: &LauncherPaths) -> ConfigStore {
        ConfigStore::open(paths.app_state_dir.join("config.json"))
    }

    fn plan(lang: &str, dir: &Path) -> SetupPlan {
        SetupPlan {
            lang: lang.into(),
            app_state_dir: dir.to_string_lossy().into_owned(),
            updates: Vec::new(),
        }
    }

    #[test]
    fn wizard_opens_until_completed_or_when_storage_has_issues() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        assert!(should_open(&cfg, &[]));
        cfg.set("lang", json!("uk_UA")).unwrap();
        cfg.set("world_backups_enabled", json!("no")).unwrap();
        assert!(should_open(&cfg, &[]));
        cfg.set(SETUP_COMPLETED_KEY, json!("yes")).unwrap();
        assert!(!should_open(&cfg, &[]));
        assert!(should_open(&cfg, &[Text::key("x")]));
    }

    #[test]
    fn rebase_puts_app_folder_into_foreign_non_empty_dirs() {
        let base = tempfile::tempdir().unwrap();
        let empty = base.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert_eq!(rebase_app_state_dir(&empty), empty);
        let foreign = base.path().join("Documents");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("notes.txt"), "x").unwrap();
        assert_eq!(rebase_app_state_dir(&foreign), foreign.join(APP_NAME));
        let named = base.path().join(APP_NAME);
        std::fs::create_dir(&named).unwrap();
        std::fs::write(named.join("other.txt"), "x").unwrap();
        assert_eq!(rebase_app_state_dir(&named), named);
        let with_state = base.path().join("state");
        std::fs::create_dir(&with_state).unwrap();
        std::fs::write(with_state.join("config.json"), "{}").unwrap();
        assert_eq!(rebase_app_state_dir(&with_state), with_state);
    }

    #[test]
    fn storage_issue_when_path_is_a_file() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let mut paths = LauncherPaths::resolve(&env, None);
        let file = home.path().join("occupied");
        std::fs::write(&file, "x").unwrap();
        paths.app_state_dir = file;
        let issues = storage_issues(&env, &paths);
        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Text::Key { key, params } => {
                assert_eq!(key, "setup_issue_app_state_unwritable");
                assert!(params.contains_key("path") && params.contains_key("error"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn storage_issue_for_protected_program_dir() {
        let home = tempfile::tempdir().unwrap();
        let mut env = linux_env(home.path());
        let exe_dir = home.path().join("Program Files").join(APP_NAME);
        std::fs::create_dir_all(&exe_dir).unwrap();
        env.exe_dir = Some(exe_dir.clone());
        let mut paths = LauncherPaths::resolve(&env, None);
        paths.minecraft_dir = exe_dir.join("minecraft");
        let keys: Vec<String> = storage_issues(&env, &paths)
            .into_iter()
            .filter_map(|t| match t {
                Text::Key { key, .. } => Some(key),
                Text::Raw { .. } => None,
            })
            .collect();
        assert_eq!(keys, vec!["setup_issue_minecraft_protected".to_string()]);
    }

    #[test]
    fn apply_same_dir_marks_completed_and_sets_lang() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        let out = apply_setup(&env, &paths, &cfg, &plan("uk_UA", &paths.app_state_dir)).unwrap();
        assert!(!out.restart_required);
        assert_eq!(cfg.get_str("lang").as_deref(), Some("uk_UA"));
        assert!(cfg.get_bool(SETUP_COMPLETED_KEY, false));
        assert_eq!(cfg.get_u64(SETUP_VERSION_KEY), Some(1));
    }

    #[test]
    fn apply_new_dir_copies_state_and_writes_pointer() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        paths.ensure_dirs().unwrap();
        let cfg = open_config(&paths);
        cfg.set("compact_sidebar", json!("no")).unwrap();
        std::fs::write(paths.app_state_dir.join("profiles.json"), "{}").unwrap();
        let target = home.path().join("games").join(APP_NAME);
        let out = apply_setup(&env, &paths, &cfg, &plan("en_US", &target)).unwrap();
        assert!(out.restart_required);
        assert_eq!(out.app_state_dir, target);
        assert!(target.join("profiles.json").is_file());
        assert_eq!(crate::paths::read_pointer(&env), Some(target.clone()));
        let moved = ConfigStore::open(target.join("config.json"));
        assert_eq!(moved.get_str("compact_sidebar").as_deref(), Some("no"));
        assert_eq!(moved.get_str("lang").as_deref(), Some("en_US"));
    }

    #[test]
    fn apply_rejects_relative_and_unwritable_paths() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        let rel = plan("en_US", Path::new("relative/dir"));
        assert_eq!(apply_setup(&env, &paths, &cfg, &rel).unwrap_err().code, ErrorCode::InvalidDirectoryPath);
        let file = home.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let under_file = plan("en_US", &file.join("sub"));
        assert_eq!(
            apply_setup(&env, &paths, &cfg, &under_file).unwrap_err().code,
            ErrorCode::DirectoryCreateFailed
        );
        assert!(!cfg.get_bool(SETUP_COMPLETED_KEY, false));
    }

    #[test]
    fn preview_rebases_a_foreign_dir_like_apply_does() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let docs = home.path().join("Documents");
        std::fs::create_dir(&docs).unwrap();
        std::fs::write(docs.join("notes.txt"), "x").unwrap();
        let p = preview(&env, docs.to_str().unwrap()).unwrap();
        assert_eq!(p.app_state_dir, docs.join(APP_NAME).to_string_lossy());
        assert_eq!(p.minecraft_dir, docs.join(APP_NAME).join("minecraft").to_string_lossy());
        assert_eq!(
            p.backups_dir,
            docs.join(APP_NAME).join("minecraft").join("backups").join("worlds").to_string_lossy()
        );
    }

    #[test]
    fn preview_of_the_default_dir_uses_the_system_minecraft_dir() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let default = default_app_state_dir(&env);
        let p = preview(&env, default.to_str().unwrap()).unwrap();
        assert_eq!(p.minecraft_dir, default_minecraft_dir(&env, &default).to_string_lossy());
        assert_eq!(
            p.minecraft_dir,
            home.path().join(".local").join("share").join(APP_NAME).to_string_lossy()
        );
    }

    #[test]
    fn the_wizard_shows_the_folders_it_keeps() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let custom = home.path().join("games").join("MC");
        let paths = LauncherPaths::resolve(&env, Some(custom.to_str().unwrap()));
        let cfg = open_config(&paths);
        cfg.set("world_backups_dir", json!(home.path().join("saves-copies").to_string_lossy())).unwrap();
        let p = preview_current(&env, &paths, &cfg, paths.app_state_dir.to_str().unwrap()).unwrap();
        assert_eq!(p.minecraft_dir, custom.to_string_lossy(), "the folder in use, not the derived one");
        assert_eq!(p.backups_dir, home.path().join("saves-copies").to_string_lossy());
        cfg.delete("world_backups_dir").unwrap();
        let p = preview_current(&env, &paths, &cfg, paths.app_state_dir.to_str().unwrap()).unwrap();
        assert_eq!(p.backups_dir, custom.join("backups").join("worlds").to_string_lossy());
    }

    #[test]
    fn a_new_launcher_folder_shows_derived_folders() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, Some(home.path().join("MC").to_str().unwrap()));
        let cfg = open_config(&paths);
        let target = home.path().join("games").join(APP_NAME);
        let p = preview_current(&env, &paths, &cfg, target.to_str().unwrap()).unwrap();
        assert_eq!(p, preview(&env, target.to_str().unwrap()).unwrap(), "a new folder brings its own");
    }

    #[test]
    fn the_wizard_s_choices_go_to_the_folder_it_moves_to() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        paths.ensure_dirs().unwrap();
        let cfg = open_config(&paths);
        // The new folder already has settings of its own: the wizard's choices still win there.
        let target = home.path().join("games").join(APP_NAME);
        std::fs::create_dir_all(&target).unwrap();
        let theirs = ConfigStore::open(target.join("config.json"));
        theirs.set("compact_sidebar", json!("yes")).unwrap();
        theirs.set("default_max_ram_gb", json!(6)).unwrap();
        drop(theirs);
        let mut chosen = plan("en_US", &target);
        chosen.updates = vec![
            SettingUpdate::CompactSidebar(false),
            SettingUpdate::CardPlay(CardPlay::Corner),
            SettingUpdate::DefaultMaxRamGb(None),
        ];
        assert!(apply_setup(&env, &paths, &cfg, &chosen).unwrap().restart_required);
        let moved = ConfigStore::open(target.join("config.json"));
        assert_eq!(moved.get_str("compact_sidebar").as_deref(), Some("no"));
        assert_eq!(moved.get_str("home_card_play").as_deref(), Some("corner"));
        assert_eq!(moved.get("default_max_ram_gb"), None, "back to the recommended amount");
        assert!(moved.get_bool(SETUP_COMPLETED_KEY, false));
    }

    #[test]
    fn the_wizard_s_choices_apply_in_the_same_folder_and_its_language_wins() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        let mut chosen = plan("uk_UA", &paths.app_state_dir);
        chosen.updates = vec![SettingUpdate::Lang("en_US".into()), SettingUpdate::HomeRecentBuilds(5)];
        assert!(!apply_setup(&env, &paths, &cfg, &chosen).unwrap().restart_required);
        assert_eq!(cfg.get_str("lang").as_deref(), Some("en_US"), "the language picked in the wizard");
        assert_eq!(cfg.get_u64("home_recent_builds"), Some(5));
    }

    #[test]
    fn a_preview_tries_a_new_folder_where_it_would_be_made_and_leaves_nothing() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let games = home.path().join("games");
        std::fs::create_dir(&games).unwrap();
        let p = preview(&env, games.join("Launcher").join("data").to_str().unwrap()).unwrap();
        assert_eq!(p.issue, None);
        assert_eq!(std::fs::read_dir(&games).unwrap().count(), 0, "nothing is left where it looked");
    }

    #[test]
    fn the_wizard_s_choices_are_checked_before_anything_is_written() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        let target = home.path().join("games").join(APP_NAME);
        let mut chosen = plan("en_US", &target);
        chosen.updates = vec![SettingUpdate::CompactSidebar(false), SettingUpdate::HomeRecentBuilds(200)];
        let err = apply_setup(&env, &paths, &cfg, &chosen).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(!target.exists(), "nothing was created");
        assert!(!cfg.get_bool(SETUP_COMPLETED_KEY, false));
        assert_eq!(crate::paths::read_pointer(&env), None);
    }

    #[test]
    fn preview_names_a_folder_that_cannot_be_used_without_creating_it() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let fine = home.path().join("games").join("Launcher");
        let p = preview(&env, fine.to_str().unwrap()).unwrap();
        assert_eq!(p.issue, None);
        assert!(!fine.exists(), "a preview creates nothing");
        let file = home.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let p = preview(&env, file.join("sub").to_str().unwrap()).unwrap();
        match p.issue {
            Some(Text::Key { key, .. }) => assert_eq!(key, "setup_issue_app_state_unwritable"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn preview_rejects_relative_paths() {
        let home = tempfile::tempdir().unwrap();
        let err = preview(&linux_env(home.path()), "relative/dir").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidDirectoryPath);
    }

    #[test]
    fn a_new_launcher_folder_clears_minecraft_and_backup_overrides() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        paths.ensure_dirs().unwrap();
        let cfg = open_config(&paths);
        cfg.set("minecraft_game_dir", json!("E:\\MC")).unwrap();
        cfg.set("world_backups_dir", json!("E:\\MC\\backups")).unwrap();
        let target = home.path().join("games").join(APP_NAME);
        apply_setup(&env, &paths, &cfg, &plan("uk_UA", &target)).unwrap();
        let moved = ConfigStore::open(target.join("config.json"));
        assert_eq!(moved.get("minecraft_game_dir"), None);
        assert_eq!(moved.get("world_backups_dir"), None);
        assert!(moved.get_bool(SETUP_COMPLETED_KEY, false));
    }

    #[test]
    fn the_wizard_keeps_custom_folders_it_does_not_change() {
        let home = tempfile::tempdir().unwrap();
        let env = linux_env(home.path());
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = open_config(&paths);
        // The wizard opened because drive E: is away; the player only pressed "Continue".
        cfg.set("minecraft_game_dir", json!("E:\\MC")).unwrap();
        cfg.set("world_backups_dir", json!("E:\\MC\\backups")).unwrap();
        apply_setup(&env, &paths, &cfg, &plan("uk_UA", &paths.app_state_dir)).unwrap();
        assert_eq!(cfg.get_str("minecraft_game_dir").as_deref(), Some("E:\\MC"));
        assert_eq!(cfg.get_str("world_backups_dir").as_deref(), Some("E:\\MC\\backups"));
        assert!(cfg.get_bool(SETUP_COMPLETED_KEY, false));
    }
}
