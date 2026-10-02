//! A fake CurseForge on this machine: the API's mods, files and search, and the files themselves.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tokio::net::TcpListener;

/// The key the fake accepts; any other is refused as CurseForge refuses one.
pub const TEST_KEY: &str = "test-key-123";
pub const MODS: u64 = 6;
pub const RESOURCE_PACKS: u64 = 12;
pub const SHADERS: u64 = 6552;
pub const MODPACKS: u64 = 4471;

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub key: Option<String>,
    pub agent: Option<String>,
    pub body: Option<Value>,
}

#[derive(Default)]
pub struct Data {
    /// Mod id → the mod as the API answers it.
    pub mods: HashMap<u64, Value>,
    /// Mod id → its files, newest first.
    pub files: HashMap<u64, Vec<Value>>,
    /// Path → a status answered instead.
    pub status: HashMap<String, u16>,
    /// Path → a body answered as is (not JSON).
    pub raw: HashMap<String, String>,
    /// `/files/{name}` → its bytes.
    pub blobs: HashMap<String, Vec<u8>>,
    /// Modrinth's files by SHA-1: (address, size).
    pub modrinth: HashMap<String, (String, u64)>,
    pub seen: Vec<Seen>,
}

pub struct FakeCurseForge {
    pub base: String,
    pub data: Arc<Mutex<Data>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeCurseForge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type Shared = Arc<Mutex<Data>>;

fn text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// Records the request; answers the path's overrides or refuses a wrong key.
fn note(
    data: &Shared,
    method: &str,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Option<Value>,
) -> Option<Response> {
    let mut data = data.lock().unwrap();
    let mut pairs: Vec<(String, String)> = query.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    pairs.sort();
    let key = text(headers, "x-api-key");
    data.seen.push(Seen {
        method: method.into(),
        path: path.into(),
        query: pairs,
        key: key.clone(),
        agent: text(headers, "user-agent"),
        body,
    });
    if let Some(status) = data.status.get(path) {
        return Some(StatusCode::from_u16(*status).unwrap().into_response());
    }
    if let Some(raw) = data.raw.get(path) {
        return Some(raw.clone().into_response());
    }
    if key.as_deref() != Some(TEST_KEY) {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    None
}

fn page(items: Vec<Value>, query: &HashMap<String, String>) -> Value {
    let index: usize = query.get("index").and_then(|v| v.parse().ok()).unwrap_or(0);
    let size: usize = query.get("pageSize").and_then(|v| v.parse().ok()).unwrap_or(50);
    let total = items.len();
    let data: Vec<Value> = items.into_iter().skip(index).take(size).collect();
    json!({
        "data": data,
        "pagination": {"index": index, "pageSize": size, "resultCount": data.len(), "totalCount": total}
    })
}

/// The loader names CurseForge lists among a file's game versions.
fn loader_name(loader: &str) -> &'static str {
    match loader {
        "1" => "Forge",
        "4" => "Fabric",
        "5" => "Quilt",
        "6" => "NeoForge",
        _ => "",
    }
}

fn file_fits(file: &Value, query: &HashMap<String, String>) -> bool {
    let versions: Vec<&str> =
        file["gameVersions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    query.get("gameVersion").is_none_or(|v| versions.contains(&v.as_str()))
        && query
            .get("modLoaderType")
            .is_none_or(|l| loader_name(l).is_empty() || versions.contains(&loader_name(l)))
}

async fn search(
    State(data): State<Shared>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(answer) = note(&data, "GET", "/v1/mods/search", &query, &headers, None) {
        return answer;
    }
    let data = data.lock().unwrap();
    let class: Option<u64> = query.get("classId").and_then(|v| v.parse().ok());
    let filter = query.get("searchFilter").map(|s| s.to_lowercase()).unwrap_or_default();
    let mut hits: Vec<Value> = data
        .mods
        .values()
        .filter(|m| class.is_none_or(|c| m["classId"].as_u64() == Some(c)))
        .filter(|m| m["name"].as_str().unwrap_or_default().to_lowercase().contains(&filter))
        .filter(|m| {
            let files = data.files.get(&m["id"].as_u64().unwrap()).cloned().unwrap_or_default();
            files.iter().any(|f| file_fits(f, &query))
        })
        .cloned()
        .collect();
    hits.sort_by_key(|m| std::cmp::Reverse(m["downloadCount"].as_u64().unwrap_or(0)));
    axum::Json(page(hits, &query)).into_response()
}

async fn one_mod(State(data): State<Shared>, Path(id): Path<u64>, headers: HeaderMap) -> Response {
    if let Some(answer) = note(&data, "GET", &format!("/v1/mods/{id}"), &HashMap::new(), &headers, None) {
        return answer;
    }
    match data.lock().unwrap().mods.get(&id) {
        Some(m) => axum::Json(json!({"data": m})).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn many_mods(State(data): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if let Some(answer) = note(&data, "POST", "/v1/mods", &HashMap::new(), &headers, Some(body.clone())) {
        return answer;
    }
    let data = data.lock().unwrap();
    let found: Vec<Value> = body["modIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_u64().and_then(|id| data.mods.get(&id).cloned()))
        .collect();
    axum::Json(json!({"data": found})).into_response()
}

async fn mod_files(
    State(data): State<Shared>,
    Path(id): Path<u64>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(answer) = note(&data, "GET", &format!("/v1/mods/{id}/files"), &query, &headers, None) {
        return answer;
    }
    let files: Vec<Value> = data
        .lock()
        .unwrap()
        .files
        .get(&id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| file_fits(f, &query))
        .collect();
    axum::Json(page(files, &query)).into_response()
}

async fn many_files(State(data): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if let Some(answer) = note(&data, "POST", "/v1/mods/files", &HashMap::new(), &headers, Some(body.clone()))
    {
        return answer;
    }
    let data = data.lock().unwrap();
    let wanted: Vec<u64> =
        body["fileIds"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    let found: Vec<Value> = data
        .files
        .values()
        .flatten()
        .filter(|f| f["id"].as_u64().is_some_and(|id| wanted.contains(&id)))
        .cloned()
        .collect();
    axum::Json(json!({"data": found})).into_response()
}

async fn fingerprints(State(data): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if let Some(answer) =
        note(&data, "POST", "/v1/fingerprints/432", &HashMap::new(), &headers, Some(body.clone()))
    {
        return answer;
    }
    let data = data.lock().unwrap();
    let wanted: Vec<u64> =
        body["fingerprints"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    let matches: Vec<Value> = data
        .files
        .values()
        .flatten()
        .filter(|f| f["fileFingerprint"].as_u64().is_some_and(|fp| wanted.contains(&fp)))
        .map(|f| json!({"id": f["modId"], "file": f, "latestFiles": []}))
        .collect();
    axum::Json(json!({"data": {"exactMatches": matches}})).into_response()
}

/// Modrinth's `POST /v2/version_files` (by SHA-1), as the same fake answers it.
async fn modrinth_files(State(data): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let mut guard = data.lock().unwrap();
    guard.seen.push(Seen {
        method: "POST".into(),
        path: "/v2/version_files".into(),
        query: Vec::new(),
        key: text(&headers, "x-api-key"),
        agent: text(&headers, "user-agent"),
        body: Some(body.clone()),
    });
    let mut answer = serde_json::Map::new();
    for hash in body["hashes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some((url, size)) = guard.modrinth.get(hash) {
            answer.insert(
                hash.to_string(),
                json!({"id": "v1", "project_id": "MR", "files": [
                    {"hashes": {"sha1": hash, "sha512": "0"}, "url": url, "filename": "x.jar", "size": size, "primary": true}
                ]}),
            );
        }
    }
    axum::Json(Value::Object(answer)).into_response()
}

async fn blob(State(data): State<Shared>, Path(name): Path<String>, headers: HeaderMap) -> Response {
    let path = format!("/files/{name}");
    let mut guard = data.lock().unwrap();
    guard.seen.push(Seen {
        method: "GET".into(),
        path: path.clone(),
        query: Vec::new(),
        key: text(&headers, "x-api-key"),
        agent: text(&headers, "user-agent"),
        body: None,
    });
    if let Some(status) = guard.status.get(&path) {
        return StatusCode::from_u16(*status).unwrap().into_response();
    }
    match guard.blobs.get(&name) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

impl FakeCurseForge {
    pub async fn start() -> FakeCurseForge {
        let data: Shared = Arc::default();
        let app = Router::new()
            .route("/v1/mods/search", get(search))
            .route("/v1/mods", post(many_mods))
            .route("/v1/mods/files", post(many_files))
            .route("/v1/mods/{id}", get(one_mod))
            .route("/v1/mods/{id}/files", get(mod_files))
            .route("/v1/fingerprints/432", post(fingerprints))
            .route("/files/{name}", get(blob))
            .route("/v2/version_files", post(modrinth_files))
            .with_state(data.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeCurseForge { base, data, task }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.data.lock().unwrap().seen.clone()
    }

    /// Serves `body` at `/files/{name}`; its address.
    pub fn blob(&self, name: &str, body: &[u8]) -> String {
        self.data.lock().unwrap().blobs.insert(name.to_string(), body.to_vec());
        format!("{}/files/{name}", self.base)
    }

    /// Serves `body` as Modrinth's file `name` (found by its SHA-1).
    pub fn on_modrinth(&self, name: &str, body: &[u8]) {
        let url = self.blob(&format!("mr-{name}"), body);
        self.data.lock().unwrap().modrinth.insert(sha1_hex(body), (url, body.len() as u64));
    }

    /// Publishes a mod and its files (newest first).
    pub fn publish(&self, mut project: Value, files: Vec<Value>) {
        let id = project["id"].as_u64().unwrap();
        project["latestFilesIndexes"] = latest_indexes(&files);
        let mut data = self.data.lock().unwrap();
        data.mods.insert(id, project);
        data.files.insert(id, files);
    }
}

/// CurseForge's `latestFilesIndexes`: the newest file of each game version, loader and release type.
fn latest_indexes(files: &[Value]) -> Value {
    let loader_type = |tag: &str| match tag {
        "Forge" => Some(1),
        "Fabric" => Some(4),
        "Quilt" => Some(5),
        "NeoForge" => Some(6),
        _ => None,
    };
    let mut newest: HashMap<(String, Option<u64>, u64), Value> = HashMap::new();
    for file in files {
        let tags: Vec<&str> =
            file["gameVersions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let games = tags.iter().filter(|t| t.starts_with(|c: char| c.is_ascii_digit()));
        let loaders: Vec<Option<u64>> = {
            let found: Vec<Option<u64>> = tags.iter().filter_map(|t| loader_type(t)).map(Some).collect();
            if found.is_empty() { vec![None] } else { found }
        };
        let (id, release) = (file["id"].as_u64().unwrap(), file["releaseType"].as_u64().unwrap_or(1));
        for game in games {
            for loader in &loaders {
                let key = (game.to_string(), *loader, release);
                let entry = json!({"gameVersion": game, "fileId": id, "filename": file["fileName"],
                    "releaseType": release, "modLoader": loader});
                if newest.get(&key).is_none_or(|e| e["fileId"].as_u64().unwrap() < id) {
                    newest.insert(key, entry);
                }
            }
        }
    }
    Value::Array(newest.into_values().collect())
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

/// A mod as the API describes it.
pub fn mod_json(id: u64, name: &str, class_id: u64, downloads: u64) -> Value {
    let slug = name.to_lowercase().replace(' ', "-");
    json!({
        "id": id,
        "gameId": 432,
        "name": name,
        "slug": slug,
        "summary": format!("{name} does things"),
        "downloadCount": downloads,
        "classId": class_id,
        "allowModDistribution": true,
        "authors": [{"name": "Author"}],
        "logo": {"thumbnailUrl": format!("https://media.forgecdn.net/avatars/{id}.png"), "url": ""},
        "links": {"websiteUrl": format!("https://www.curseforge.com/minecraft/mc-mods/{slug}")}
    })
}

/// A file of mod `mod_id` served by `server` (`blocked`: the author disallows other apps, so
/// the API gives no address).
#[allow(clippy::too_many_arguments)]
pub fn file_json(
    server: &FakeCurseForge,
    id: u64,
    mod_id: u64,
    name: &str,
    body: &[u8],
    game_versions: &[&str],
    dependencies: Value,
    blocked: bool,
) -> Value {
    let url = if blocked { Value::Null } else { Value::String(server.blob(name, body)) };
    json!({
        "id": id,
        "modId": mod_id,
        "displayName": name,
        "fileName": name,
        "releaseType": 1,
        "fileDate": format!("2026-09-{:02}T10:00:00Z", (id % 28) + 1),
        "fileLength": body.len(),
        "downloadUrl": url,
        "gameVersions": game_versions,
        "dependencies": dependencies,
        "hashes": [{"value": sha1_hex(body), "algo": 1}, {"value": "00", "algo": 2}],
        "fileFingerprint": module_curseforge::backend::fingerprint::fingerprint(body),
        "isAvailable": true
    })
}

#[allow(unused_imports)]
pub use world::*;

mod world {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use launcher_core::feedback::{EventSink, FeedbackService};
    use launcher_core::launch::options::game_dir;
    use launcher_core::loaders::{ComponentFuture, ComponentSource, ComponentSpec};
    use launcher_core::lock::Coordinator;
    use launcher_core::minecraft::InstallProgressFn;
    use launcher_core::minecraft::install::InstalledVersion;
    use launcher_core::net::downloader::{Downloader, DownloaderConfig};
    use launcher_core::storage::versions::{Build, VersionStore};
    use launcher_shared::provider::{InstallAnswer, InstallArgs, InstallOutcome};
    use launcher_shared::{ActivityEntry, Alert, AppError, AppResult, ErrorCode, OpsSnapshot, Text, Toast};
    use module_curseforge::backend::api::CurseForgeApi;
    use module_curseforge::backend::held::Elsewhere;
    use module_curseforge::backend::key::ApiKey;
    use module_curseforge::backend::service::{CurseForgeService, Deps};

    use super::{FakeCurseForge, TEST_KEY};

    /// What the service told the user.
    #[derive(Default)]
    pub struct Said(Mutex<Vec<Text>>);

    impl Said {
        pub fn keys(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|t| match t {
                    Text::Key { key, .. } => Some(key.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    impl EventSink for Said {
        fn ops(&self, _: &OpsSnapshot) {}
        fn activity(&self, _: &ActivityEntry) {}
        fn toast(&self, toast: &Toast) {
            self.0.lock().unwrap().push(toast.title.clone());
        }
        fn alert(&self, _: &Alert) {}
    }

    /// Installs components without the network: it only notes what was asked.
    #[derive(Default)]
    pub struct FakeComponents {
        pub asked: Mutex<Vec<String>>,
        pub fail: AtomicBool,
    }

    impl ComponentSource for FakeComponents {
        fn install<'a>(&'a self, spec: &'a ComponentSpec, _: InstallProgressFn<'a>) -> ComponentFuture<'a> {
            Box::pin(async move {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(AppError::new(ErrorCode::LoaderInstallFailed, "fake"));
                }
                let id = spec.component_id();
                self.asked.lock().unwrap().push(id.clone());
                Ok(InstalledVersion { id, java: Some(PathBuf::from(format!("java-for-{}", spec.mc))) })
            })
        }
    }

    pub struct World {
        pub tmp: tempfile::TempDir,
        pub versions: Arc<VersionStore>,
        pub instances: Arc<Coordinator>,
        pub service: Arc<CurseForgeService>,
        pub server: FakeCurseForge,
        pub said: Arc<Said>,
        /// The builds' games "run".
        pub running: Arc<AtomicBool>,
        pub components: Arc<FakeComponents>,
        /// The user's Downloads folder.
        pub downloads: PathBuf,
    }

    pub async fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
        std::fs::create_dir_all(&state).unwrap();
        let versions = Arc::new(VersionStore::open(&state, &mc));
        let instances = Arc::new(Coordinator::instances());
        let server = FakeCurseForge::start().await;
        let said = Arc::new(Said::default());
        let downloader =
            Downloader::new(DownloaderConfig { retries: 0, ..DownloaderConfig::default() }).unwrap();
        let running = Arc::new(AtomicBool::new(false));
        let downloads = tmp.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let components = Arc::new(FakeComponents::default());
        let flag = running.clone();
        let deps = Deps {
            versions: versions.clone(),
            instances: instances.clone(),
            feedback: FeedbackService::new(said.clone()),
            downloader: Arc::new(downloader),
            running: Arc::new(move |_: &str| flag.load(Ordering::SeqCst)),
            components: components.clone(),
            gpu_mode: Arc::new(|| "dgpu".to_string()),
            elsewhere: Elsewhere { modrinth: Some(server.base.clone()), downloads: Some(downloads.clone()) },
        };
        let key = ApiKey::new(TEST_KEY).unwrap();
        let api = CurseForgeApi::new(&server.base, key.clone()).unwrap();
        let service = Arc::new(CurseForgeService::new(api, deps, &key));
        World { tmp, versions, instances, service, server, said, running, components, downloads }
    }

    /// A Minecraft 1.21.1 build of `client` running `loader`, and its game folder.
    pub fn build(w: &World, name: &str, client: &str, loader: &str) -> (String, PathBuf) {
        let mut b = Build::new(name);
        b.client = Some(client.into());
        b.version = Some("1.21.1".into());
        b.loader = Some(loader.into());
        w.versions.create(&mut b).unwrap();
        let dir = game_dir(&b, w.versions.minecraft_dir());
        std::fs::create_dir_all(&dir).unwrap();
        (b.key, dir)
    }

    /// Plans `args`, approves the whole plan and installs it.
    pub async fn install(w: &World, args: InstallArgs) -> AppResult<InstallOutcome> {
        let plan = w.service.plan(&args).await?;
        let approved = InstallArgs {
            version_id: plan.main.as_ref().map(|m| m.version_id.clone()),
            approved: plan.changes(&[]),
            ..args
        };
        match w.service.install(&approved).await? {
            InstallAnswer::Installed(done) => Ok(done),
            InstallAnswer::Replanned(plan) => panic!("replanned: {plan:?}"),
        }
    }
}

/// A Fabric mod jar declaring mod id `id`.
pub fn mod_jar(id: &str) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(json!({"schemaVersion": 1, "id": id, "version": "1.0", "name": id}).to_string().as_bytes())
        .unwrap();
    zip.finish().unwrap().into_inner()
}

/// A modpack's zip: its manifest (Minecraft 1.20.1 on Forge 47.2.0, `files` as `[project, file]`)
/// and `entries` (overrides and the like).
pub fn pack_zip(files: &[(u64, u64)], entries: &[(&str, &[u8])], version: &str) -> Vec<u8> {
    pack_zip_on(files, entries, version, "1.20.1", "forge-47.2.0")
}

/// [`pack_zip`] for Minecraft `minecraft` with loader `loader` (a manifest's loader id).
pub fn pack_zip_on(
    files: &[(u64, u64)],
    entries: &[(&str, &[u8])],
    version: &str,
    minecraft: &str,
    loader: &str,
) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let listed: Vec<Value> = files
        .iter()
        .map(|(project, file)| json!({"projectID": project, "fileID": file, "required": true}))
        .collect();
    let manifest = json!({
        "minecraft": {"version": minecraft, "modLoaders": [{"id": loader, "primary": true}]},
        "manifestType": "minecraftModpack", "manifestVersion": 1, "name": "Big Pack", "version": version,
        "author": "Author", "files": listed, "overrides": "overrides"
    });
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("manifest.json", options).unwrap();
    zip.write_all(manifest.to_string().as_bytes()).unwrap();
    for (name, body) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Publishes modpack `id` ("Big Pack") with `files` (newest first) as `pack_file` makes them.
pub fn publish_pack(server: &FakeCurseForge, id: u64, files: Vec<Value>) {
    let mut project = mod_json(id, "Big Pack", MODPACKS, 500);
    project["links"]["websiteUrl"] =
        json!(format!("https://www.curseforge.com/minecraft/modpacks/big-pack-{id}"));
    server.publish(project, files);
}

/// File `id` of modpack `pack` carrying `zip`, for Minecraft `game` on `loader` (its tag).
pub fn pack_file(server: &FakeCurseForge, id: u64, pack: u64, zip: &[u8], game: &str, loader: &str) -> Value {
    let mut file =
        file_json(server, id, pack, &format!("big-pack-{id}.zip"), zip, &[game, loader], json!([]), false);
    file["displayName"] = json!(format!("Big Pack {id}"));
    file
}
