//! IPC for a build's content.

use std::path::Path;

use launcher_core::content::screenshots::ScreenshotFile;
use launcher_core::content::screenshots::rgba;
use launcher_shared::{
    AppError, AppResult, BuildShots, ContentKind, ContentList, ErrorCode, ScreenshotDto, ShotRef,
};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::commands::{AppState, blocking};
use crate::screenshot_protocol::{screenshot_url, thumbnail_url};

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
            thumb: thumbnail_url(key, &f.name, f.modified_ms),
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

/// Every build's screenshots (the Screenshots page), builds with none left out.
#[tauri::command]
pub async fn screenshots_all(state: State<'_, AppState>) -> AppResult<Vec<BuildShots>> {
    let core = state.core.clone();
    blocking(move || {
        Ok(core
            .content
            .all_screenshots()
            .into_iter()
            .map(|(key, files)| BuildShots { shots: shots(&key, files), key })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn screenshot_rename(
    state: State<'_, AppState>,
    key: String,
    name: String,
    new_name: String,
) -> AppResult<ScreenshotDto> {
    let core = state.core.clone();
    blocking(move || {
        let renamed = core.content.rename_screenshot(&key, &name, &new_name)?;
        shots(&key, vec![renamed]).pop().ok_or_else(|| AppError::internal("no screenshot"))
    })
    .await
}

/// Deletes several screenshots; how many went.
#[tauri::command]
pub async fn screenshots_delete(state: State<'_, AppState>, items: Vec<ShotRef>) -> AppResult<usize> {
    let core = state.core.clone();
    let items: Vec<(String, String)> = items.into_iter().map(|i| (i.key, i.name)).collect();
    blocking(move || Ok(core.content.delete_screenshots(&items))).await
}

/// The one clipboard of the launcher: on Linux the copied picture lives as long as it does.
static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);

/// Puts screenshot `name` of build `key` on the clipboard as a picture.
#[tauri::command]
pub async fn screenshot_copy(state: State<'_, AppState>, key: String, name: String) -> AppResult<()> {
    let core = state.core.clone();
    blocking(move || {
        let (width, height, bytes) = rgba(&core.content.screenshot(&key, &name)?)?;
        let failed = |e: arboard::Error| AppError::new(ErrorCode::Io, format!("clipboard: {e}"));
        let mut clipboard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(failed)?);
        }
        let image =
            arboard::ImageData { width: width as usize, height: height as usize, bytes: bytes.into() };
        clipboard.as_mut().map_or(Ok(()), |c| c.set_image(image).map_err(failed))?;
        core.feedback.success(launcher_shared::Text::key("screenshot_copied"));
        Ok(())
    })
    .await
}

/// Shows screenshot `name` of build `key` in its folder.
#[tauri::command(async)]
pub fn screenshot_reveal(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    name: String,
) -> AppResult<()> {
    let shot = state.core.content.screenshot(&key, &name)?;
    app.opener().reveal_item_in_dir(&shot.path).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
}
