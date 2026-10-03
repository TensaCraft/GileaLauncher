//! In-browser mock of the Tauri backend (only used when `window.__TAURI__` is absent).

use std::cell::RefCell;
use std::rc::Rc;

use launcher_shared::{
    AccountKind, ActivityEntry, ActivityEvent, AppError, AppInfo, AuthState, ClickSound, ErrorCode, Level,
    LogEntry, LogLevel, LogView, ModuleInfo, OpsSnapshot, PathsInfo, ProfileDto, ProfilesSnapshot,
    SettingUpdate, SettingsSnapshot, SetupPreview, SetupState, Text, UpdateChannel, UpdateInfo, UpdateState,
    UpdateStatus, names, sort_profiles,
};
use serde_json::{Value, json};
use ui_kit::ipc;

pub fn apply_update(s: &mut SettingsSnapshot, update: &SettingUpdate) {
    match update.clone() {
        SettingUpdate::Lang(v) => s.lang = v,
        SettingUpdate::AutoUpdate(v) => s.auto_update = v,
        SettingUpdate::IncludeBetaUpdates(v) => s.include_beta_updates = v,
        SettingUpdate::OnGameStart(v) => s.on_game_start = v,
        SettingUpdate::AskProfileOnLaunch(v) => s.ask_profile_on_launch = v,
        SettingUpdate::CompactSidebar(v) => s.compact_sidebar = v,
        SettingUpdate::ClickSoundEnabled(v) => s.click_sound_enabled = v,
        SettingUpdate::ClickSound(v) => s.click_sound = v,
        SettingUpdate::DefaultMaxRamGb(v) => s.default_max_ram_gb = v,
        SettingUpdate::GpuModeDefault(v) => s.gpu_mode_default = v,
        SettingUpdate::WindowSize(v) => s.window_size = v,
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, AppError> {
    serde_json::to_value(v).map_err(|e| AppError::internal(e.to_string()))
}

fn query_flag(name: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains(&format!("{name}=1")))
        .unwrap_or(false)
}

fn query_value(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search.trim_start_matches('?').split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn mock_update_info() -> UpdateInfo {
    UpdateInfo {
        version: "0.2.0".into(),
        channel: UpdateChannel::Stable,
        // As `cargo xtask release-notes` writes them.
        notes: "# v0.2.0\n\nChanges since `v0.1.0`.\n\n## New Features\n\n- **Updates:** the launcher updates itself\n\n## Fixes\n\n- **Downloads:** a broken download resumes\n- stability fixes\n"
            .into(),
        asset_name: "Launcher.exe".into(),
        size: 13_000_000,
        published_at: Some("2026-09-26T12:00:00Z".into()),
    }
}

fn mock_update_status(mode: &str) -> UpdateStatus {
    let mut status = UpdateStatus::idle(
        mode != "off",
        env!("CARGO_PKG_VERSION").into(),
        Some("http://127.0.0.1:1430".into()),
        mode != "dev",
        (mode == "updated").then(|| "0.0.9".to_string()),
    );
    if mode == "ready" {
        status.state = UpdateState::Ready { info: mock_update_info() };
    }
    status
}

/// A full rotated log (`?log=big`): the viewer's limit of records, older ones left out.
pub fn mock_big_log_view() -> LogView {
    let levels =
        [LogLevel::Info, LogLevel::Info, LogLevel::Debug, LogLevel::Warn, LogLevel::Info, LogLevel::Trace];
    let entries = (0..5000)
        .map(|i| {
            let (level, message) = if i % 97 == 0 {
                (LogLevel::Error, format!("Launch {i} failed: the game exited with code 1\nCaused by:\n    0: mixin apply failed"))
            } else {
                (levels[i % levels.len()], format!("Operation {i}: downloading library {} of 212", i % 212))
            };
            let time = format!("2026-09-27 {:02}:{:02}:{:02}", 8 + i / 3600, i / 60 % 60, i % 60);
            LogEntry { time, level: Some(level), message }
        })
        .collect();
    LogView { file: mock_log_view().file, entries, skipped: 11000 }
}

/// A launcher log for the browser preview.
pub fn mock_log_view() -> LogView {
    let rows: [(&str, LogLevel, &str); 12] = [
        ("2026-09-27 12:00:00", LogLevel::Info, "Logging initialised"),
        ("2026-09-27 12:00:00", LogLevel::Info, "Launcher 0.1.0 (full) on windows"),
        ("2026-09-27 12:00:01", LogLevel::Debug, "config.json loaded (24 keys)"),
        ("2026-09-27 12:00:01", LogLevel::Info, "Modules: modrinth, backups, reports"),
        ("2026-09-27 12:00:02", LogLevel::Trace, "java scan: 3 roots"),
        ("2026-09-27 12:00:03", LogLevel::Warn, "Modrinth answered slowly (2.4 s)"),
        ("2026-09-27 12:00:05", LogLevel::Info, "Build aeronautics: launching with Java 21"),
        (
            "2026-09-27 12:00:09",
            LogLevel::Error,
            "Launch failed: the game exited with code 1\nCaused by:\n    0: mixin apply failed for sodium\n    1: java.lang.NoSuchMethodError",
        ),
        ("2026-09-27 12:00:10", LogLevel::Info, "Crash report saved"),
        ("2026-09-27 12:00:12", LogLevel::Debug, "update check skipped: checked 10 min ago"),
        ("2026-09-27 12:00:15", LogLevel::Warn, "Low free space on D: (1.2 GB)"),
        ("2026-09-27 12:00:20", LogLevel::Info, "Settings saved"),
    ];
    LogView {
        file: "C:\\Users\\Player\\AppData\\Local\\Launcher\\app.log".into(),
        entries: rows
            .into_iter()
            .map(|(time, level, message)| LogEntry {
                time: time.into(),
                level: Some(level),
                message: message.into(),
            })
            .collect(),
        skipped: 0,
    }
}

fn mock_profile(name: &str, kind: AccountKind, is_default: bool) -> ProfileDto {
    let id = match kind {
        AccountKind::Microsoft => "0123456789abcdef0123456789abcdef",
        AccountKind::Offline => "00000000-0000-0000-0000-000000000000",
    };
    ProfileDto {
        key: name.into(),
        name: name.into(),
        id: id.into(),
        kind,
        is_default,
        reauth_required: false,
        reauth_reason: None,
    }
}

fn mock_snapshot(list: &[ProfileDto]) -> ProfilesSnapshot {
    let mut profiles = list.to_vec();
    sort_profiles(&mut profiles);
    ProfilesSnapshot { profiles }
}

/// The core's offline rules, close enough for the browser preview.
pub fn mock_add_offline(list: &mut Vec<ProfileDto>, raw: &str) -> Result<(), AppError> {
    let name = raw.trim();
    let count = name.chars().count();
    if count == 0 || count > 16 || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::new(ErrorCode::ProfileNameInvalid, "mock"));
    }
    if list.iter().any(|p| p.key == name) {
        return Err(AppError::new(ErrorCode::ProfileExists, "mock").with_param("name", name));
    }
    list.iter_mut().for_each(|p| p.is_default = false);
    list.push(mock_profile(name, AccountKind::Offline, true));
    Ok(())
}

pub fn mock_delete(list: &mut Vec<ProfileDto>, key: &str) {
    list.retain(|p| p.key != key);
    if !list.iter().any(|p| p.is_default)
        && let Some(first) = list.iter_mut().min_by_key(|p| p.name.to_lowercase())
    {
        first.is_default = true;
    }
}

/// The providers the preview has (the modules compiled in).
// `vec![]` cannot hold `#[cfg]`-gated elements, hence push-after-new.
#[allow(clippy::vec_init_then_push)]
fn preview_providers() -> Vec<launcher_shared::provider::ProviderInfo> {
    #[allow(unused_mut)]
    let mut list = Vec::new();
    #[cfg(feature = "mod-modrinth")]
    list.push(module_modrinth::types::provider_info());
    #[cfg(all(feature = "mod-modrinth", feature = "mod-curseforge"))]
    list.push(module_curseforge::types::provider_info());
    list
}

pub fn install() {
    let settings = Rc::new(RefCell::new(SettingsSnapshot {
        lang: "uk_UA".into(),
        auto_update: true,
        include_beta_updates: false,
        on_game_start: Default::default(),
        ask_profile_on_launch: false,
        compact_sidebar: true,
        click_sound_enabled: true,
        click_sound: ClickSound::GateLatchClick,
        minecraft_dir: "C:\\Users\\Player\\AppData\\Roaming\\Launcher".into(),
        minecraft_dir_is_default: true,
        default_minecraft_dir: "C:\\Users\\Player\\AppData\\Roaming\\Launcher".into(),
        default_max_ram_gb: None,
        gpu_mode_default: "dgpu".into(),
        window_size: "1366x800".into(),
        revision: 0,
    }));
    let info = AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        profile: "browser-preview".into(),
        dev_mode: true,
        os: "windows".into(),
        support_url: Some("https://discord.com/invite/mftAjQA4Pp".into()),
        issues_url: Some(launcher_shared::branding::ISSUES_URL.into()),
        updates_configured: false,
        update_source: None,
        modules: crate::modules::ui_modules()
            .iter()
            .map(|m| ModuleInfo { id: m.id().into(), version: env!("CARGO_PKG_VERSION").into() })
            .collect(),
        providers: preview_providers(),
        paths: PathsInfo {
            app_state_dir: "C:\\Users\\Player\\AppData\\Local\\Launcher".into(),
            minecraft_dir: "C:\\Users\\Player\\AppData\\Roaming\\Launcher".into(),
            cache_dir: "C:\\Users\\Player\\AppData\\Local\\Launcher\\cache".into(),
            log_dir: "C:\\Users\\Player\\AppData\\Local\\Launcher".into(),
            log_file: "C:\\Users\\Player\\AppData\\Local\\Launcher\\app.log".into(),
        },
    };
    let update_mode = query_value("update").unwrap_or_else(|| "available".into());
    let log_mode = query_value("log").unwrap_or_default();
    let update = Rc::new(RefCell::new(mock_update_status(&update_mode)));
    let auth_mode = query_value("auth").unwrap_or_default();
    let profiles = Rc::new(RefCell::new(if query_value("profiles").as_deref() == Some("none") {
        Vec::new()
    } else {
        vec![
            mock_profile("Notch_UA", AccountKind::Microsoft, true),
            mock_profile("Steve", AccountKind::Offline, false),
        ]
    }));
    let emit_profiles = {
        let profiles = profiles.clone();
        move || -> Result<Value, AppError> {
            let snapshot = mock_snapshot(&profiles.borrow());
            ipc::emit_mock(names::PROFILES, to_value(&snapshot)?);
            to_value(snapshot)
        }
    };
    let builds = Rc::new(RefCell::new(
        crate::mock_builds::MockBuilds::new(
            query_value("builds").as_deref() == Some("none"),
            query_value("catalog").as_deref() == Some("error"),
        )
        .with_no_profile(query_value("launch").as_deref() == Some("noprofile")),
    ));
    let components = Rc::new(RefCell::new(crate::mock_components::MockComponents::new()));
    let content = Rc::new(RefCell::new(crate::mock_content::MockContent::new()));
    #[cfg(feature = "mod-backups")]
    let backups = Rc::new(RefCell::new(crate::mock_backups::MockBackups::default()));
    #[cfg(feature = "mod-diagnostics")]
    let diagnostics =
        Rc::new(RefCell::new(crate::mock_diagnostics::MockDiagnostics::new(query_flag("crash"))));
    #[cfg(feature = "mod-reports")]
    let reports = Rc::new(RefCell::new(crate::mock_reports::MockReports::new(
        query_value("reports").is_some_and(|v| v.contains("fail")),
    )));
    #[cfg(feature = "mod-tensa")]
    let tensa = Rc::new(RefCell::new(crate::mock_tensa::MockTensa::new(
        query_value("tensa").as_deref() != Some("off"),
        query_value("tensa").as_deref() == Some("error"),
        builds.clone(),
    )));
    #[cfg(feature = "mod-modrinth")]
    let modrinth = Rc::new(RefCell::new(crate::mock_modrinth::MockModrinth::new(
        query_value("modrinth").as_deref() == Some("error"),
        builds.clone(),
    )));
    let handler: ipc::MockHandler = Rc::new(move |cmd, args| match cmd {
        "app_info" => to_value(&info),
        "settings_get" => to_value(settings.borrow().clone()),
        "settings_set" => {
            let update: SettingUpdate = serde_json::from_value(args["update"].clone())
                .map_err(|e| AppError::internal(e.to_string()))?;
            apply_update(&mut settings.borrow_mut(), &update);
            settings.borrow_mut().revision += 1;
            let snap = settings.borrow().clone();
            ipc::emit_mock(names::SETTINGS, to_value(&snap)?);
            to_value(snap)
        }
        "settings_save_minecraft_dir" => {
            settings.borrow_mut().minecraft_dir = args["path"].as_str().unwrap_or_default().to_string();
            to_value(true)
        }
        "setup_state" => to_value(SetupState {
            should_open: query_flag("setup"),
            lang: settings.borrow().lang.clone(),
            app_state_dir: info.paths.app_state_dir.clone(),
            default_app_state_dir: info.paths.app_state_dir.clone(),
            derived_minecraft_dir: info.paths.minecraft_dir.clone(),
            derived_backups_dir: format!("{}\\backups\\worlds", info.paths.minecraft_dir),
            issues: Vec::new(),
        }),
        "setup_preview" => {
            let dir = args["dir"].as_str().unwrap_or_default().trim_end_matches(['\\', '/']).to_string();
            let minecraft = if dir == info.paths.app_state_dir {
                info.paths.minecraft_dir.clone()
            } else {
                format!("{dir}\\minecraft")
            };
            to_value(SetupPreview {
                app_state_dir: dir,
                backups_dir: format!("{minecraft}\\backups\\worlds"),
                minecraft_dir: minecraft,
            })
        }
        "setup_apply" => to_value(false),
        "startup_warnings" => to_value(Vec::<Text>::new()),
        "take_pending_launch" => Ok(Value::Null),
        // The launcher plays clicks; the browser preview stays quiet.
        "play_click" => Ok(Value::Null),
        "ops_snapshot" => to_value(OpsSnapshot::default()),
        "activity_recent" => to_value(vec![
            ActivityEntry {
                seq: 2,
                at_ms: js_sys::Date::now() as u64,
                event: ActivityEvent::Notify,
                level: Level::Success,
                message: Text::key("settings_saved"),
                operation_id: None,
                kind: None,
            },
            ActivityEntry {
                seq: 1,
                at_ms: js_sys::Date::now() as u64 - 60_000,
                event: ActivityEvent::Begin,
                level: Level::Info,
                message: Text::key("installation_started"),
                operation_id: Some(1),
                kind: Some("install".into()),
            },
        ]),
        "pick_directory" => Ok(json!("D:\\Games\\Launcher")),
        #[cfg(feature = "mod-modrinth")]
        cmd if cmd.starts_with("provider_") => modrinth.borrow_mut().handle(cmd, &args),
        #[cfg(feature = "mod-backups")]
        "module_invoke" if args["module"] == "backups" => {
            backups.borrow_mut().handle(args["command"].as_str().unwrap_or_default(), &args["args"])
        }
        #[cfg(feature = "mod-diagnostics")]
        "build_launch" if diagnostics.borrow().crashes() => {
            let key = args["key"].as_str().unwrap_or_default().to_string();
            let name = builds.borrow().snapshot().builds.into_iter().find(|b| b.key == key).map(|b| b.name);
            match name {
                Some(name) => diagnostics.borrow_mut().crash(&key, &name),
                None => Err(AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", &key)),
            }
        }
        #[cfg(feature = "mod-reports")]
        "module_invoke" if args["module"] == "reports" => {
            reports.borrow_mut().handle(args["command"].as_str().unwrap_or_default(), &args["args"])
        }
        #[cfg(feature = "mod-reports")]
        "build_launch" if query_value("reports").is_some_and(|v| v.contains("alert")) => {
            let alert = launcher_shared::Alert {
                id: 7,
                title: Text::key("warning"),
                message: Text::key("version_crashed_open_logs")
                    .param("path", r"C:\Users\Player\AppData\Roaming\Launcher\games\aero\logs\latest.log"),
                allow_report: true,
            };
            // The launch goes as usual; the alert follows.
            let answer = builds.borrow_mut().handle(cmd, &args).unwrap_or(Ok(Value::Null));
            ipc::emit_mock(names::ALERT, to_value(&alert)?);
            answer
        }
        #[cfg(feature = "mod-tensa")]
        "module_invoke" if args["module"] == "tensa" => {
            tensa.borrow_mut().handle(args["command"].as_str().unwrap_or_default(), &args["args"])
        }
        #[cfg(feature = "mod-diagnostics")]
        "module_invoke" if args["module"] == "diagnostics" => {
            diagnostics.borrow_mut().handle(args["command"].as_str().unwrap_or_default(), &args["args"])
        }
        "open_path" | "open_url" => Ok(Value::Null),
        "window_control" => Ok(Value::Bool(false)),
        "app_quit" => Ok(Value::Null),
        #[cfg(feature = "mod-modrinth")]
        "held_files_found" => {
            to_value(modrinth.borrow_mut().held_found(args["files"].as_array().map_or(0, Vec::len)))
        }
        #[cfg(not(feature = "mod-modrinth"))]
        "held_files_found" => to_value(vec![false; args["files"].as_array().map_or(0, Vec::len)]),
        "log_view" => to_value(if log_mode == "big" { mock_big_log_view() } else { mock_log_view() }),
        "reveal_path" => Ok(Value::Null),
        "app_restart" => {
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
            Ok(Value::Null)
        }
        "update_status" => to_value(update.borrow().clone()),
        "update_check" => {
            let state = match update_mode.as_str() {
                "off" => UpdateState::Idle,
                "none" => UpdateState::UpToDate,
                "error" => UpdateState::Failed {
                    error: AppError::new(ErrorCode::RateLimited, "mock").with_param("minutes", "12"),
                },
                _ => UpdateState::Available { info: mock_update_info() },
            };
            {
                let mut s = update.borrow_mut();
                s.state = state;
                s.last_checked_ms = Some(js_sys::Date::now() as u64);
            }
            let snapshot = update.borrow().clone();
            ipc::emit_mock(names::UPDATE, to_value(&snapshot)?);
            to_value(snapshot)
        }
        "update_download" => {
            update.borrow_mut().state = UpdateState::Ready { info: mock_update_info() };
            let snapshot = update.borrow().clone();
            ipc::emit_mock(names::UPDATE, to_value(&snapshot)?);
            to_value(snapshot)
        }
        "update_apply" => {
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
            Ok(Value::Null)
        }
        "profiles_list" => to_value(mock_snapshot(&profiles.borrow())),
        "profile_create_offline" => {
            mock_add_offline(&mut profiles.borrow_mut(), args["name"].as_str().unwrap_or_default())?;
            emit_profiles()
        }
        "profile_delete" => {
            mock_delete(&mut profiles.borrow_mut(), args["key"].as_str().unwrap_or_default());
            emit_profiles()
        }
        "profile_set_default" => {
            let key = args["key"].as_str().unwrap_or_default().to_string();
            profiles.borrow_mut().iter_mut().for_each(|p| p.is_default = p.key == key);
            emit_profiles()
        }
        "auth_sign_in" => {
            {
                let mut list = profiles.borrow_mut();
                list.retain(|p| p.key != "Notch_UA");
                list.iter_mut().for_each(|p| p.is_default = false);
                list.push(mock_profile("Notch_UA", AccountKind::Microsoft, true));
            }
            emit_profiles()?;
            to_value(mock_profile("Notch_UA", AccountKind::Microsoft, true))
        }
        "auth_state" => to_value(if auth_mode == "device" {
            AuthState::DeviceCode {
                user_code: "ABCD-1234".into(),
                verification_uri: "https://www.microsoft.com/link".into(),
                open_url: "https://www.microsoft.com/link?otc=ABCD-1234".into(),
            }
        } else if auth_mode == "browser" {
            AuthState::Browser
        } else {
            AuthState::Idle
        }),
        "auth_cancel" => {
            ipc::emit_mock(names::AUTH, to_value(AuthState::Idle)?);
            Ok(Value::Null)
        }
        "profile_avatar" => {
            let key = args["key"].as_str().unwrap_or("Steve");
            to_value(Some(format!("https://mc-heads.net/avatar/{key}/64")))
        }
        other => builds
            .borrow_mut()
            .handle(other, &args)
            .or_else(|| components.borrow_mut().handle(other, &args))
            .or_else(|| content.borrow_mut().handle(other, &args))
            .unwrap_or_else(|| Err(AppError::internal(format!("mock: unknown command {other}")))),
    });
    ipc::set_mock(handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_update_changes_only_target_field() {
        let mut s = SettingsSnapshot::default();
        apply_update(&mut s, &SettingUpdate::CompactSidebar(true));
        apply_update(&mut s, &SettingUpdate::ClickSound(ClickSound::TypewriterSoftClick));
        assert!(s.compact_sidebar);
        assert_eq!(s.click_sound, ClickSound::TypewriterSoftClick);
        assert!(!s.auto_update);
    }

    #[test]
    fn the_big_mock_log_fills_the_viewer() {
        let view = mock_big_log_view();
        assert_eq!((view.entries.len(), view.skipped), (5000, 11000));
        let errors = view.entries.iter().filter(|e| e.level == Some(LogLevel::Error)).count();
        assert!(errors > 10 && view.entries.iter().any(|e| e.message.lines().count() > 2));
    }

    #[test]
    fn the_mock_log_has_every_level_and_a_multiline_error() {
        let view = mock_log_view();
        for level in [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
            assert!(view.entries.iter().any(|e| e.level == Some(level)), "{level:?}");
        }
        assert!(view.entries.iter().any(|e| e.level == Some(LogLevel::Error) && e.message.contains('\n')));
    }

    #[test]
    fn mock_offline_rules() {
        let mut list = vec![mock_profile("Notch_UA", AccountKind::Microsoft, true)];
        assert_eq!(mock_add_offline(&mut list, "Notch_UA").unwrap_err().code, ErrorCode::ProfileExists);
        assert_eq!(mock_add_offline(&mut list, "two words").unwrap_err().code, ErrorCode::ProfileNameInvalid);
        assert_eq!(
            mock_add_offline(&mut list, &"x".repeat(17)).unwrap_err().code,
            ErrorCode::ProfileNameInvalid
        );
        mock_add_offline(&mut list, " Steve ").unwrap();
        assert!(list.iter().find(|p| p.key == "Steve").unwrap().is_default);
        assert!(!list.iter().find(|p| p.key == "Notch_UA").unwrap().is_default);
        mock_delete(&mut list, "Steve");
        assert_eq!(list.len(), 1);
        assert!(list[0].is_default);
    }
}
