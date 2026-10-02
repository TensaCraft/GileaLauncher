//! Server builds on the Modpacks page (the provider contract, modpacks only): the catalog as a
//! page of modpacks, one version for each — the one the server has now — and installing it as a
//! new build. What makes them server builds (the sync before every launch) is the launch step's.

use std::sync::Arc;

use launcher_core::providers::{ContentProvider, ProviderFuture};
use launcher_shared::provider::{
    PACKS_LIMIT, PackArgs, PackInstallArgs, PackInstalled, PackVersion, PacksArgs, ProjectHit, ProviderInfo,
    SearchPage,
};
use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};
use serde_json::Value;

use super::install::install;
use super::pack::{Pack, find};
use super::service::Deps;

/// The name the server builds go by; their icon the original's.
pub fn info() -> ProviderInfo {
    ProviderInfo {
        id: crate::ID.into(),
        name: "TensaCraft".into(),
        icon: "rocket_launch".into(),
        content: Vec::new(),
        updates: Vec::new(),
        modpacks: true,
        modpack_updates: false,
    }
}

/// "Fabric" for `fabric`, the name as written for a loader the launcher does not know.
fn loader_name(pack: &Pack) -> String {
    let loader = pack.loader.as_deref().unwrap_or_default();
    LoaderKind::from_client(loader).map_or_else(|| loader.to_string(), |kind| kind.display_name().to_string())
}

/// What a server build runs, as a card's second line: "Fabric 26.3".
fn runs(pack: &Pack) -> String {
    [loader_name(pack), pack.minecraft.clone().unwrap_or_default()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A page of the catalog's server builds whose name, id or description holds `query`.
pub fn page(packs: &[Value], args: &PacksArgs) -> SearchPage {
    let query = args.query.trim().to_lowercase();
    let found: Vec<Pack> = packs
        .iter()
        .filter_map(Pack::from_value)
        .filter(|pack| {
            query.is_empty()
                || [Some(&pack.name), Some(&pack.id), pack.description.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|text| text.to_lowercase().contains(&query))
        })
        .collect();
    let hits = found
        .iter()
        .skip(args.offset as usize)
        .take(PACKS_LIMIT as usize)
        .map(|pack| ProjectHit {
            project_id: pack.id.clone(),
            slug: pack.id.clone(),
            title: pack.name.clone(),
            author: runs(pack),
            description: pack.description.clone().unwrap_or_default(),
            downloads: 0,
            icon_url: pack.image.clone(),
            url: None,
        })
        .collect();
    SearchPage { hits, total: found.len() as u32, offset: args.offset, limit: PACKS_LIMIT }
}

/// The one version of server build `project_id`: the one the server has now.
pub fn versions(packs: &[Value], project_id: &str) -> AppResult<Vec<PackVersion>> {
    let pack = find(packs, project_id).ok_or_else(|| {
        AppError::new(ErrorCode::NotFound, format!("no server build {project_id}"))
            .with_param("pack", project_id)
    })?;
    let version_number = [Some(loader_name(&pack)), pack.loader_version.clone()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(vec![PackVersion {
        id: pack.id.clone(),
        version_number,
        game_versions: pack.minecraft.iter().cloned().collect(),
        loaders: pack.loader.iter().cloned().collect(),
    }])
}

pub struct ServerBuilds {
    deps: Arc<Deps>,
}

impl ServerBuilds {
    pub fn new(deps: Arc<Deps>) -> ServerBuilds {
        ServerBuilds { deps }
    }
}

impl ContentProvider for ServerBuilds {
    fn modpacks(&self, args: PacksArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { Ok(page(&self.deps.api.packs().await?, &args)) })
    }

    fn modpack_versions(&self, args: PackArgs) -> ProviderFuture<'_, Vec<PackVersion>> {
        Box::pin(async move { versions(&self.deps.api.packs().await?, &args.project_id) })
    }

    fn install_modpack(&self, args: PackInstallArgs) -> ProviderFuture<'_, PackInstalled> {
        Box::pin(async move {
            let build = install(&self.deps, &args.project_id, &args.name).await?;
            Ok(PackInstalled { key: build.key, name: build.name })
        })
    }
}
