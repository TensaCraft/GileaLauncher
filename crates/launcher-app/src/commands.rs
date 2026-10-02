//! IPC commands. Every fallible command returns `AppError`, which the UI translates.

use std::sync::{Arc, Mutex};

use launcher_core::core_app::CoreApp;
use launcher_core::platform::sound::ClickPlayer;
use launcher_shared::{
    ActivityEntry, AppError, AppInfo, AppResult, AuthState, ErrorCode, LogView, OpsSnapshot, ProfileDto,
    ProfilesSnapshot, SettingUpdate, SettingsSnapshot, SetupPlan, SetupPreview, SetupState, Text,
    UpdateStatus, WindowAction, WindowSize, names,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub core: Arc<CoreApp>,
    pub pending_launch: Mutex<Option<String>>,
    pub clicks: ClickPlayer,
}

/// Plays click sound `sound` for the interface (the web view plays no audio itself).
#[tauri::command]
pub fn play_click(state: State<'_, AppState>, sound: String) {
    state.clicks.play(launcher_shared::ClickSound::from_config_str(&sound));
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    state.core.app_info()
}

#[tauri::command]
pub fn startup_warnings(state: State<'_, AppState>) -> Vec<Text> {
    state.core.startup_warnings.clone()
}

#[tauri::command]
pub fn take_pending_launch(state: State<'_, AppState>) -> Option<String> {
    state.pending_launch.lock().ok().and_then(|mut p| p.take())
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> SettingsSnapshot {
    state.core.settings.snapshot()
}

#[tauri::command(async)]
pub fn settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    update: SettingUpdate,
) -> AppResult<SettingsSnapshot> {
    let resize = matches!(update, SettingUpdate::WindowSize(_));
    let relang = matches!(update, SettingUpdate::Lang(_));
    let snapshot = state.core.settings.apply(update)?;
    if relang {
        crate::tray::retitle(&app);
    }
    if resize
        && let (Some(window), Some(size)) =
            (app.get_webview_window("main"), WindowSize::parse(&snapshot.window_size))
    {
        crate::window::change_window_size(window, size);
    }
    let _ = app.emit(names::SETTINGS, snapshot.clone());
    Ok(snapshot)
}

#[tauri::command(async)]
pub fn settings_save_minecraft_dir(state: State<'_, AppState>, path: String) -> AppResult<bool> {
    state.core.settings.save_minecraft_dir(&path)
}

#[tauri::command(async)]
pub fn setup_state(state: State<'_, AppState>) -> AppResult<SetupState> {
    Ok(state.core.setup_state())
}

#[tauri::command(async)]
pub fn setup_preview(state: State<'_, AppState>, dir: String) -> AppResult<SetupPreview> {
    state.core.setup_preview(&dir)
}

#[tauri::command(async)]
pub fn setup_apply(state: State<'_, AppState>, plan: SetupPlan) -> AppResult<bool> {
    Ok(state.core.apply_setup(&plan)?.restart_required)
}

#[tauri::command]
pub fn ops_snapshot(state: State<'_, AppState>) -> OpsSnapshot {
    state.core.feedback.snapshot()
}

#[tauri::command]
pub fn activity_recent(state: State<'_, AppState>, limit: usize) -> Vec<ActivityEntry> {
    state.core.feedback.activity(limit.min(200))
}

#[tauri::command]
pub async fn pick_directory(app: AppHandle, start: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if let Some(dir) = start.filter(|s| !s.trim().is_empty()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    rx.await.ok().flatten().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

/// A Java executable picked in the system dialog (`*.exe` on Windows).
#[tauri::command]
pub async fn pick_java_file(app: AppHandle, start: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if let Some(dir) = start.filter(|s| !s.trim().is_empty()) {
        dialog = dialog.set_directory(dir);
    }
    if cfg!(windows) {
        dialog = dialog.add_filter("Java", &["exe"]);
    }
    dialog.pick_file(move |picked| {
        let _ = tx.send(picked);
    });
    rx.await.ok().flatten().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

/// An icon picked in the system dialog, checked to be one a build takes (`invalid_input` if not).
#[tauri::command]
pub async fn pick_image_file(app: AppHandle) -> AppResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file(
        move |picked| {
            let _ = tx.send(picked);
        },
    );
    let Some(path) = rx.await.ok().flatten().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    launcher_core::builds::settings::read_icon(&path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Only folders may be opened: a file path would be *executed* by the system shell.
pub fn openable_dir(path: &str) -> AppResult<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(path);
    if !dir.exists() {
        return Err(AppError::new(ErrorCode::NotFound, "path does not exist").with_param("path", path));
    }
    if !dir.is_dir() {
        return Err(
            AppError::new(ErrorCode::InvalidInput, "only folders can be opened").with_param("path", path)
        );
    }
    Ok(dir)
}

#[tauri::command(async)]
pub fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    open_folder(&app, &openable_dir(&path)?)
}

/// Opens folder `dir` in the system file manager (on Linux over D-Bus, see
/// `launcher_core::platform::open`), the system opener when that fails.
pub fn open_folder(app: &AppHandle, dir: &std::path::Path) -> AppResult<()> {
    launcher_core::platform::open::open_folder(dir, |dir| open_file(app, dir))
        .map_err(|e| AppError::new(ErrorCode::Io, e))
}

/// Opens a file or a folder in its program; on Linux without the AppImage's environment.
pub fn open_file(app: &AppHandle, path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        launcher_core::platform::open::open_detached(path.as_os_str())
    }
    #[cfg(not(target_os = "linux"))]
    {
        app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
    }
}

/// Opens a link in the browser; on Linux without the AppImage's environment.
pub fn open_link(app: &AppHandle, url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        launcher_core::platform::open::open_detached(url.as_ref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
    }
}

/// The launcher log for the in-app viewer.
#[tauri::command(async)]
pub fn log_view(state: State<'_, AppState>) -> AppResult<LogView> {
    launcher_core::log_view::read(&state.core.log_file())
        .map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
}

/// Shows an existing file selected in the system file manager; the file is never opened.
#[tauri::command(async)]
pub fn reveal_path(app: AppHandle, path: String) -> AppResult<()> {
    let file = revealable_file(&path)?;
    app.opener().reveal_item_in_dir(&file).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
}

/// Only an existing file given by an absolute path may be revealed.
pub fn revealable_file(path: &str) -> AppResult<std::path::PathBuf> {
    let file = std::path::PathBuf::from(path);
    if !file.is_absolute() {
        return Err(
            AppError::new(ErrorCode::InvalidInput, "the path must be absolute").with_param("path", path)
        );
    }
    if !file.exists() {
        return Err(AppError::new(ErrorCode::NotFound, "path does not exist").with_param("path", path));
    }
    if !file.is_file() {
        return Err(
            AppError::new(ErrorCode::InvalidInput, "only files can be revealed").with_param("path", path)
        );
    }
    Ok(file)
}

#[tauri::command(async)]
pub fn open_url(app: AppHandle, url: String) -> AppResult<()> {
    if !url.starts_with("https://") {
        return Err(
            AppError::new(ErrorCode::InvalidInput, "only https links can be opened").with_param("url", url)
        );
    }
    open_link(&app, &url).map_err(|e| AppError::new(ErrorCode::Io, e))
}

/// A module's command: one dispatcher instead of a Tauri plugin per module.
#[tauri::command]
pub async fn module_invoke(
    app: AppHandle,
    state: State<'_, AppState>,
    module: String,
    command: String,
    args: Value,
) -> AppResult<Value> {
    let core = state.core.clone();
    let result = core.modules.call(&core, &module, &command, args).await;
    // A module's command that made or changed a build: the lists everywhere follow.
    if result.is_ok() && core.modules.changes_builds(&module, &command) {
        crate::game_commands::announce_builds(&app, &core);
    }
    result
}

#[tauri::command]
pub fn app_restart(app: AppHandle) {
    tracing::info!("Restart requested by the UI");
    app.restart();
}

/// The launcher's own title bar: `action` on the window. Whether the window fills the screen
/// afterwards (the maximize button's icon).
#[tauri::command]
pub fn window_control(window: WebviewWindow, action: WindowAction) -> bool {
    let done = match action {
        WindowAction::Look => Ok(()),
        WindowAction::Minimize => window.minimize(),
        WindowAction::Maximize => return crate::window::toggle_maximized(&window),
        WindowAction::Close => window.close(),
        WindowAction::Tray => {
            crate::tray::hide(window.app_handle());
            Ok(())
        }
        WindowAction::Drag => window.start_dragging(),
    };
    if let Err(e) = done {
        tracing::warn!("The launcher window cannot do {action:?}: {e}");
    }
    window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false)
}

#[tauri::command]
pub fn update_status(state: State<'_, AppState>) -> UpdateStatus {
    state.core.updater.status()
}

#[tauri::command]
pub async fn update_check(state: State<'_, AppState>, manual: bool) -> AppResult<UpdateStatus> {
    let include_beta = state.core.settings.snapshot().include_beta_updates;
    Ok(state.core.updater.check(include_beta, manual).await)
}

#[tauri::command]
pub async fn update_download(state: State<'_, AppState>) -> AppResult<UpdateStatus> {
    Ok(state.core.updater.download_and_prepare().await)
}

/// Starts the update helper and exits so it can replace the program. Hashes the staged files,
/// so it runs off the main thread.
#[tauri::command(async)]
pub fn update_apply(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    state.core.updater.apply()?;
    tracing::info!("Launcher update helper started; exiting");
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn profiles_list(state: State<'_, AppState>) -> ProfilesSnapshot {
    state.core.auth.snapshot()
}

#[tauri::command(async)]
pub fn profile_create_offline(state: State<'_, AppState>, name: String) -> AppResult<ProfilesSnapshot> {
    state.core.auth.create_offline(&name)
}

#[tauri::command(async)]
pub fn profile_delete(state: State<'_, AppState>, key: String) -> AppResult<ProfilesSnapshot> {
    state.core.auth.delete(&key)
}

#[tauri::command(async)]
pub fn profile_set_default(state: State<'_, AppState>, key: String) -> AppResult<ProfilesSnapshot> {
    state.core.auth.set_default(&key)
}

#[tauri::command]
pub fn auth_state(state: State<'_, AppState>) -> AuthState {
    state.core.auth.auth_state()
}

/// Microsoft sign-in; progress arrives as `app://auth` events (device code dialog).
#[tauri::command]
pub async fn auth_sign_in(state: State<'_, AppState>) -> AppResult<ProfileDto> {
    let lang = state.core.settings.lang();
    state.core.auth.sign_in_microsoft(&lang).await
}

#[tauri::command]
pub fn auth_cancel(state: State<'_, AppState>) {
    state.core.auth.cancel_sign_in();
}

/// `data:` URL of the cached head, the remote address as a last resort, or nothing.
#[tauri::command]
pub async fn profile_avatar(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    Ok(state.core.auth.avatar(&key).await.map(|avatar| avatar.to_src()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openable_dir_accepts_only_existing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run-me.bat");
        std::fs::write(&file, "echo").unwrap();
        assert!(openable_dir(dir.path().to_str().unwrap()).is_ok());
        assert_eq!(openable_dir(file.to_str().unwrap()).unwrap_err().code, ErrorCode::InvalidInput);
        let missing = dir.path().join("missing");
        assert_eq!(openable_dir(missing.to_str().unwrap()).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn revealable_file_accepts_only_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.log");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(revealable_file(file.to_str().unwrap()).unwrap(), file);
        assert_eq!(revealable_file(dir.path().to_str().unwrap()).unwrap_err().code, ErrorCode::InvalidInput);
        let missing = dir.path().join("gone.log");
        assert_eq!(revealable_file(missing.to_str().unwrap()).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(revealable_file("app.log").unwrap_err().code, ErrorCode::InvalidInput, "relative");
    }
}
