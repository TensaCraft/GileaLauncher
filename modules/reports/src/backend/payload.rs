//! The report the server receives, with nothing private left in it.

use std::path::PathBuf;

use launcher_core::feedback::{FeedbackService, ReportKind};
use serde_json::{Map, Value, json};

use super::log::{LOG_CAP, cap, combined};
use super::redact::Redactor;

/// The latest activity a report carries.
pub const ACTIVITY_IN_REPORT: usize = 12;

/// What went wrong, as the caller tells it.
#[derive(Debug, Clone)]
pub struct ReportInput {
    pub kind: ReportKind,
    /// English; empty means "Launcher report".
    pub title: String,
    pub message: String,
    pub screen: String,
    pub action: String,
    /// An object merged into the report's metadata.
    pub metadata: Value,
    pub attachments: Vec<PathBuf>,
}

/// What the launcher adds to every report.
#[derive(Debug, Clone)]
pub struct Env {
    pub version: String,
    /// `windows`, `macos` or `linux`.
    pub platform: &'static str,
    pub os: String,
    pub home: Option<PathBuf>,
    pub app_log: Option<PathBuf>,
    pub contact: Option<String>,
    /// [`feedback_snapshot`].
    pub feedback: Value,
}

/// The report's JSON.
pub fn build(input: ReportInput, env: &Env) -> Value {
    let redactor = Redactor::new(env.home.as_deref());
    let title = match input.title.trim() {
        "" => "Launcher report",
        title => title,
    };
    let mut log = combined(&input.attachments, env.app_log.as_deref());
    if log.trim().is_empty() {
        log = if input.message.trim().is_empty() { title.to_string() } else { input.message.clone() };
    }
    let contact = env.contact.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let mut metadata = Map::new();
    metadata.insert("screen".into(), json!(input.screen));
    metadata.insert("action".into(), json!(input.action));
    metadata.insert("runtime".into(), json!("rust"));
    metadata.insert("launcher_version".into(), json!(env.version));
    metadata.insert("feedback".into(), env.feedback.clone());
    if let Value::Object(extra) = input.metadata {
        metadata.extend(extra);
    }
    if let Some(contact) = contact {
        metadata.insert("contact".into(), json!(contact));
    }
    let mut payload = json!({
        "type": input.kind.as_str(),
        "severity": "error",
        "platform": env.platform,
        "launcher_version": env.version,
        "os": env.os,
        "title": redactor.text(title),
        "message": redactor.text(&input.message),
        "log": cap(&redactor.text(&log), LOG_CAP, "combined log"),
        "metadata": redactor.value(Value::Object(metadata)),
    });
    if let Some(contact) = contact {
        payload["contact"] = json!(contact);
    }
    payload
}

/// What the launcher was doing: whether it is busy, its operations and its latest activity.
pub fn feedback_snapshot(feedback: &FeedbackService) -> Value {
    let ops = feedback.snapshot();
    json!({
        "busy": ops.busy,
        "active_operations": ops.operations,
        "recent_activity": feedback.activity(ACTIVITY_IN_REPORT),
    })
}
