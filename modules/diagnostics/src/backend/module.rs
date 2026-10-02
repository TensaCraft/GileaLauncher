//! The module as the launcher sees it: its eye on every started game and its command.

use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::launch::hooks::GameWatcher;
use launcher_core::launch::options::game_dir;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_core::storage::versions::VersionStore;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::Value;

use super::of_build;
use super::watch::CrashWatcher;
use crate::dto::{BUILD_DIAGNOSTICS, BuildArgs, BuildDiagnostics};

pub struct DiagnosticsModule;

impl Module for DiagnosticsModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn game_watcher(&self) -> Option<Arc<dyn GameWatcher>> {
        Some(Arc::new(CrashWatcher))
    }

    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        if command != BUILD_DIAGNOSTICS {
            return None;
        }
        let versions = core.versions.clone();
        Some(Box::pin(async move {
            let args: BuildArgs = serde_json::from_value(args)
                .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
            serde_json::to_value(build_diagnostics(&versions, &args.key)?)
                .map_err(|e| AppError::internal(e.to_string()))
        }))
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(DiagnosticsModule)
}

/// Where build `key`'s folders and logs are, and its last crash's diagnosis.
pub fn build_diagnostics(versions: &VersionStore, key: &str) -> AppResult<BuildDiagnostics> {
    let build = versions.get(key).ok_or_else(|| {
        AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
    })?;
    Ok(of_build(&game_dir(&build, versions.minecraft_dir())))
}
