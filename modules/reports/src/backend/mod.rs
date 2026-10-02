//! The reports module's backend: a report as the original sends it.

pub mod log;
pub mod payload;
pub mod redact;
pub mod send;
pub mod service;

use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_shared::{AppError, ErrorCode};
use serde_json::{Value, json};

use service::Reports;

/// `{enabled}`: reports have somewhere to go.
pub const STATUS: &str = "status";
/// `{id, title, message}` → `{report_id}`: reports an alert as the user saw it.
pub const SEND_ALERT: &str = "send_alert";
/// `{key}` → `[name]`: the files a report of the build attaches.
pub const ATTACHMENTS: &str = "attachments";
/// `{key, message, contact}` → `{report_id}`: reports a build.
pub const SEND_BUILD: &str = "send_build";
/// `{contact}`; `set_settings` takes the same.
pub const SETTINGS: &str = "settings";
pub const SET_SETTINGS: &str = "set_settings";

fn text(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn key_arg(args: &Value) -> Result<String, AppError> {
    Some(text(args, "key").trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, "the command needs the build's `key`"))
}

pub struct ReportsModule;

impl Module for ReportsModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn config_defaults(&self) -> Vec<(&'static str, serde_json::Value)> {
        Vec::new()
    }

    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        let reports = Reports::of(core);
        let future: ModuleFuture = match command {
            STATUS => Box::pin(async move {
                let reports = reports?;
                // Where reports go shows in the log before any is sent (a debug build may point
                // them at a local server).
                tracing::info!(
                    "Reports go to {}",
                    if reports.enabled() { reports.endpoint.as_str() } else { "nowhere" }
                );
                Ok(json!({"enabled": reports.enabled()}))
            }),
            SEND_ALERT => Box::pin(async move {
                let id = args.get("id").and_then(Value::as_u64).ok_or_else(|| {
                    AppError::new(ErrorCode::InvalidInput, "send_alert needs the alert's `id`")
                })?;
                let report_id =
                    reports?.send_alert(id, &text(&args, "title"), &text(&args, "message")).await?;
                Ok(json!({"report_id": report_id}))
            }),
            ATTACHMENTS => Box::pin(async move { Ok(json!(reports?.attachments(&key_arg(&args)?)?)) }),
            SEND_BUILD => Box::pin(async move {
                let key = key_arg(&args)?;
                let report_id =
                    reports?.send_build(&key, &text(&args, "message"), &text(&args, "contact")).await?;
                Ok(json!({"report_id": report_id}))
            }),
            SETTINGS => Box::pin(async move { Ok(json!({"contact": reports?.contact()})) }),
            SET_SETTINGS => Box::pin(async move {
                reports?.set_contact(&text(&args, "contact"))?;
                Ok(Value::Null)
            }),
            _ => return None,
        };
        Some(future)
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(ReportsModule)
}
