//! IPC for content providers (`ContentProvider`): each command names its provider, and
//! the provider is asked only for what its `ProviderInfo` offers.

use launcher_core::content::held::{Wanted, downloads_dir, find_copies};
use launcher_shared::provider::{
    HeldFile, InstallAnswer, InstallArgs, ModpackBuild, Need, Overview, OverviewArgs, PackArgs,
    PackInstallArgs, PackInstalled, PackUpdateArgs, PackUpdated, PackVersion, PacksArgs, PlanDto, SearchArgs,
    SearchPage,
};
use launcher_shared::{AppError, AppResult};
use tauri::{AppHandle, State};

use crate::commands::AppState;

#[tauri::command]
pub async fn provider_search(
    state: State<'_, AppState>,
    provider: String,
    args: SearchArgs,
) -> AppResult<SearchPage> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::Content(args.kind))?.search(args).await
}

#[tauri::command]
pub async fn provider_plan(
    state: State<'_, AppState>,
    provider: String,
    args: InstallArgs,
) -> AppResult<PlanDto> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::Content(args.kind))?.plan(args).await
}

#[tauri::command]
pub async fn provider_install(
    state: State<'_, AppState>,
    provider: String,
    args: InstallArgs,
) -> AppResult<InstallAnswer> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::Content(args.kind))?.install(args).await
}

/// Which files are the provider's; checking their updates needs that offer too.
#[tauri::command]
pub async fn provider_overview(
    state: State<'_, AppState>,
    provider: String,
    args: OverviewArgs,
) -> AppResult<Overview> {
    let core = state.core.clone();
    if args.check_updates {
        core.modules.provider_info(&provider, Need::Updates(args.kind))?;
    }
    core.modules.provider_for(&core, &provider, Need::Content(args.kind))?.overview(args).await
}

#[tauri::command]
pub async fn provider_modpacks(
    state: State<'_, AppState>,
    provider: String,
    args: PacksArgs,
) -> AppResult<SearchPage> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::Modpacks)?.modpacks(args).await
}

#[tauri::command]
pub async fn provider_modpack_versions(
    state: State<'_, AppState>,
    provider: String,
    args: PackArgs,
) -> AppResult<Vec<PackVersion>> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::Modpacks)?.modpack_versions(args).await
}

#[tauri::command]
pub async fn provider_modpack_builds(
    state: State<'_, AppState>,
    provider: String,
) -> AppResult<Vec<ModpackBuild>> {
    let core = state.core.clone();
    core.modules.provider_for(&core, &provider, Need::ModpackUpdates)?.modpack_builds().await
}

/// A changed build: the lists everywhere follow.
#[tauri::command]
pub async fn provider_update_modpack(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: String,
    args: PackUpdateArgs,
) -> AppResult<PackUpdated> {
    let core = state.core.clone();
    let updated =
        core.modules.provider_for(&core, &provider, Need::ModpackUpdates)?.update_modpack(args).await?;
    crate::game_commands::announce_builds(&app, &core);
    Ok(updated)
}

/// A new build: the lists everywhere follow.
#[tauri::command]
pub async fn provider_install_modpack(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: String,
    args: PackInstallArgs,
) -> AppResult<PackInstalled> {
    let core = state.core.clone();
    let installed =
        core.modules.provider_for(&core, &provider, Need::Modpacks)?.install_modpack(args).await?;
    crate::game_commands::announce_builds(&app, &core);
    Ok(installed)
}

/// The folder the launcher looks in for files downloaded by hand (the user's Downloads).
#[tauri::command]
pub fn downloads_folder() -> Option<String> {
    downloads_dir().map(|dir| dir.to_string_lossy().into_owned())
}

/// Which of `files` — downloaded by hand from their pages — are in the user's Downloads folder
/// now (by size and SHA-1, whatever their names).
#[tauri::command]
pub async fn held_files_found(files: Vec<HeldFile>) -> AppResult<Vec<bool>> {
    let Some(dir) = downloads_dir() else { return Ok(vec![false; files.len()]) };
    tauri::async_runtime::spawn_blocking(move || {
        let wanted: Vec<Wanted<'_>> =
            files.iter().map(|f| Wanted { name: &f.file_name, size: f.size, sha1: &f.sha1 }).collect();
        find_copies(&dir, &wanted).into_iter().map(|copy| copy.is_some()).collect()
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))
}
