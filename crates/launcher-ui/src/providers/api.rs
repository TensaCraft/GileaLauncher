//! The provider commands (`provider_*` in `launcher-app`): each names its provider.

use launcher_shared::AppError;
use launcher_shared::provider::{
    HeldFile, InstallAnswer, InstallArgs, ModpackBuild, Overview, OverviewArgs, PackArgs, PackInstallArgs,
    PackInstalled, PackUpdateArgs, PackUpdated, PackVersion, PacksArgs, PlanDto, SearchArgs, SearchPage,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ui_kit::ipc;

#[derive(Serialize)]
struct Call<'a, A: Serialize> {
    provider: &'a str,
    args: &'a A,
}

async fn call<A: Serialize, R: DeserializeOwned>(
    command: &str,
    provider: &str,
    args: &A,
) -> Result<R, AppError> {
    ipc::invoke(command, &Call { provider, args }).await
}

pub async fn search(provider: &str, args: &SearchArgs) -> Result<SearchPage, AppError> {
    call("provider_search", provider, args).await
}

pub async fn plan(provider: &str, args: &InstallArgs) -> Result<PlanDto, AppError> {
    call("provider_plan", provider, args).await
}

pub async fn install(provider: &str, args: &InstallArgs) -> Result<InstallAnswer, AppError> {
    call("provider_install", provider, args).await
}

pub async fn overview(provider: &str, args: &OverviewArgs) -> Result<Overview, AppError> {
    call("provider_overview", provider, args).await
}

pub async fn modpacks(provider: &str, args: &PacksArgs) -> Result<SearchPage, AppError> {
    call("provider_modpacks", provider, args).await
}

pub async fn modpack_versions(provider: &str, args: &PackArgs) -> Result<Vec<PackVersion>, AppError> {
    call("provider_modpack_versions", provider, args).await
}

pub async fn install_modpack(provider: &str, args: &PackInstallArgs) -> Result<PackInstalled, AppError> {
    call("provider_install_modpack", provider, args).await
}

#[derive(Serialize)]
struct Provider<'a> {
    provider: &'a str,
}

pub async fn modpack_builds(provider: &str) -> Result<Vec<ModpackBuild>, AppError> {
    ipc::invoke("provider_modpack_builds", &Provider { provider }).await
}

pub async fn update_modpack(provider: &str, args: &PackUpdateArgs) -> Result<PackUpdated, AppError> {
    call("provider_update_modpack", provider, args).await
}

#[derive(Serialize)]
struct Held<'a> {
    files: &'a [HeldFile],
}

/// Which of `files` are in the user's Downloads folder now.
pub async fn held_found(files: &[HeldFile]) -> Result<Vec<bool>, AppError> {
    ipc::invoke("held_files_found", &Held { files }).await
}
