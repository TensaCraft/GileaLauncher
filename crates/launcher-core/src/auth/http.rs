//! JSON over HTTPS for the sign-in chain: 3 attempts on network errors and 5xx, non-JSON bodies
//! kept as `{"raw": …}`.

use std::time::Duration;

use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use super::AuthTimings;

const ATTEMPTS: u32 = 3;

/// Why a sign-in step produced no result.
#[derive(Debug, Clone, PartialEq)]
pub enum FlowError {
    /// The browser flow cannot run here (port busy, no browser): fall back to a device code.
    Unavailable(String),
    Timeout,
    Cancelled,
    Denied,
    /// Minecraft services answered 5xx: a device code would not help.
    ServicesUnavailable(String),
    Failed(String),
    App(AppError),
}

impl FlowError {
    pub fn into_app_error(self) -> AppError {
        match self {
            FlowError::Unavailable(detail) | FlowError::Failed(detail) => {
                AppError::new(ErrorCode::AuthFailed, detail)
            }
            FlowError::Timeout => AppError::new(ErrorCode::AuthTimeout, "sign-in timed out"),
            FlowError::Cancelled => AppError::new(ErrorCode::Cancelled, "sign-in cancelled"),
            FlowError::Denied => AppError::new(ErrorCode::AuthDenied, "access denied by the user"),
            FlowError::ServicesUnavailable(detail) => {
                AppError::new(ErrorCode::MinecraftServicesUnavailable, detail)
            }
            FlowError::App(e) => e,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body of a 2xx reply, else `Failed("{context} failed: HTTP {status} {body}")`.
    pub fn ok(self, context: &str) -> Result<Value, FlowError> {
        if self.is_success() {
            Ok(self.body)
        } else {
            Err(FlowError::Failed(format!("{context} failed: HTTP {} {}", self.status, self.body)))
        }
    }
}

pub struct AuthHttp {
    client: Client,
    backoff: Duration,
}

impl AuthHttp {
    pub fn new(timings: &AuthTimings) -> AppResult<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .user_agent(format!("{APP_NAME}/{VERSION}"))
            .timeout(timings.http_timeout)
            .build()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Self { client, backoff: timings.retry_backoff })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn post_form(
        &self,
        context: &str,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<Reply, FlowError> {
        self.send(context, || self.client.post(url).form(form)).await
    }

    pub async fn post_json(&self, context: &str, url: &str, body: &Value) -> Result<Reply, FlowError> {
        let bytes = serde_json::to_vec(body).map_err(|e| FlowError::Failed(e.to_string()))?;
        self.send(context, || {
            self.client.post(url).header(CONTENT_TYPE, "application/json").body(bytes.clone())
        })
        .await
    }

    pub async fn get_bearer(&self, context: &str, url: &str, token: &str) -> Result<Reply, FlowError> {
        self.send(context, || self.client.get(url).bearer_auth(token)).await
    }

    async fn send(&self, context: &str, build: impl Fn() -> RequestBuilder) -> Result<Reply, FlowError> {
        let mut last = Err(FlowError::Failed(format!("{context} failed: no attempt made")));
        for attempt in 1..=ATTEMPTS {
            match build().header(ACCEPT, "application/json").send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let text = response.text().await.unwrap_or_default();
                    let body = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
                    let reply = Reply { status, body };
                    if status < 500 {
                        return Ok(reply);
                    }
                    last = Ok(reply);
                }
                Err(e) => last = Err(FlowError::Failed(format!("{context} failed: {e}"))),
            }
            if attempt < ATTEMPTS {
                tokio::time::sleep(self.backoff * attempt).await;
            }
        }
        last
    }
}
