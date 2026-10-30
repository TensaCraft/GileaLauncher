//! «Сайт TensaCraft»: the server's site (Azuriom) signs players in through its auth API
//! (`/api/auth/authenticate`, `/verify`, `/logout`). The site vouches for who plays; the game
//! starts as an offline player of the site's nickname and UUID (the owner's choice, 2026-10-30),
//! as the server's offline mode expects. Its address comes from the build profile.

use std::time::Duration;

use launcher_core::auth::provider::{AccountProvider, ProviderAccount, ProviderFuture};
use launcher_core::net::api_client;
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};

pub use crate::SITE;
const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);

pub struct TensaSite {
    base: String,
    client: Client,
}

/// Why the site said no, by its `reason`.
fn refusal(status: u16, body: &Value) -> AppError {
    let reason = body["reason"].as_str().unwrap_or_default();
    let detail = format!("the site: HTTP {status} {body}");
    match (status, reason) {
        (_, "2fa") => AppError::new(ErrorCode::AuthCodeRequired, detail),
        (_, "invalid_credentials" | "invalid_2fa") => {
            AppError::new(ErrorCode::AuthCredentialsInvalid, detail)
        }
        (_, "invalid_token") | (401, _) => AppError::new(ErrorCode::ReauthRequired, detail),
        (_, "user_banned") | (403, _) => AppError::new(ErrorCode::AuthDenied, detail),
        (400, _) => AppError::new(ErrorCode::AuthServerInvalid, detail),
        _ => AppError::new(ErrorCode::AuthFailed, detail),
    }
}

fn account(body: &Value) -> AppResult<ProviderAccount> {
    let text = |field: &str| body[field].as_str().map(str::to_string).filter(|v| !v.is_empty());
    match (text("uuid"), text("username"), text("access_token")) {
        (Some(id), Some(name), Some(token)) => Ok(ProviderAccount { id, name, token }),
        _ => Err(AppError::new(ErrorCode::AuthFailed, format!("the site sent no account: {body}"))),
    }
}

impl TensaSite {
    pub fn new(base: &str) -> AppResult<TensaSite> {
        let client = api_client(&format!("{APP_NAME}/{VERSION}"), CONNECT, READ)?;
        Ok(TensaSite { base: base.trim_end_matches('/').to_string(), client })
    }

    /// A JSON POST to the auth API: the status and the body (`null` when it is no JSON).
    async fn send(&self, path: &str, body: &Value) -> AppResult<(u16, Value)> {
        let url = format!("{}/api/auth/{path}", self.base);
        let response = self
            .client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| AppError::new(ErrorCode::Network, format!("the site: {e}")))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    async fn post(&self, path: &str, body: Value) -> AppResult<ProviderAccount> {
        let (status, body) = self.send(path, &body).await?;
        if (200..300).contains(&status) { account(&body) } else { Err(refusal(status, &body)) }
    }
}

impl AccountProvider for TensaSite {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn name(&self) -> String {
        "TensaCraft".into()
    }

    fn sign_in<'a>(
        &'a self,
        login: &'a str,
        password: &'a str,
        code: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move {
            let mut body = json!({ "email": login.trim(), "password": password });
            if let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) {
                body["code"] = json!(code);
            }
            self.post("authenticate", body).await
        })
    }

    fn verify<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move { self.post("verify", json!({ "access_token": token })).await })
    }

    fn sign_out<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move { self.send("logout", &json!({ "access_token": token })).await.map(|_| ()) })
    }

    fn skin_url(&self, name: &str) -> Option<String> {
        Some(format!("{}/api/skin-api/skins/{name}.png", self.base))
    }
}
