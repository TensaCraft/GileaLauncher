//! The backups module's backend: world archives, restoring them, and a backup of
//! changed worlds before every launch.

pub mod hook;
pub mod restore;
pub mod service;
pub mod settings;
pub mod store;

use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::launch::hooks::LaunchHook;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dto::{BackupArgs, BackupSettings, BuildArgs, WorldArgs};
use service::{BackupsService, Deps};

pub struct BackupsModule;

impl Module for BackupsModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        if !COMMANDS.contains(&command) {
            return None;
        }
        let (service, command) = (service_of(core), command.to_string());
        Some(Box::pin(async move { dispatch(service, &command, args).await }))
    }

    fn launch_hook(&self) -> Option<Arc<dyn LaunchHook>> {
        Some(Arc::new(hook::AutoBackup))
    }

    fn config_defaults(&self) -> Vec<(&'static str, serde_json::Value)> {
        vec![
            ("world_backups_enabled", serde_json::json!("no")),
            ("world_backups_keep_count", serde_json::json!(3)),
        ]
    }
}

const COMMANDS: [&str; 8] =
    ["settings", "set_settings", "worlds", "backups", "create", "restore", "delete", "delete_build"];

fn service_of(core: &Arc<CoreApp>) -> BackupsService {
    let launcher = core.launcher.clone();
    BackupsService::new(Deps {
        config: core.config.clone(),
        versions: core.versions.clone(),
        instances: core.instances.clone(),
        feedback: core.feedback.clone(),
        running: Arc::new(move |key: &str| launcher.is_running(key)),
    })
}

fn parse<T: DeserializeOwned>(args: Value) -> AppResult<T> {
    serde_json::from_value(args).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

fn answer<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

async fn dispatch(service: BackupsService, command: &str, args: Value) -> AppResult<Value> {
    match command {
        "settings" => answer(service.settings()),
        "set_settings" => answer(service.set_settings(parse::<BackupSettings>(args)?)?),
        "worlds" => answer(service.worlds(&parse::<BuildArgs>(args)?.key).await?),
        "backups" => {
            let a: WorldArgs = parse(args)?;
            answer(service.backups(&a.key, &a.world).await?)
        }
        "create" => {
            let a: WorldArgs = parse(args)?;
            answer(service.create(&a.key, &a.world).await?)
        }
        "restore" => {
            let a: BackupArgs = parse(args)?;
            answer(service.restore(&a.key, &a.world, &a.zip_name).await?)
        }
        "delete" => {
            let a: BackupArgs = parse(args)?;
            answer(service.delete(&a.key, &a.world, &a.zip_name).await?)
        }
        "delete_build" => answer(service.delete_build(&parse::<BuildArgs>(args)?.key).await?),
        other => Err(AppError::new(ErrorCode::NotFound, format!("no command {other}"))),
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(BackupsModule)
}
