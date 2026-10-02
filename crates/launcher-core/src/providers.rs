//! The content-provider contract (`ContentProvider`, modpacks included): a
//! module that is a provider says what it offers (`Module::provider_info`) and answers only that;
//! the app asks for nothing else (`ModuleRegistry::provider_for`). Every method is optional — the
//! defaults answer `Unsupported`.

use std::future::Future;
use std::pin::Pin;

use launcher_shared::provider::{
    InstallAnswer, InstallArgs, ModpackBuild, Need, Overview, OverviewArgs, PackArgs, PackInstallArgs,
    PackInstalled, PackUpdateArgs, PackUpdated, PackVersion, PacksArgs, PlanDto, ProviderInfo, SearchArgs,
    SearchPage,
};
use launcher_shared::{AppError, AppResult, ErrorCode};

/// A provider's answer.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// What a provider answers for what it does not offer.
pub fn unsupported<'a, T: 'a>(what: &'static str) -> ProviderFuture<'a, T> {
    let error = AppError::new(ErrorCode::Unsupported, format!("the provider does not offer {what}"));
    Box::pin(async move { Err(error) })
}

/// `Ok` when `info` offers `need`; otherwise `Unsupported`, naming the provider.
pub fn require(info: &ProviderInfo, need: Need) -> AppResult<()> {
    if info.offers(need) {
        return Ok(());
    }
    Err(AppError::new(ErrorCode::Unsupported, format!("{} does not offer {need:?}", info.id))
        .with_param("provider", &info.name))
}

pub trait ContentProvider: Send + Sync + 'static {
    /// A page of a kind of content for a build (its version and loader narrow it).
    fn search(&self, args: SearchArgs) -> ProviderFuture<'_, SearchPage> {
        let _ = args;
        unsupported("search")
    }

    /// What installing a project takes: its dependencies, replacements, what blocks it.
    fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
        let _ = args;
        unsupported("plan")
    }

    /// Installs an approved plan, or answers with the plan as it is now.
    fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
        let _ = args;
        unsupported("install")
    }

    /// Which of a build's files are the provider's, and (when asked) their newer versions.
    fn overview(&self, args: OverviewArgs) -> ProviderFuture<'_, Overview> {
        let _ = args;
        unsupported("overview")
    }

    /// A page of modpacks.
    fn modpacks(&self, args: PacksArgs) -> ProviderFuture<'_, SearchPage> {
        let _ = args;
        unsupported("modpacks")
    }

    /// A modpack's versions, newest first.
    fn modpack_versions(&self, args: PackArgs) -> ProviderFuture<'_, Vec<PackVersion>> {
        let _ = args;
        unsupported("modpack versions")
    }

    /// A new build from a modpack version.
    fn install_modpack(&self, args: PackInstallArgs) -> ProviderFuture<'_, PackInstalled> {
        let _ = args;
        unsupported("modpack installs")
    }

    /// The builds it installed from modpacks, each with its newest version when that is newer.
    fn modpack_builds(&self) -> ProviderFuture<'_, Vec<ModpackBuild>> {
        unsupported("modpack builds")
    }

    /// Updates a build installed from a modpack to another version of it.
    fn update_modpack(&self, args: PackUpdateArgs) -> ProviderFuture<'_, PackUpdated> {
        let _ = args;
        unsupported("modpack updates")
    }
}
