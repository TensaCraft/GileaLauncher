//! IPC for the «Сховище» settings tab. Measuring and clearing walk folders: they run off the
//! async runtime.

use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_shared::{AppError, AppResult, StorageLine, StorageRule, StorageSnapshot};
use tauri::State;

use crate::commands::AppState;

async fn blocking(
    state: State<'_, AppState>,
    work: impl FnOnce(&CoreApp) -> AppResult<StorageSnapshot> + Send + 'static,
) -> AppResult<StorageSnapshot> {
    let core: Arc<CoreApp> = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || work(&core))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
}

#[tauri::command]
pub async fn storage_snapshot(state: State<'_, AppState>, refresh: bool) -> AppResult<StorageSnapshot> {
    blocking(state, move |core| Ok(core.storage.snapshot(refresh))).await
}

#[tauri::command]
pub async fn storage_clear(
    state: State<'_, AppState>,
    key: Option<String>,
    line: StorageLine,
) -> AppResult<StorageSnapshot> {
    blocking(state, move |core| core.storage.clear(key.as_deref(), line)).await
}

#[tauri::command]
pub async fn storage_clear_unneeded(state: State<'_, AppState>) -> AppResult<StorageSnapshot> {
    blocking(state, |core| core.storage.clear_unneeded()).await
}

#[tauri::command]
pub async fn storage_set_rule(
    state: State<'_, AppState>,
    rule: StorageRule,
    on: bool,
) -> AppResult<StorageSnapshot> {
    blocking(state, move |core| core.storage.set_rule(rule, on)).await
}
