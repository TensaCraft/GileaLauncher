//! A local authlib-injector (Yggdrasil) auth server: its site root points at the API with the
//! `X-Authlib-Injector-API-Location` header, as Ely.by's does.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const LOGIN: &str = "player@example.com";
pub const PASSWORD: &str = "secret";
pub const TOTP: &str = "123456";
pub const PROFILE_ID: &str = "7c0d1a2b3c4d5e6f708192a3b4c5d6e7";
pub const PROFILE_NAME: &str = "Alex_UA";
pub const SERVER_NAME: &str = "Fake Auth";

#[derive(Debug, Clone, Default)]
pub struct Knobs {
    /// The account has a second factor: a password without `:TOTP` is refused with Ely.by's text.
    pub two_factor: bool,
    /// The site root answers without the ALI header (the API is only at `/api/yggdrasil/`).
    pub no_ali: bool,
    /// Tokens the server takes as valid.
    pub valid_tokens: Vec<String>,
    /// Tokens no longer valid that a refresh still renews.
    pub stale_tokens: Vec<String>,
    /// The profile wears no skin of its own.
    pub no_skin: bool,
}

#[derive(Clone)]
struct Shared {
    base: String,
    knobs: Arc<Mutex<Knobs>>,
    log: Arc<Mutex<Vec<(String, Value)>>>,
    issued: Arc<Mutex<u32>>,
}

pub struct FakeYgg {
    pub base: String,
    shared: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeYgg {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeYgg {
    /// The API root the site root points at.
    pub fn api_root(&self) -> String {
        format!("{}/api/yggdrasil/", self.base)
    }

    /// Its session server, where a profile's textures are.
    pub fn session(&self) -> String {
        format!("{}sessionserver/", self.api_root())
    }

    pub fn set(&self, change: impl FnOnce(&mut Knobs)) {
        change(&mut self.shared.knobs.lock().unwrap());
    }

    /// The JSON bodies posted to `path` (under the API root), oldest first.
    pub fn posts(&self, path: &str) -> Vec<Value> {
        self.shared.log.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, b)| b.clone()).collect()
    }

    /// The valid tokens go stale: a check refuses them, a refresh renews them.
    pub fn stale_all(&self) {
        let mut knobs = self.shared.knobs.lock().unwrap();
        let tokens = std::mem::take(&mut knobs.valid_tokens);
        knobs.stale_tokens.extend(tokens);
    }

    /// Every token ends: only a new sign-in helps.
    pub fn end_all(&self) {
        let mut knobs = self.shared.knobs.lock().unwrap();
        knobs.valid_tokens.clear();
        knobs.stale_tokens.clear();
    }
}

pub async fn start() -> FakeYgg {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let shared = Shared {
        base: base.clone(),
        knobs: Arc::new(Mutex::new(Knobs::default())),
        log: Arc::new(Mutex::new(Vec::new())),
        issued: Arc::new(Mutex::new(0)),
    };
    let app = Router::new()
        .route("/", get(site))
        .route("/not-an-api", get(|| async { "<html>a shop</html>" }))
        .route("/api/yggdrasil/", get(metadata))
        .route("/api/yggdrasil/authserver/authenticate", post(authenticate))
        .route("/api/yggdrasil/authserver/refresh", post(refresh))
        .route("/api/yggdrasil/authserver/validate", post(validate))
        .route("/api/yggdrasil/authserver/invalidate", post(invalidate))
        .route("/api/yggdrasil/sessionserver/session/minecraft/profile/{id}", get(profile))
        .route("/textures/{name}", get(texture))
        // An API that sends its sign-in on to plain http elsewhere.
        .route(
            "/away/authserver/authenticate",
            post(|| async {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", "http://example.invalid/authserver/authenticate")],
                )
            }),
        )
        .with_state(shared.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    FakeYgg { base, shared, task }
}

/// The profile with its textures, as the session server gives it (the skin on this server, slim).
async fn profile(State(s): State<Shared>, axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    use base64::Engine;
    if id != PROFILE_ID {
        return StatusCode::NO_CONTENT.into_response();
    }
    let textures = if s.knobs.lock().unwrap().no_skin {
        json!({})
    } else {
        json!({ "SKIN": { "url": format!("{}/textures/alex", s.base), "metadata": { "model": "slim" } } })
    };
    let value =
        json!({ "timestamp": 0, "profileId": PROFILE_ID, "profileName": PROFILE_NAME, "textures": textures });
    let encoded = base64::engine::general_purpose::STANDARD.encode(value.to_string());
    Json(json!({ "id": PROFILE_ID, "name": PROFILE_NAME, "properties": [{ "name": "textures", "value": encoded }] }))
        .into_response()
}

async fn texture(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    if name != "alex" {
        return StatusCode::NOT_FOUND.into_response();
    }
    ([("content-type", "image/png")], super::fake_skins::skin_png(33, true)).into_response()
}

async fn site(State(s): State<Shared>) -> Response {
    let mut headers = HeaderMap::new();
    if !s.knobs.lock().unwrap().no_ali {
        headers.insert("X-Authlib-Injector-API-Location", "/api/yggdrasil/".parse().unwrap());
    }
    (headers, "<html>the site</html>").into_response()
}

async fn metadata() -> Json<Value> {
    Json(json!({
        "meta": { "serverName": SERVER_NAME, "implementationName": "fake", "feature.non_email_login": true },
        "skinDomains": ["127.0.0.1"],
        "signaturePublickey": "-----BEGIN PUBLIC KEY-----\nfake\n-----END PUBLIC KEY-----\n"
    }))
}

fn record(s: &Shared, path: &str, body: &Value) {
    s.log.lock().unwrap().push((path.to_string(), body.clone()));
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": "ForbiddenOperationException", "errorMessage": message })))
        .into_response()
}

fn session(s: &Shared, client_token: &str) -> Value {
    let mut issued = s.issued.lock().unwrap();
    *issued += 1;
    let token = format!("token-{issued}");
    s.knobs.lock().unwrap().valid_tokens.push(token.clone());
    let profile = json!({ "id": PROFILE_ID, "name": PROFILE_NAME });
    json!({
        "accessToken": token,
        "clientToken": client_token,
        "availableProfiles": [profile.clone()],
        "selectedProfile": profile,
    })
}

async fn authenticate(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, "authserver/authenticate", &body);
    let login = body["username"].as_str().unwrap_or_default();
    let password = body["password"].as_str().unwrap_or_default();
    let two_factor = s.knobs.lock().unwrap().two_factor;
    let expected = if two_factor { format!("{PASSWORD}:{TOTP}") } else { PASSWORD.to_string() };
    if login != LOGIN {
        return forbidden("Invalid credentials. Invalid username or password.");
    }
    if two_factor && password == PASSWORD {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "ForbiddenOperationException", "errorMessage": "Account protected with two factor auth." })),
        )
            .into_response();
    }
    if password != expected {
        return forbidden("Invalid credentials. Invalid username or password.");
    }
    let client_token = body["clientToken"].as_str().unwrap_or("server-made").to_string();
    Json(session(&s, &client_token)).into_response()
}

async fn refresh(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, "authserver/refresh", &body);
    let token = body["accessToken"].as_str().unwrap_or_default().to_string();
    let known = {
        let knobs = s.knobs.lock().unwrap();
        knobs.valid_tokens.contains(&token) || knobs.stale_tokens.contains(&token)
    };
    if !known {
        return forbidden("Invalid token.");
    }
    {
        let mut knobs = s.knobs.lock().unwrap();
        knobs.valid_tokens.retain(|t| t != &token);
        knobs.stale_tokens.retain(|t| t != &token);
    }
    let client_token = body["clientToken"].as_str().unwrap_or_default().to_string();
    Json(session(&s, &client_token)).into_response()
}

async fn validate(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, "authserver/validate", &body);
    let token = body["accessToken"].as_str().unwrap_or_default().to_string();
    if s.knobs.lock().unwrap().valid_tokens.contains(&token) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        forbidden("Invalid token.")
    }
}

async fn invalidate(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, "authserver/invalidate", &body);
    let token = body["accessToken"].as_str().unwrap_or_default().to_string();
    s.knobs.lock().unwrap().valid_tokens.retain(|t| t != &token);
    StatusCode::NO_CONTENT.into_response()
}
