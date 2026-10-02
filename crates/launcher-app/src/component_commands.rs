//! IPC for the Components page.

use launcher_shared::{AppError, AppResult, ComponentsSnapshot, ErrorCode, LoaderKind, VerifyOutcome};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::commands::AppState;

#[tauri::command]
pub async fn components_list(state: State<'_, AppState>) -> AppResult<ComponentsSnapshot> {
    let core = state.core.clone();
    Ok(core.component_manager.list().await)
}

#[tauri::command]
pub async fn component_install(
    state: State<'_, AppState>,
    loader: LoaderKind,
    mc: String,
    loader_version: Option<String>,
) -> AppResult<ComponentsSnapshot> {
    let core = state.core.clone();
    core.component_manager.install(loader, &mc, loader_version.as_deref()).await?;
    Ok(core.component_manager.list().await)
}

#[tauri::command]
pub async fn component_verify(state: State<'_, AppState>, id: String) -> AppResult<VerifyOutcome> {
    let core = state.core.clone();
    core.component_manager.verify(&id).await
}

#[tauri::command]
pub async fn component_reinstall(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let core = state.core.clone();
    core.component_manager.reinstall(&id).await
}

#[tauri::command]
pub async fn component_delete(state: State<'_, AppState>, id: String) -> AppResult<ComponentsSnapshot> {
    let core = state.core.clone();
    let worker = core.clone();
    tauri::async_runtime::spawn_blocking(move || {
        worker.component_manager.delete(&id, |key| worker.launcher.is_running(key))
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))??;
    Ok(core.component_manager.list().await)
}

#[tauri::command(async)]
pub fn component_open_dir(app: AppHandle, state: State<'_, AppState>, id: String) -> AppResult<()> {
    let dir = state.core.component_manager.dir(&id)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
}
