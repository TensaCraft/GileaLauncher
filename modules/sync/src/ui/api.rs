//! The module's commands (`module_invoke("sync", …)`) and the build list.

use launcher_shared::{AppError, BuildDto, BuildsSnapshot};
use serde_json::Value;
use ui_kit::ipc;

use crate::dto::{
    BuildArgs, GroupArgs, GroupView, HistoryEntry, PassReport, RestoreArgs, SaveArgs, SyncGroup, TreeArgs,
    TreeEntry,
};

pub async fn groups() -> Result<Vec<GroupView>, AppError> {
    ipc::module_invoke(crate::ID, "groups", &Value::Null).await
}

pub async fn save(group: SyncGroup) -> Result<GroupView, AppError> {
    ipc::module_invoke(crate::ID, "save", &SaveArgs { group }).await
}

pub async fn delete(id: &str) -> Result<Value, AppError> {
    ipc::module_invoke(crate::ID, "delete", &GroupArgs { id: id.into() }).await
}

pub async fn sync(id: &str) -> Result<PassReport, AppError> {
    ipc::module_invoke(crate::ID, "sync", &GroupArgs { id: id.into() }).await
}

/// Every group of build `key`: each group's name and its error, if it failed.
pub async fn sync_build(key: &str) -> Result<Vec<(String, Option<AppError>)>, AppError> {
    ipc::module_invoke(crate::ID, "sync_build", &BuildArgs { key: key.into() }).await
}

pub async fn tree(
    builds: Vec<String>,
    dir: String,
    editing: Option<String>,
) -> Result<Vec<TreeEntry>, AppError> {
    ipc::module_invoke(crate::ID, "tree", &TreeArgs { builds, dir, editing }).await
}

pub async fn history(id: &str) -> Result<Vec<HistoryEntry>, AppError> {
    ipc::module_invoke(crate::ID, "history", &GroupArgs { id: id.into() }).await
}

pub async fn restore(group: &str, id: &str) -> Result<PassReport, AppError> {
    ipc::module_invoke(crate::ID, "restore", &RestoreArgs { group: group.into(), id: id.into() }).await
}

pub async fn builds() -> Result<Vec<BuildDto>, AppError> {
    ipc::call::<BuildsSnapshot>("builds_list").await.map(|s| s.builds)
}
