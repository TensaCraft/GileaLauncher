//! IPC for builds, the Minecraft catalog, Java and memory. Commands that change the
//! build list announce it on `app://builds`.

use launcher_core::builds::service::build_dto;
use launcher_core::core_app::CoreApp;
use launcher_core::java::memory::MemoryLimits;
use launcher_core::java::preferences::now_secs;
use launcher_core::launch::options::game_dir;
use launcher_core::launch::service::LaunchRequest;
use launcher_core::platform::shortcuts;
use launcher_core::storage::versions::Build;
use launcher_shared::{
    AppError, AppResult, BuildDto, BuildSettingsDto, BuildSettingsUpdate, BuildsSnapshot, CatalogVersion,
    ErrorCode, JavaList, LoaderCatalog, LoaderKind, MemoryInfo, Text, names,
};
use tauri::{AppHandle, Emitter, State};

use crate::commands::AppState;

fn builds_snapshot(core: &CoreApp) -> BuildsSnapshot {
    core.builds.snapshot(|key| core.launcher.is_running(key))
}

pub(crate) fn announce_builds(app: &AppHandle, core: &CoreApp) -> BuildsSnapshot {
    let snapshot = builds_snapshot(core);
    let _ = app.emit(names::BUILDS, snapshot.clone());
    snapshot
}

fn find_build(core: &CoreApp, key: &str) -> AppResult<Build> {
    core.versions.get(key).ok_or_else(|| {
        AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
    })
}

fn dto(core: &CoreApp, build: &Build) -> BuildDto {
    build_dto(build, &core.paths.minecraft_dir, core.launcher.is_running(&build.key))
}

#[tauri::command(async)]
pub fn builds_list(state: State<'_, AppState>) -> BuildsSnapshot {
    builds_snapshot(&state.core)
}

/// Puts the builds in the order of `keys` (dragged on Home or in Builds).
#[tauri::command(async)]
pub fn builds_reorder(
    app: AppHandle,
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> AppResult<BuildsSnapshot> {
    state.core.builds.reorder(&keys)?;
    Ok(announce_builds(&app, &state.core))
}

#[tauri::command]
pub async fn build_create_vanilla(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    version: String,
) -> AppResult<BuildDto> {
    let core = state.core.clone();
    let build = core.builds.install_vanilla(&name, &version).await?;
    announce_builds(&app, &core);
    Ok(dto(&core, &build))
}

#[tauri::command]
pub async fn build_create_loader(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    loader: LoaderKind,
    mc: String,
    loader_version: String,
) -> AppResult<BuildDto> {
    let core = state.core.clone();
    let build = core.builds.install_loader(&name, loader, &mc, &loader_version).await?;
    announce_builds(&app, &core);
    Ok(dto(&core, &build))
}

#[tauri::command(async)]
pub fn build_settings_get(state: State<'_, AppState>, key: String) -> AppResult<BuildSettingsDto> {
    state.core.builds.settings(&key)
}

#[tauri::command(async)]
pub fn build_settings_save(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    update: BuildSettingsUpdate,
) -> AppResult<BuildSettingsDto> {
    let core = &state.core;
    let result = core.builds.update_settings(&key, update);
    announce_builds(&app, core);
    result
}

#[tauri::command]
pub async fn build_change_component(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    loader: LoaderKind,
    mc: String,
    loader_version: Option<String>,
) -> AppResult<BuildSettingsDto> {
    let core = state.core.clone();
    let result = core.builds.change_component(&key, loader, &mc, loader_version.as_deref()).await;
    announce_builds(&app, &core);
    result
}

#[tauri::command]
pub async fn build_copy(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    name: String,
) -> AppResult<BuildDto> {
    let core = state.core.clone();
    let build = core.builds.copy(&key, &name, || core.launcher.is_running(&key)).await?;
    announce_builds(&app, &core);
    Ok(dto(&core, &build))
}

#[tauri::command(async)]
pub fn build_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    delete_files: bool,
) -> AppResult<BuildsSnapshot> {
    let core = &state.core;
    let result = core.builds.delete(&key, delete_files, || core.launcher.is_running(&key));
    let snapshot = announce_builds(&app, core);
    result.map(|()| snapshot)
}

#[tauri::command]
pub async fn build_launch(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    profile_key: Option<String>,
    allow_duplicate: bool,
) -> AppResult<u32> {
    let core = state.core.clone();
    let started =
        core.launcher.launch(LaunchRequest { build_key: key, profile_key, allow_duplicate }).await?;
    announce_builds(&app, &core);
    Ok(started.pid)
}

#[tauri::command(async)]
pub fn build_stop(state: State<'_, AppState>, key: String) -> usize {
    state.core.launcher.terminate(&key)
}

#[tauri::command(async)]
pub fn build_open_dir(app: AppHandle, state: State<'_, AppState>, key: String) -> AppResult<()> {
    let core = &state.core;
    let dir = game_dir(&find_build(core, &key)?, &core.paths.minecraft_dir);
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string())
            .with_param("path", dir.to_string_lossy())
    })?;
    crate::commands::open_folder(&app, &dir)
}

#[tauri::command]
pub async fn build_shortcut(state: State<'_, AppState>, key: String) -> AppResult<String> {
    let core = state.core.clone();
    let build = find_build(&core, &key)?;
    // The build's own picture is the shortcut's icon (fetched when the record holds an address).
    let picture = shortcuts::build_picture(&core.meta, build.image.as_deref()).await;
    let shortcuts = core.shortcuts.clone();
    let path = tauri::async_runtime::spawn_blocking(move || {
        shortcuts.create(&build.version_id, &build.name, picture.as_deref())
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::ShortcutFailed, e.to_string()))??;
    core.feedback.success(Text::key("desktop_shortcut_created"));
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn catalog_minecraft(
    state: State<'_, AppState>,
    snapshots: bool,
) -> AppResult<Vec<CatalogVersion>> {
    let core = state.core.clone();
    core.minecraft.catalog(snapshots).await
}

#[tauri::command]
pub async fn catalog_loader(
    state: State<'_, AppState>,
    loader: LoaderKind,
    unstable: bool,
) -> AppResult<LoaderCatalog> {
    let core = state.core.clone();
    core.components.catalog(loader, unstable).await.map(LoaderCatalog::pack)
}

#[tauri::command(async)]
pub fn java_list(state: State<'_, AppState>) -> JavaList {
    state.core.java_settings.list()
}

#[tauri::command(async)]
pub fn java_add(state: State<'_, AppState>, label: String, path: String) -> AppResult<JavaList> {
    let list = state.core.java_settings.add_custom(&label, &path)?;
    state.core.feedback.success(Text::key("custom_java_added"));
    Ok(list)
}

#[tauri::command(async)]
pub fn java_remove(state: State<'_, AppState>, path: String) -> AppResult<JavaList> {
    state.core.java_settings.remove_custom(&path)
}

/// "Scan Java": every Java found on this computer joins the user's list.
#[tauri::command]
pub async fn java_scan(state: State<'_, AppState>) -> AppResult<JavaList> {
    let core = state.core.clone();
    let scanner = core.clone();
    let added =
        tauri::async_runtime::spawn_blocking(move || scanner.java_settings.import_discovered(now_secs()))
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
    if added == 0 {
        core.feedback.info(Text::key("custom_java_scan_no_new"));
    } else {
        core.feedback.success(Text::key("custom_java_scan_added").param("count", added.to_string()));
    }
    Ok(core.java_settings.list())
}

#[tauri::command(async)]
pub fn memory_info() -> MemoryInfo {
    MemoryInfo::from(MemoryLimits::detect())
}
