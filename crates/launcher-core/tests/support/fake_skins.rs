//! A local imitation of where skins come from: Mojang's profile API (a UUID by nickname), its
//! session server (a profile's textures), its texture host, and the third-party skin and head
//! hosts under `/hosts/<host>/`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use launcher_core::skins::SkinEndpoints;
use serde_json::json;
use tokio::net::TcpListener;

/// The third-party skin hosts, in the order they are asked.
pub const SKIN_HOSTS: [&str; 2] = ["mc-heads", "minotar"];
/// The third-party head hosts, in the order they are asked.
pub const HEAD_HOSTS: [&str; 3] = ["mc-heads", "minotar", "mineskin"];

#[derive(Debug, Clone, Default)]
pub struct Recorded {
    /// The logical path: `/name/<name>`, `/session/<id>`, `/textures/<name>`, `/skin/<host>`,
    /// `/head/<host>`.
    pub path: String,
    /// What a third-party host was asked for.
    pub id: Option<String>,
    pub bearer: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Player {
    /// 32 hex digits.
    pub id: String,
    pub name: String,
    /// The texture it wears (served at `/textures/<name>`); `None`: a default skin.
    pub texture: Option<String>,
    pub slim: bool,
}

impl Player {
    pub fn new(id: &str, name: &str, texture: Option<&str>, slim: bool) -> Player {
        Player { id: id.into(), name: name.into(), texture: texture.map(str::to_string), slim }
    }
}

#[derive(Debug, Clone)]
pub struct Knobs {
    /// A status the session server answers every profile with (e.g. 204, 404, 429, 500).
    pub session_status: Option<u16>,
    /// A status the name lookup answers every nickname with.
    pub name_status: Option<u16>,
    /// Unknown nicknames answer 204 instead of 404.
    pub unknown_is_204: bool,
    /// `(status, content type)` of each skin host.
    pub skins: [(u16, &'static str); 2],
    /// `(status, content type)` of each head host.
    pub heads: [(u16, &'static str); 3],
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            session_status: None,
            name_status: None,
            unknown_is_204: false,
            skins: [(200, "image/png"); 2],
            heads: [(200, "image/png"); 3],
        }
    }
}

#[derive(Clone)]
struct Shared {
    base: String,
    knobs: Arc<Mutex<Knobs>>,
    players: Arc<Mutex<Vec<Player>>>,
    textures: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    /// What each skin host serves.
    host_skins: Arc<Mutex<[Vec<u8>; 2]>>,
    log: Arc<Mutex<Vec<Recorded>>>,
}

pub struct FakeSkins {
    pub base: String,
    shared: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeSkins {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A skin told apart from others by `mark` (the face's pixel (9, 9)); slim when `slim`.
pub fn skin_png(mark: u8, slim: bool) -> Vec<u8> {
    let mut image = image::RgbaImage::from_pixel(64, 64, image::Rgba([90, 60, 40, 255]));
    image.put_pixel(9, 9, image::Rgba([mark, 0, 0, 255]));
    for y in 0..16 {
        for x in 32..64 {
            image.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
        }
    }
    if slim {
        for y in 20..32 {
            image.put_pixel(54, y, image::Rgba([0, 0, 0, 0]));
            image.put_pixel(55, y, image::Rgba([0, 0, 0, 0]));
        }
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    png.into_inner()
}

impl FakeSkins {
    pub fn endpoints(&self) -> SkinEndpoints {
        SkinEndpoints::local(&self.base)
    }

    pub fn knobs(&self) -> MutexGuard<'_, Knobs> {
        self.shared.knobs.lock().unwrap()
    }

    pub fn add_player(&self, player: Player) {
        let mut players = self.shared.players.lock().unwrap();
        players.retain(|p| p.id != player.id);
        players.push(player);
    }

    /// Serves `bytes` at `/textures/<name>`.
    pub fn add_texture(&self, name: &str, bytes: Vec<u8>) {
        self.shared.textures.lock().unwrap().insert(name.into(), bytes);
    }

    pub fn texture_url(&self, name: &str) -> String {
        format!("{}/textures/{name}", self.base)
    }

    /// What skin host `index` serves from now on.
    pub fn set_host_skin(&self, index: usize, bytes: Vec<u8>) {
        self.shared.host_skins.lock().unwrap()[index] = bytes;
    }

    pub fn requests(&self, path: &str) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().iter().filter(|r| r.path == path).cloned().collect()
    }

    /// Requests whose logical path starts with `prefix`.
    pub fn requests_to(&self, prefix: &str) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().iter().filter(|r| r.path.starts_with(prefix)).cloned().collect()
    }

    pub fn all_requests(&self) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().clone()
    }
}

pub async fn start() -> FakeSkins {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let shared = Shared {
        base: base.clone(),
        knobs: Arc::new(Mutex::new(Knobs::default())),
        players: Arc::new(Mutex::new(Vec::new())),
        textures: Arc::new(Mutex::new(BTreeMap::new())),
        host_skins: Arc::new(Mutex::new([skin_png(201, false), skin_png(202, false)])),
        log: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/users/profiles/minecraft/{name}", get(name_lookup))
        .route("/session/minecraft/profile/{id}", get(session))
        .route("/textures/{name}", get(texture))
        .route("/hosts/{host}/skin/{id}", get(host_skin))
        .route("/hosts/{host}/{kind}/{id}/{size}", get(host_head))
        .with_state(shared.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    FakeSkins { base, shared, task }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").to_string())
}

fn push(s: &Shared, path: String, id: Option<String>, headers: &HeaderMap) {
    s.log.lock().unwrap().push(Recorded { path, id, bearer: bearer(headers) });
}

fn forced(status: u16) -> Response {
    (StatusCode::from_u16(status).unwrap(), Json(json!({"errorMessage": "forced"}))).into_response()
}

async fn name_lookup(State(s): State<Shared>, headers: HeaderMap, Path(name): Path<String>) -> Response {
    push(&s, format!("/name/{name}"), None, &headers);
    let knobs = s.knobs.lock().unwrap().clone();
    if let Some(status) = knobs.name_status {
        return forced(status);
    }
    let found = s.players.lock().unwrap().iter().find(|p| p.name.eq_ignore_ascii_case(&name)).cloned();
    match found {
        Some(player) => Json(json!({"id": player.id, "name": player.name})).into_response(),
        None if knobs.unknown_is_204 => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"errorMessage": "Couldn't find any profile"})))
            .into_response(),
    }
}

async fn session(State(s): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    push(&s, format!("/session/{id}"), None, &headers);
    if let Some(status) = s.knobs.lock().unwrap().session_status {
        return forced(status);
    }
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

async fn texture(State(s): State<Shared>, headers: HeaderMap, Path(name): Path<String>) -> Response {
    push(&s, format!("/textures/{name}"), None, &headers);
    match s.textures.lock().unwrap().get(&name) {
        Some(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn host_skin(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path((host, id)): Path<(String, String)>,
) -> Response {
    push(&s, format!("/skin/{host}"), Some(id), &headers);
    let Some(index) = SKIN_HOSTS.iter().position(|h| *h == host) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (status, content_type) = s.knobs.lock().unwrap().skins[index];
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let body = if content_type.starts_with("image/") {
        s.host_skins.lock().unwrap()[index].clone()
    } else {
        b"<html>not a skin</html>".to_vec()
    };
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn host_head(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path((host, _kind, id, _size)): Path<(String, String, String, String)>,
) -> Response {
    push(&s, format!("/head/{host}"), Some(id), &headers);
    let Some(index) = HEAD_HOSTS.iter().position(|h| *h == host) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (status, content_type) = s.knobs.lock().unwrap().heads[index];
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    ([(header::CONTENT_TYPE, content_type)], format!("PNG-{host}").into_bytes()).into_response()
}
