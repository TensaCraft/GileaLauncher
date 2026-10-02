//! Sending a report: `POST` of its JSON, 15 s, no sign-in; the server answers
//! `ok` and the report's id.

use std::time::Duration;

use launcher_core::net::api_client;
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;

pub const TIMEOUT: Duration = Duration::from_secs(15);
/// A debug build's own reports server, so that a manual check never reaches the real one.
pub const DEV_ENDPOINT_VAR: &str = "LAUNCHER_DEV_REPORTS_ENDPOINT";

/// Where reports go: the build profile's `[modules.reports] endpoint`; empty — nowhere.
pub fn endpoint() -> String {
    if cfg!(debug_assertions)
        && let Some(dev) =
            std::env::var(DEV_ENDPOINT_VAR).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    {
        return dev;
    }
    option_env!("LAUNCHER_MOD_REPORTS_ENDPOINT").unwrap_or_default().trim().to_string()
}

pub fn client() -> AppResult<Client> {
    api_client(&format!("{APP_NAME}/{VERSION}"), TIMEOUT, TIMEOUT)
}

fn network(detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Network, detail)
}

/// The server's words, briefly.
fn clip(text: &str) -> String {
    text.chars().take(200).collect()
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => false,
    }
}

/// Sends `payload` to `endpoint`; the report's id (`-` when the server gives none).
pub async fn send(http: &Client, endpoint: &str, payload: &Value) -> AppResult<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(AppError::new(ErrorCode::Unsupported, "reports are not configured in this build"));
    }
    let body = serde_json::to_vec(payload).map_err(|e| AppError::internal(e.to_string()))?;
    let response = http
        .post(endpoint)
        .header(CONTENT_TYPE, "application/json")
        .timeout(TIMEOUT)
        .body(body)
        .send()
        .await
        .map_err(|e| network(e.to_string()))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| network(e.to_string()))?;
    // The server answered: its reason (`reason`) is what the user is told, not a lost connection.
    if !status.is_success() {
        let reason = format!("HTTP {status}: {}", clip(&text));
        return Err(network(reason.clone())
            .with_param("status", status.as_u16().to_string())
            .with_param("reason", reason));
    }
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !truthy(answer.get("ok")) {
        let reason = ["error", "message"]
            .iter()
            .find_map(|key| answer.get(*key).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("the server did not accept the report: {}", clip(&text)));
        return Err(network(reason.clone()).with_param("reason", reason));
    }
    Ok(match answer.get("report_id") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.trim().to_string(),
        Some(Value::Number(id)) => id.to_string(),
        _ => "-".into(),
    })
}
