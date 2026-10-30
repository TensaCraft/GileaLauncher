//! Auth servers that speak authlib-injector's Yggdrasil API (Ely.by, a server of one's own): finding
//! the API behind an address, signing in, and keeping the session alive. The game reaches the same
//! server through the authlib-injector agent (`launch`).

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};
use url::Url;

use super::http::{AuthHttp, FlowError, Reply};

/// An auth server found behind an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthServerInfo {
    /// The API root, ending in `/`: the agent's argument, and the base of every endpoint.
    pub api_root: String,
    /// `meta.serverName`, or the host when it has none.
    pub name: String,
    /// The metadata as the server sent it: the game gets it prefetched, so a server down at
    /// launch does not stop it.
    pub metadata: String,
}

/// What a sign-in sends. `code` is the second factor, sent after the password (`password:code`),
/// as Ely.by asks.
#[derive(Debug, Clone, Copy)]
pub struct Credentials<'a> {
    pub login: &'a str,
    pub password: &'a str,
    pub code: Option<&'a str>,
    pub client_token: &'a str,
}

/// A player's game profile on the server (`id` without dashes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: String,
    pub client_token: String,
    /// The profile the session plays as; none until one of `profiles` is chosen.
    pub profile: Option<GameProfile>,
    pub profiles: Vec<GameProfile>,
}

fn invalid_server(detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::AuthServerInvalid, detail)
}

fn network(e: FlowError) -> AppError {
    match e {
        FlowError::Failed(detail) => AppError::new(ErrorCode::Network, detail),
        other => other.into_app_error(),
    }
}

fn on_this_computer(url: &Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

/// What the player typed, as an address: `https://` when it names none; plain `http` only for this
/// computer (a password would cross the network in the clear).
pub fn address(input: &str) -> AppResult<Url> {
    let input = input.trim();
    if input.is_empty() {
        return Err(invalid_server("no address"));
    }
    let spelled = if input.contains("://") { input.to_string() } else { format!("https://{input}") };
    let url = Url::parse(&spelled).map_err(|e| invalid_server(format!("{input}: {e}")))?;
    match url.scheme() {
        "https" if url.host_str().is_some() => Ok(url),
        "http" if on_this_computer(&url) => Ok(url),
        _ => Err(invalid_server(format!("{input}: only https"))),
    }
}

fn with_slash(url: &Url) -> String {
    let url = url.as_str();
    if url.ends_with('/') { url.to_string() } else { format!("{url}/") }
}

/// The auth server behind `input` (a site, or its API root): the site's
/// `X-Authlib-Injector-API-Location` header leads to the API, whose root answers with metadata.
pub async fn resolve(http: &AuthHttp, input: &str) -> AppResult<AuthServerInfo> {
    let asked = address(input)?;
    let first = http.get_raw("auth server", asked.as_str()).await.map_err(network)?;
    let landed = address(&first.url)?;
    let api = match first.api_location.as_deref().map(|at| landed.join(at)) {
        Some(Ok(api)) if api != landed => {
            let api = address(api.as_str())?;
            let reply = http.get_raw("auth server", api.as_str()).await.map_err(network)?;
            (address(&reply.url)?, reply)
        }
        Some(Err(e)) => return Err(invalid_server(format!("API location: {e}"))),
        _ => (landed, first),
    };
    let (root, reply) = api;
    let metadata: Value = serde_json::from_str(&reply.text)
        .ok()
        .filter(|m: &Value| (200..300).contains(&reply.status) && m["meta"].is_object())
        .ok_or_else(|| invalid_server(format!("{root}: no auth server metadata")))?;
    let name = metadata["meta"]["serverName"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| root.host_str().unwrap_or_default().to_string());
    Ok(AuthServerInfo { api_root: with_slash(&root), name, metadata: reply.text })
}

fn profile_of(value: &Value) -> Option<GameProfile> {
    Some(GameProfile { id: value["id"].as_str()?.to_string(), name: value["name"].as_str()?.to_string() })
}

fn session_of(body: &Value) -> AppResult<Session> {
    let access_token = body["accessToken"].as_str().filter(|t| !t.is_empty());
    let Some(access_token) = access_token else {
        return Err(AppError::new(ErrorCode::AuthFailed, "the auth server sent no access token"));
    };
    Ok(Session {
        access_token: access_token.to_string(),
        client_token: body["clientToken"].as_str().unwrap_or_default().to_string(),
        profile: profile_of(&body["selectedProfile"]),
        profiles: body["availableProfiles"].as_array().into_iter().flatten().filter_map(profile_of).collect(),
    })
}

fn message(reply: &Reply) -> String {
    reply.body["errorMessage"].as_str().unwrap_or_default().to_string()
}

/// A refusal of the server's own (`ForbiddenOperationException`, 401 or 403).
fn refused(reply: &Reply) -> bool {
    matches!(reply.status, 401 | 403)
}

fn failed(context: &str, reply: &Reply) -> AppError {
    AppError::new(ErrorCode::AuthFailed, format!("{context}: HTTP {} {}", reply.status, reply.body))
}

/// Signs in; `AuthCodeRequired` when the account has a second factor and `code` is missing.
pub async fn authenticate(
    http: &AuthHttp,
    api_root: &str,
    credentials: &Credentials<'_>,
) -> AppResult<Session> {
    let password = match credentials.code {
        Some(code) => format!("{}:{}", credentials.password, code.trim()),
        None => credentials.password.to_string(),
    };
    let body = json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": credentials.login.trim(),
        "password": password,
        "clientToken": credentials.client_token,
        "requestUser": true,
    });
    let url = format!("{api_root}authserver/authenticate");
    let reply = http.post_json("auth server sign-in", &url, &body).await.map_err(network)?;
    if reply.is_success() {
        return session_of(&reply.body);
    }
    if refused(&reply) {
        let two_factor = message(&reply).to_lowercase().contains("two factor");
        return Err(if two_factor && credentials.code.is_none() {
            AppError::new(ErrorCode::AuthCodeRequired, message(&reply))
        } else {
            AppError::new(ErrorCode::AuthCredentialsInvalid, message(&reply))
        });
    }
    Err(failed("auth server sign-in", &reply))
}

/// A new access token for the session (the old one ends); `ReauthRequired` when the server no
/// longer knows it, and the password is needed again.
pub async fn refresh(
    http: &AuthHttp,
    api_root: &str,
    access_token: &str,
    client_token: &str,
) -> AppResult<Session> {
    let body = json!({ "accessToken": access_token, "clientToken": client_token, "requestUser": true });
    let url = format!("{api_root}authserver/refresh");
    let reply = http.post_json("auth server refresh", &url, &body).await.map_err(network)?;
    if reply.is_success() {
        return session_of(&reply.body);
    }
    if refused(&reply) {
        return Err(AppError::new(ErrorCode::ReauthRequired, message(&reply)));
    }
    Err(failed("auth server refresh", &reply))
}

/// Whether the server still takes `access_token`.
pub async fn validate(
    http: &AuthHttp,
    api_root: &str,
    access_token: &str,
    client_token: &str,
) -> AppResult<bool> {
    let body = json!({ "accessToken": access_token, "clientToken": client_token });
    let url = format!("{api_root}authserver/validate");
    let reply = http.post_json("auth server check", &url, &body).await.map_err(network)?;
    match reply.status {
        200..=299 => Ok(true),
        400 | 401 | 403 => Ok(false),
        _ => Err(failed("auth server check", &reply)),
    }
}

/// Ends the session on the server; a server out of reach only means the token ends on its own.
pub async fn invalidate(http: &AuthHttp, api_root: &str, access_token: &str, client_token: &str) {
    let body = json!({ "accessToken": access_token, "clientToken": client_token });
    let url = format!("{api_root}authserver/invalidate");
    if let Err(e) = http.post_json("auth server sign-out", &url, &body).await {
        tracing::debug!("The auth server did not take the sign-out: {e:?}");
    }
}
