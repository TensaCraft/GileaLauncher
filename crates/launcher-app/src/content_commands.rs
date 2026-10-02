//! IPC for a build's content.

use std::path::Path;

use launcher_core::content::screenshots::ScreenshotFile;
use launcher_shared::{AppError, AppResult, ContentKind, ContentList, ErrorCode, ScreenshotDto};
use tauri::{AppHandle, State};

use crate::commands::AppState;
use crate::screenshot_protocol::screenshot_url;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

#[tauri::command]
pub async fn content_list(
    state: State<'_, AppState>,
    key: String,
    kind: ContentKind,
) -> AppResult<ContentList> {
    let core = state.core.clone();
    blocking(move || core.content.list(&key, kind)).await
}

#[tauri::command]
pub async fn content_toggle(
    state: State<'_, AppState>,
    key: String,
    kind: ContentKind,
    file: String,
    enable: bool,
) -> AppResult<ContentList> {
    let core = state.core.clone();
    blocking(move || core.content.toggle(&key, kind, &file, enable)).await
}

#[tauri::command]
pub async fn content_delete(
    state: State<'_, AppState>,
    key: String,
    kind: ContentKind,
    file: String,
) -> AppResult<ContentList> {
    let core = state.core.clone();
    blocking(move || core.content.delete(&key, kind, &file)).await
}

#[tauri::command]
pub async fn content_restore(
    state: State<'_, AppState>,
    key: String,
    kind: ContentKind,
    file: String,
) -> AppResult<ContentList> {
    let core = state.core.clone();
    blocking(move || core.content.restore(&key, kind, &file)).await
}

#[tauri::command(async)]
pub fn content_open_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    kind: ContentKind,
) -> AppResult<()> {
    crate::commands::open_folder(&app, &state.core.content.dir(&key, kind)?)
}

/// Opens a file (a screenshot) in its program.
fn open(app: &AppHandle, path: &Path) -> AppResult<()> {
    crate::commands::open_file(app, path).map_err(|e| AppError::new(ErrorCode::Io, e))
}

fn shots(key: &str, files: Vec<ScreenshotFile>) -> Vec<ScreenshotDto> {
    files
        .into_iter()
        .map(|f| ScreenshotDto {
            src: screenshot_url(key, &f.name, f.modified_ms),
            name: f.name,
            size: f.size,
            modified_ms: f.modified_ms,
        })
        .collect()
}

#[tauri::command]
pub async fn screenshots_list(state: State<'_, AppState>, key: String) -> AppResult<Vec<ScreenshotDto>> {
    let core = state.core.clone();
    blocking(move || core.content.screenshots(&key).map(|files| shots(&key, files))).await
}

#[tauri::command]
pub async fn screenshot_delete(
    state: State<'_, AppState>,
    key: String,
    name: String,
) -> AppResult<Vec<ScreenshotDto>> {
    let core = state.core.clone();
    blocking(move || core.content.delete_screenshot(&key, &name).map(|files| shots(&key, files))).await
}

#[tauri::command(async)]
pub fn screenshot_open(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    name: String,
) -> AppResult<()> {
    let shot = state.core.content.screenshot(&key, &name)?;
    open(&app, &shot.path)
}

#[tauri::command(async)]
pub fn screenshots_open_dir(app: AppHandle, state: State<'_, AppState>, key: String) -> AppResult<()> {
    crate::commands::open_folder(&app, &state.core.content.screenshots_dir(&key)?)
}
