//! The module's commands (`module_invoke("reports", …)`).

use launcher_shared::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ui_kit::ipc;
use ui_kit::problem::ProblemDraft;

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

/// The launcher's own crash waiting to be reported.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Crash {
    pub version: String,
    pub at: String,
    pub message: String,
    pub location: String,
}

pub async fn send_problem(
    message: &str,
    contact: &str,
    error: Option<&ProblemDraft>,
) -> Result<Sent, AppError> {
    let args = json!({"message": message, "contact": contact, "error": error});
    ipc::module_invoke(crate::ID, "send_problem", &args).await
}

pub async fn last_crash() -> Result<Option<Crash>, AppError> {
    ipc::module_invoke(crate::ID, "last_crash", &Value::Null).await
}

pub async fn send_crash(message: &str, contact: &str) -> Result<Sent, AppError> {
    ipc::module_invoke(crate::ID, "send_crash", &json!({"message": message, "contact": contact})).await
}

pub async fn dismiss_crash() -> Result<Value, AppError> {
    ipc::module_invoke(crate::ID, "dismiss_crash", &Value::Null).await
}

pub async fn settings() -> Result<Settings, AppError> {
    ipc::module_invoke(crate::ID, "settings", &Value::Null).await
}

pub async fn set_settings(settings: &Settings) -> Result<Value, AppError> {
    ipc::module_invoke(crate::ID, "set_settings", settings).await
}
