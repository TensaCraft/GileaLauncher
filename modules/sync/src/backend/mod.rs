//! The sync module's backend: groups of builds whose chosen files are kept the same.

pub mod apply;
pub mod blacklist;
pub mod decide;
pub mod history;
pub mod hook;
pub mod paths;
pub mod scan;
pub mod service;
pub mod store;
pub mod watch;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use launcher_core::core_app::CoreApp;
use launcher_core::launch::hooks::{GameWatcher, LaunchHook};
use launcher_core::modules::{Module, ModuleFuture};
use launcher_core::storage::versions::Build;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dto::{BuildArgs, GroupArgs, RestoreArgs, SaveArgs, TreeArgs};
use service::{Deps, SyncService};

pub struct SyncModule {
    /// The service, made once the core is up (`attach`); the hook and the watcher read it.
    shared: Arc<OnceLock<SyncService>>,
}

const COMMANDS: [&str; 8] = ["groups", "save", "delete", "sync", "sync_build", "tree", "history", "restore"];

impl Module for SyncModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn attach(&self, core: &Arc<CoreApp>) {
        // Weak: the core keeps this module, and through it the service.
        let weak = Arc::downgrade(core);
        let running = weak.clone();
        let deps = Deps {
            versions: core.versions.clone(),
            instances: core.instances.clone(),
            feedback: core.feedback.clone(),
            config: core.config.clone(),
            backups: core.backups.clone(),
            state_dir: core.paths.app_state_dir.clone(),
            running: Arc::new(move |key: &str| running.upgrade().is_some_and(|c| c.launcher.is_running(key))),
            managed: Arc::new(move |build: &Build, game: &Path| {
                weak.upgrade().map(|c| c.modules.managed_files(build, game)).unwrap_or_default()
            }),
        };
        let _ = self.shared.set(SyncService::new(deps));
    }

    fn launch_hook(&self) -> Option<Arc<dyn LaunchHook>> {
        Some(Arc::new(hook::SyncHook(self.shared.clone())))
    }

    fn game_watcher(&self) -> Option<Arc<dyn GameWatcher>> {
        Some(Arc::new(watch::SyncWatcher(self.shared.clone())))
    }

    fn call(&self, _core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        if !COMMANDS.contains(&command) {
            return None;
        }
        let (shared, command) = (self.shared.clone(), command.to_string());
        Some(Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let service =
                    shared.get().ok_or_else(|| AppError::internal("the sync module is not attached"))?;
                dispatch(service, &command, args)
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
        }))
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> AppResult<T> {
    serde_json::from_value(args).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

fn answer<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

fn dispatch(service: &SyncService, command: &str, args: Value) -> AppResult<Value> {
    match command {
        "groups" => answer(service.groups()),
        "save" => answer(service.save(parse::<SaveArgs>(args)?.group)?),
        "delete" => answer(service.delete(&parse::<GroupArgs>(args)?.id)?),
        "sync" => answer(service.sync_group(&parse::<GroupArgs>(args)?.id, None)?),
        "sync_build" => {
            let key = parse::<BuildArgs>(args)?.key;
            let results: Vec<(String, Option<AppError>)> = service
                .sync_build(&key, None)
                .into_iter()
                .map(|(group, result)| (group, result.err()))
                .collect();
            answer(results)
        }
        "tree" => {
            let a: TreeArgs = parse(args)?;
            answer(service.tree(&a.builds, &a.dir, a.editing.as_deref())?)
        }
        "history" => answer(service.history(&parse::<GroupArgs>(args)?.id)?),
        "restore" => {
            let a: RestoreArgs = parse(args)?;
            answer(service.restore(&a.group, &a.id)?)
        }
        other => Err(AppError::new(ErrorCode::NotFound, format!("no command {other}"))),
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(SyncModule { shared: Arc::new(OnceLock::new()) })
}
