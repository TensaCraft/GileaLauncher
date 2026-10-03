//! A local imitation of Microsoft identity, Xbox Live, Minecraft services and avatar hosts.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use launcher_core::auth::{AuthEndpoints, DEVICE_GRANT};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const MC_ID: &str = "0123456789abcdef0123456789abcdef";
pub const MC_NAME: &str = "Notch_UA";
pub const XUID: &str = "2535400000000001";
pub const GOOD_CODE: &str = "good-code";
pub const USER_CODE: &str = "ABCD-1234";

#[derive(Debug, Clone, Default)]
pub struct Recorded {
    pub path: String,
    pub form: BTreeMap<String, String>,
    pub json: Option<Value>,
    pub bearer: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Knobs {
    /// The token endpoint answers 503 this many times before working.
    pub token_failures: u32,
    /// `error` of the refresh reply (e.g. `invalid_grant`); `None` = success.
    pub refresh_error: Option<String>,
    pub omit_refresh_token: bool,
    /// Device polls answered `slow_down`, then `authorization_pending`, then `device_error` or tokens.
    pub device_slow_down: u32,
    pub device_pending: u32,
    pub device_error: Option<String>,
    pub xsts_xerr: Option<u64>,
    pub mc_login_status: Option<u16>,
    pub mc_profile_status: Option<u16>,
    /// `expires_in` of the Minecraft login; `None` omits it (the JWT `exp` is used).
    pub mc_expires_in: Option<i64>,
    pub jwt_exp: i64,
    /// `(status, content type)` for mc-heads, minotar, mineskin.
    pub avatars: [(u16, &'static str); 3],
}

#[derive(Clone)]
struct Shared {
    knobs: Arc<Mutex<Knobs>>,
    log: Arc<Mutex<Vec<Recorded>>>,
    token_calls: Arc<Mutex<u32>>,
    device_polls: Arc<Mutex<u32>>,
}

pub struct Fake {
    pub base: String,
    shared: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fake {
    pub fn endpoints(&self) -> AuthEndpoints {
        AuthEndpoints::local(&self.base)
    }

    pub fn knobs(&self) -> MutexGuard<'_, Knobs> {
        self.shared.knobs.lock().unwrap()
    }

    /// Requests seen at a logical path: `/token`, `/devicecode`, `/xbl`, `/xsts`, `/mc-login`,
    /// `/mc-profile`, `/avatar/<host>`.
    pub fn requests(&self, path: &str) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().iter().filter(|r| r.path == path).cloned().collect()
    }

    pub fn avatar_sources(&self) -> Vec<String> {
        vec![
            format!("{}/mc-heads/avatar/{{id}}/{{size}}", self.base),
            format!("{}/minotar/helm/{{id}}/{{size}}.png", self.base),
            format!("{}/mineskin/avatar/{{id}}/{{size}}", self.base),
        ]
    }
}

pub async fn start() -> Fake {
    let shared = Shared {
        knobs: Arc::new(Mutex::new(Knobs {
            mc_expires_in: Some(86_400),
            jwt_exp: 4_000_000_000,
            avatars: [(200, "image/png"); 3],
            ..Knobs::default()
        })),
        log: Arc::new(Mutex::new(Vec::new())),
        token_calls: Arc::new(Mutex::new(0)),
        device_polls: Arc::new(Mutex::new(0)),
    };
    let app = Router::new()
        .route("/consumers/oauth2/v2.0/token", post(token))
        .route("/consumers/oauth2/v2.0/devicecode", post(device_code))
        .route("/user/authenticate", post(xbl))
        .route("/xsts/authorize", post(xsts))
        .route("/authentication/login_with_xbox", post(mc_login))
        .route("/minecraft/profile", get(mc_profile))
        .route("/{host}/{kind}/{id}/{size}", get(avatar))
        .with_state(shared.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fake { base, shared, task }
}

fn record(s: &Shared, entry: Recorded) {
    s.log.lock().unwrap().push(entry);
}

fn oauth_error(error: &str, description: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error, "error_description": description })))
        .into_response()
}

fn ms_tokens(access: &str, refresh: &str, omit_refresh: bool) -> Response {
    let mut body = json!({ "access_token": access, "token_type": "Bearer", "expires_in": 3600 });
    if !omit_refresh {
        body["refresh_token"] = json!(refresh);
    }
    Json(body).into_response()
}

pub fn jwt(xuid: &str, exp: i64) -> String {
    let part = |v: Value| URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.signature", part(json!({ "alg": "none" })), part(json!({ "xuid": xuid, "exp": exp })))
}

async fn token(State(s): State<Shared>, Form(form): Form<BTreeMap<String, String>>) -> Response {
    record(&s, Recorded { path: "/token".into(), form: form.clone(), ..Recorded::default() });
    let k = s.knobs.lock().unwrap().clone();
    {
        let mut calls = s.token_calls.lock().unwrap();
        if *calls < k.token_failures {
            *calls += 1;
            return (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response();
        }
    }
    let grant = form.get("grant_type").map(String::as_str).unwrap_or_default();
    match grant {
        "authorization_code" if form.get("code").map(String::as_str) == Some(GOOD_CODE) => {
            ms_tokens("ms-access-1", "ms-refresh-1", k.omit_refresh_token)
        }
        "authorization_code" => oauth_error("invalid_grant", "unknown code"),
        "refresh_token" => match k.refresh_error {
            Some(error) => oauth_error(&error, "AADSTS70000: The provided grant has expired or is revoked."),
            None => ms_tokens("ms-access-2", "ms-refresh-2", k.omit_refresh_token),
        },
        g if g == DEVICE_GRANT => {
            let mut polls = s.device_polls.lock().unwrap();
            *polls += 1;
            if *polls <= k.device_slow_down {
                return oauth_error("slow_down", "");
            }
            if *polls <= k.device_slow_down + k.device_pending {
                return oauth_error("authorization_pending", "");
            }
            match k.device_error {
                Some(error) => oauth_error(&error, ""),
                None => ms_tokens("ms-access-d", "ms-refresh-d", false),
            }
        }
        _ => oauth_error("unsupported_grant_type", ""),
    }
}

async fn device_code(State(s): State<Shared>, Form(form): Form<BTreeMap<String, String>>) -> Response {
    record(&s, Recorded { path: "/devicecode".into(), form, ..Recorded::default() });
    Json(json!({
        "device_code": "dev-1",
        "user_code": USER_CODE,
        "verification_uri": "https://www.microsoft.com/link",
        "interval": 0,
        "expires_in": 900
    }))
    .into_response()
}

async fn xbl(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, Recorded { path: "/xbl".into(), json: Some(body), ..Recorded::default() });
    Json(json!({ "Token": "xbl-token", "DisplayClaims": { "xui": [{ "uhs": "uhs-1" }] } })).into_response()
}

async fn xsts(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, Recorded { path: "/xsts".into(), json: Some(body), ..Recorded::default() });
    match s.knobs.lock().unwrap().xsts_xerr {
        Some(xerr) => {
            (StatusCode::UNAUTHORIZED, Json(json!({ "XErr": xerr, "Message": "" }))).into_response()
        }
        None => Json(json!({ "Token": "xsts-token" })).into_response(),
    }
}

async fn mc_login(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    record(&s, Recorded { path: "/mc-login".into(), json: Some(body), ..Recorded::default() });
    let k = s.knobs.lock().unwrap().clone();
    if let Some(status) = k.mc_login_status {
        return (StatusCode::from_u16(status).unwrap(), Json(json!({ "error": "busy" }))).into_response();
    }
    let mut reply = json!({ "access_token": jwt(XUID, k.jwt_exp), "token_type": "Bearer" });
    if let Some(expires_in) = k.mc_expires_in {
        reply["expires_in"] = json!(expires_in);
    }
    Json(reply).into_response()
}

async fn mc_profile(State(s): State<Shared>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    record(&s, Recorded { path: "/mc-profile".into(), bearer, ..Recorded::default() });
    match s.knobs.lock().unwrap().mc_profile_status {
        Some(status) => {
            (StatusCode::from_u16(status).unwrap(), Json(json!({ "error": "NOT_FOUND" }))).into_response()
        }
        None => Json(json!({ "id": MC_ID, "name": MC_NAME })).into_response(),
    }
}

async fn avatar(
    State(s): State<Shared>,
    Path((host, _kind, id, _size)): Path<(String, String, String, String)>,
) -> Response {
    record(
        &s,
        Recorded {
            path: format!("/avatar/{host}"),
            form: BTreeMap::from([("id".into(), id)]),
            ..Recorded::default()
        },
    );
    let index = match host.as_str() {
        "mc-heads" => 0,
        "minotar" => 1,
        _ => 2,
    };
    let (status, content_type) = s.knobs.lock().unwrap().avatars[index];
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    ([(header::CONTENT_TYPE, content_type)], format!("PNG-{host}").into_bytes()).into_response()
}

/// Sends a bare `GET` like a browser would and returns the raw HTTP response.
pub async fn visit(url: &str) -> String {
    let parsed = reqwest::Url::parse(url).unwrap();
    let host = match parsed.host_str().unwrap() {
        "localhost" => "127.0.0.1".to_string(),
        other => other.trim_matches(['[', ']']).to_string(),
    };
    let target = match parsed.query() {
        Some(query) => format!("{}?{query}", parsed.path()),
        None => parsed.path().to_string(),
    };
    let mut stream = TcpStream::connect((host.as_str(), parsed.port().unwrap())).await.unwrap();
    stream
        .write_all(
            format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out).await;
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    /// Signs in and is redirected back with a good code.
    Consent,
    /// The user refuses.
    Deny,
    /// No browser can be opened.
    Broken,
    /// Opens but the user never comes back.
    Idle,
}

pub struct FakeBrowser {
    pub mode: Browser,
    pub opened: Mutex<Vec<String>>,
}

impl FakeBrowser {
    pub fn new(mode: Browser) -> Arc<FakeBrowser> {
        Arc::new(FakeBrowser { mode, opened: Mutex::new(Vec::new()) })
    }
}

impl launcher_core::auth::UrlOpener for FakeBrowser {
    fn open(&self, url: &str) -> bool {
        if self.mode == Browser::Broken {
            return false;
        }
        self.opened.lock().unwrap().push(url.to_string());
        let parsed = reqwest::Url::parse(url).unwrap();
        let param = |name: &str| parsed.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        // Device-code pages carry no redirect: nothing to click there.
        let Some(redirect) = param("redirect_uri") else { return true };
        let query = match self.mode {
            Browser::Consent => format!("code={GOOD_CODE}&state={}", param("state").unwrap_or_default()),
            // As Microsoft does (RFC 6749 4.1.2.1): the state comes back with an error too.
            Browser::Deny => format!(
                "error=access_denied&error_description=The+user+declined&state={}",
                param("state").unwrap_or_default()
            ),
            Browser::Broken | Browser::Idle => return true,
        };
        let target = format!("{redirect}?{query}");
        tokio::spawn(async move {
            visit(&target).await;
        });
        true
    }
}
