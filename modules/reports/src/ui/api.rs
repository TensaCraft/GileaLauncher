//! The module's commands (`module_invoke("reports", …)`).

use launcher_shared::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ui_kit::ipc;

#[derive(Debug, Clone, Deserialize)]
pub struct Status {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sent {
    pub report_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub contact: String,
}

pub async fn status() -> Result<Status, AppError> {
    ipc::module_invoke(crate::ID, "status", &Value::Null).await
}

pub async fn send_alert(id: u64, title: &str, message: &str) -> Result<Sent, AppError> {
    ipc::module_invoke(crate::ID, "send_alert", &json!({"id": id, "title": title, "message": message})).await
}

pub async fn attachments(key: &str) -> Result<Vec<String>, AppError> {
    ipc::module_invoke(crate::ID, "attachments", &json!({"key": key})).await
}

pub async fn send_build(key: &str, message: &str, contact: &str) -> Result<Sent, AppError> {
    ipc::module_invoke(crate::ID, "send_build", &json!({"key": key, "message": message, "contact": contact}))
        .await
}

pub async fn settings() -> Result<Settings, AppError> {
    ipc::module_invoke(crate::ID, "settings", &Value::Null).await
}

pub async fn set_settings(settings: &Settings) -> Result<Value, AppError> {
    ipc::module_invoke(crate::ID, "set_settings", settings).await
}
