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
/// `{message, contact, error?: {title, code?, detail?}}` → `{report_id}`: reports a problem with
/// the launcher.
pub const SEND_PROBLEM: &str = "send_problem";
/// → the launcher's own crash waiting to be reported (`{version, at, message, location, thread}`)
/// or null.
pub const LAST_CRASH: &str = "last_crash";
/// `{message, contact}` → `{report_id}`: reports that crash.
pub const SEND_CRASH: &str = "send_crash";
/// Forgets that crash.
pub const DISMISS_CRASH: &str = "dismiss_crash";
/// `{contact}`; `set_settings` takes the same.
pub const SETTINGS: &str = "settings";
pub const SET_SETTINGS: &str = "set_settings";

fn text(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
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
            SEND_PROBLEM => Box::pin(async move {
                let error = args.get("error").cloned().and_then(|e| serde_json::from_value(e).ok());
                let report_id =
                    reports?.send_problem(&text(&args, "message"), &text(&args, "contact"), error).await?;
                Ok(json!({"report_id": report_id}))
            }),
            LAST_CRASH => Box::pin(async move { Ok(json!(reports?.last_crash())) }),
            SEND_CRASH => Box::pin(async move {
                let report_id = reports?.send_crash(&text(&args, "message"), &text(&args, "contact")).await?;
                Ok(json!({"report_id": report_id}))
            }),
            DISMISS_CRASH => Box::pin(async move {
                reports?.dismiss_crash();
                Ok(Value::Null)
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
