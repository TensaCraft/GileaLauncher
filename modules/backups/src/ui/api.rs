//! The module's commands (`module_invoke("backups", …)`).

use launcher_shared::AppError;
use serde_json::Value;
use ui_kit::ipc;

use crate::dto::{BackupArgs, BackupDto, BackupSettings, BuildArgs, WorldArgs, WorldDto};

pub async fn worlds(key: &str) -> Result<Vec<WorldDto>, AppError> {
    ipc::module_invoke(crate::ID, "worlds", &BuildArgs { key: key.into() }).await
}

pub async fn backups(key: &str, world: &str) -> Result<Vec<BackupDto>, AppError> {
    ipc::module_invoke(crate::ID, "backups", &WorldArgs { key: key.into(), world: world.into() }).await
}

pub async fn create(key: &str, world: &str) -> Result<BackupDto, AppError> {
    ipc::module_invoke(crate::ID, "create", &WorldArgs { key: key.into(), world: world.into() }).await
}

pub async fn restore(key: &str, world: &str, zip_name: &str) -> Result<Value, AppError> {
    let args = BackupArgs { key: key.into(), world: world.into(), zip_name: zip_name.into() };
    ipc::module_invoke(crate::ID, "restore", &args).await
}

pub async fn delete(key: &str, world: &str, zip_name: &str) -> Result<Value, AppError> {
    let args = BackupArgs { key: key.into(), world: world.into(), zip_name: zip_name.into() };
    ipc::module_invoke(crate::ID, "delete", &args).await
}

pub async fn settings() -> Result<BackupSettings, AppError> {
    ipc::module_invoke(crate::ID, "settings", &Value::Null).await
}

pub async fn set_settings(settings: &BackupSettings) -> Result<BackupSettings, AppError> {
    ipc::module_invoke(crate::ID, "set_settings", settings).await
}
