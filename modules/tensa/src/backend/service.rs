//! The services the module works with: the server's API, the launcher's builds, locks, feedback,
//! downloader and component installer (real ones in the app, fakes in the tests).

use std::sync::Arc;

use launcher_core::feedback::{FeedbackService, OperationSpec};
use launcher_core::launch::options::game_dir;
use launcher_core::loaders::ComponentSource;
use launcher_core::lock::Coordinator;
use launcher_core::net::downloader::Downloader;
use launcher_core::storage::versions::VersionStore;
use launcher_shared::{AppError, AppResult, ErrorCode, Text};

use super::api::TensaApi;
use super::sync::{SyncDeps, Synced, sync};

/// The lock a forced sync holds on the build's folder.
const LEASE: &str = "tensacraft_sync";

#[derive(Clone)]
pub struct Deps {
    pub api: Arc<TensaApi>,
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub downloader: Arc<Downloader>,
    pub components: Arc<dyn ComponentSource>,
    /// The graphics card a new build starts with (the launcher's setting).
    pub gpu_mode: Arc<dyn Fn() -> &'static str + Send + Sync>,
    /// Build `key`'s game is running.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

/// Syncs build `key` with the server now, downloading every managed file again ("Force sync"):
/// not while its game runs.
pub async fn force_sync(deps: &Deps, key: &str) -> AppResult<Synced> {
    let build = deps.versions.get(key).ok_or_else(|| {
        AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
    })?;
    if (deps.running)(&build.key) {
        return Err(AppError::new(ErrorCode::GameRunning, format!("{} is running", build.name))
            .with_param("version", &build.name));
    }
    let game = game_dir(&build, deps.versions.minecraft_dir());
    if !super::identity::is_managed(&build, &game) {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("{} is no server build", build.name))
            .with_param("version", &build.name));
    }
    let _lease = deps.instances.try_acquire(&game, LEASE)?;
    let op = deps.feedback.begin(
        OperationSpec::new(Text::key("tensacraft_force_sync_of").param("version", &build.name), "sync")
            .status(Text::key("tensacraft_force_sync_running")),
    );
    let sync_deps = SyncDeps {
        api: &deps.api,
        versions: &deps.versions,
        components: deps.components.as_ref(),
        downloader: &deps.downloader,
        running: false,
        ask_within: None,
    };
    match sync(&sync_deps, &build, true, &op).await {
        Ok(done) => {
            op.finish();
            deps.feedback.success(Text::key("tensacraft_force_sync_complete").param("version", &build.name));
            Ok(done)
        }
        Err(e) => {
            op.fail(
                Text::key("tensacraft_force_sync_failed")
                    .param("version", &build.name)
                    .param("error", &e.detail),
            );
            Err(e)
        }
    }
}
