//! A local imitation of Minecraft Services (an account's skins and capes), Mojang's profile API,
//! its session server and the texture host.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const TOKEN: &str = "token-1";

#[derive(Debug, Clone, Default)]
pub struct Recorded {
    pub method: String,
    /// The logical path: `/profile`, `/skins`, `/capes`, `/name/<name>`, `/session/<id>`,
    /// `/textures/<name>`.
    pub path: String,
    pub bearer: Option<String>,
    /// Multipart fields: text as is, `file` as its byte count and bytes.
    pub fields: BTreeMap<String, Vec<u8>>,
    pub json: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Player {
    pub id: String,
    pub name: String,
    pub texture: Option<String>,
    pub slim: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Knobs {
    /// Only this bearer is taken; any other gets 401. `None`: any bearer.
    pub valid_token: Option<String>,
    /// Statuses forced on `/profile`, `/skins` and `/capes` (each while set).
    pub profile_status: Option<u16>,
    pub upload_status: Option<u16>,
    pub cape_status: Option<u16>,
    /// Changes answer with an empty body (the client then asks for the profile).
    pub bare_answers: bool,
    /// Unknown names answer 204 instead of 404.
    pub unknown_is_204: bool,
}

/// What the account wears and has.
#[derive(Debug, Clone, Default)]
pub struct Account {
    /// `(texture name, slim)`; `None`: no skin listed.
    pub skin: Option<(String, bool)>,
    /// The skin is one of Minecraft's own (`STEVE`, `ALEX`…), as the real profile names them.
    pub skin_alias: Option<String>,
    /// `(id, alias, texture name)`.
    pub capes: Vec<(String, String, String)>,
    pub active_cape: Option<String>,
}

#[derive(Clone)]
struct Shared {
    base: String,
    knobs: Arc<Mutex<Knobs>>,
    account: Arc<Mutex<Account>>,
    players: Arc<Mutex<Vec<Player>>>,
    textures: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    log: Arc<Mutex<Vec<Recorded>>>,
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
    pub fn knobs(&self) -> MutexGuard<'_, Knobs> {
        self.shared.knobs.lock().unwrap()
    }

    pub fn account(&self) -> MutexGuard<'_, Account> {
        self.shared.account.lock().unwrap()
    }

    pub fn add_player(&self, player: Player) {
        self.shared.players.lock().unwrap().push(player);
    }

    /// Serves `bytes` at `/textures/<name>`.
    pub fn add_texture(&self, name: &str, bytes: Vec<u8>) {
        self.shared.textures.lock().unwrap().insert(name.into(), bytes);
    }

    pub fn texture_url(&self, name: &str) -> String {
        format!("{}/textures/{name}", self.base)
    }

    pub fn requests(&self, path: &str) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().iter().filter(|r| r.path == path).cloned().collect()
    }

    pub fn all_requests(&self) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().clone()
    }
}

pub async fn start() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let shared = Shared {
        base: base.clone(),
        knobs: Arc::new(Mutex::new(Knobs::default())),
        account: Arc::new(Mutex::new(Account::default())),
        players: Arc::new(Mutex::new(Vec::new())),
        textures: Arc::new(Mutex::new(BTreeMap::new())),
        log: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/minecraft/profile", get(profile))
        .route("/minecraft/profile/skins", axum::routing::post(upload))
        .route("/minecraft/profile/capes/active", axum::routing::put(put_cape).delete(delete_cape))
        .route("/users/profiles/minecraft/{name}", get(name_lookup))
        .route("/session/minecraft/profile/{id}", get(session))
        .route("/textures/{name}", get(texture))
        .with_state(shared.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fake { base, shared, task }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

fn record(method: &Method, path: String, headers: &HeaderMap) -> Recorded {
    Recorded { method: method.to_string(), path, bearer: bearer(headers), ..Recorded::default() }
}

fn push(s: &Shared, r: Recorded) {
    s.log.lock().unwrap().push(r);
}

/// 401 for a bearer the knobs do not take; a forced status; else `None`.
fn refused(s: &Shared, headers: &HeaderMap, forced: Option<u16>) -> Option<Response> {
    let valid = s.knobs.lock().unwrap().valid_token.clone();
    if let Some(valid) = valid
        && bearer(headers).as_deref() != Some(valid.as_str())
    {
        return Some((StatusCode::UNAUTHORIZED, "").into_response());
    }
    forced.map(|code| {
        let status = StatusCode::from_u16(code).unwrap();
        (status, Json(json!({"errorMessage": "forced"}))).into_response()
    })
}

fn profile_json(s: &Shared) -> Value {
    let account = s.account.lock().unwrap();
    let skins: Vec<Value> = account
        .skin
        .iter()
        .map(|(texture, slim)| {
            let mut skin = json!({
                "id": format!("skin-{texture}"),
                "state": "ACTIVE",
                "url": format!("{}/textures/{texture}", s.base),
                "textureKey": texture,
                "variant": if *slim { "SLIM" } else { "CLASSIC" },
            });
            if let Some(alias) = &account.skin_alias {
                skin["alias"] = json!(alias);
            }
            skin
        })
        .collect();
    let capes: Vec<Value> = account
        .capes
        .iter()
        .map(|(id, alias, texture)| {
            let active = account.active_cape.as_deref() == Some(id.as_str());
            json!({
                "id": id,
                "state": if active { "ACTIVE" } else { "INACTIVE" },
                "url": format!("{}/textures/{texture}", s.base),
                "alias": alias,
            })
        })
        .collect();
    json!({"id": "0123456789abcdef0123456789abcdef", "name": "Steve_42", "skins": skins, "capes": capes})
}

fn changed(s: &Shared) -> Response {
    if s.knobs.lock().unwrap().bare_answers {
        return StatusCode::OK.into_response();
    }
    Json(profile_json(s)).into_response()
}

async fn profile(State(s): State<Shared>, method: Method, headers: HeaderMap) -> Response {
    push(&s, record(&method, "/profile".into(), &headers));
    let forced = s.knobs.lock().unwrap().profile_status;
    if let Some(answer) = refused(&s, &headers, forced) {
        return answer;
    }
    Json(profile_json(&s)).into_response()
}

async fn upload(
    State(s): State<Shared>,
    method: Method,
    headers: HeaderMap,
    mut form: Multipart,
) -> Response {
    let mut seen = record(&method, "/skins".into(), &headers);
    while let Ok(Some(field)) = form.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.unwrap_or_default();
        seen.fields.insert(name, bytes.to_vec());
    }
    push(&s, seen.clone());
    let forced = s.knobs.lock().unwrap().upload_status;
    if let Some(answer) = refused(&s, &headers, forced) {
        return answer;
    }
    let file = seen.fields.get("file").cloned().unwrap_or_default();
    let slim = seen.fields.get("variant").is_some_and(|v| v == b"slim");
    let texture = format!("uploaded{}", s.textures.lock().unwrap().len());
    s.textures.lock().unwrap().insert(texture.clone(), file);
    s.account.lock().unwrap().skin = Some((texture, slim));
    changed(&s)
}

async fn put_cape(State(s): State<Shared>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let mut seen = record(&method, "/capes".into(), &headers);
    seen.json = serde_json::from_slice(&body).ok();
    push(&s, seen.clone());
    let forced = s.knobs.lock().unwrap().cape_status;
    if let Some(answer) = refused(&s, &headers, forced) {
        return answer;
    }
    let id = seen.json.as_ref().and_then(|j| j["capeId"].as_str()).map(str::to_string);
    let known = id.as_ref().is_some_and(|id| s.account.lock().unwrap().capes.iter().any(|c| &c.0 == id));
    if !known {
        return (StatusCode::BAD_REQUEST, Json(json!({"errorMessage": "no such cape"}))).into_response();
    }
    s.account.lock().unwrap().active_cape = id;
    changed(&s)
}

async fn delete_cape(State(s): State<Shared>, method: Method, headers: HeaderMap) -> Response {
    push(&s, record(&method, "/capes".into(), &headers));
    let forced = s.knobs.lock().unwrap().cape_status;
    if let Some(answer) = refused(&s, &headers, forced) {
        return answer;
    }
    s.account.lock().unwrap().active_cape = None;
    changed(&s)
}

async fn name_lookup(
    State(s): State<Shared>,
    method: Method,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    push(&s, record(&method, format!("/name/{name}"), &headers));
    let found = s.players.lock().unwrap().iter().find(|p| p.name.eq_ignore_ascii_case(&name)).cloned();
    match found {
        Some(player) => Json(json!({"id": player.id, "name": player.name})).into_response(),
        None if s.knobs.lock().unwrap().unknown_is_204 => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"errorMessage": "Couldn't find any profile"})))
            .into_response(),
    }
}

async fn session(
    State(s): State<Shared>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    push(&s, record(&method, format!("/session/{id}"), &headers));
    let Some(player) = s.players.lock().unwrap().iter().find(|p| p.id == id).cloned() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let mut textures = json!({});
    if let Some(texture) = &player.texture {
        let mut skin = json!({"url": format!("{}/textures/{texture}", s.base)});
        if player.slim {
            skin["metadata"] = json!({"model": "slim"});
        }
        textures["SKIN"] = skin;
    }
    let value =
        json!({"timestamp": 1, "profileId": player.id, "profileName": player.name, "textures": textures});
    Json(json!({
        "id": player.id,
        "name": player.name,
        "properties": [{"name": "textures", "value": STANDARD.encode(value.to_string())}],
    }))
    .into_response()
}

async fn texture(
    State(s): State<Shared>,
    method: Method,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    push(&s, record(&method, format!("/textures/{name}"), &headers));
    match s.textures.lock().unwrap().get(&name) {
        Some(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
