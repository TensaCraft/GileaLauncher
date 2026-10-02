//! A local Modrinth for the module's tests — search, projects, versions, `/version_files` and
//! `/files/{name}`, remembering every request and its User-Agent — and the service around it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_core::launch::options::game_dir;
use launcher_core::loaders::ComponentSpec;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::InstallProgressFn;
use launcher_core::minecraft::install::InstalledVersion;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::provider::{InstallAnswer, InstallArgs, InstallOutcome};
use launcher_shared::{
    ActivityEntry, Alert, AppError, AppResult, ContentKind, ErrorCode, OpsSnapshot, Text, Toast,
};
use module_modrinth::backend::api::ModrinthApi;
use module_modrinth::backend::components::{ComponentFuture, ComponentSource};
use module_modrinth::backend::service::{Deps, ModrinthService};
use module_modrinth::types::BuildArgs;
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use tokio::net::TcpListener;

#[derive(Default)]
pub struct Data {
    /// `/search` answers this, or a body that is not JSON when `None`.
    pub search: Option<Value>,
    /// `/search` answers this status instead.
    pub search_status: Option<u16>,
    pub versions: HashMap<String, Value>,
    /// `/version/{id}`.
    pub version_ids: HashMap<String, Value>,
    /// `/project/{id}`.
    pub projects: HashMap<String, Value>,
    /// `/version_files`: SHA-512 → version.
    pub hashes: HashMap<String, Value>,
    /// `/version_files` answers this status instead.
    pub hashes_status: Option<u16>,
    /// `/version_files` rewrites this file first (a user changing mods while the launcher plans).
    pub touch_on_identify: Option<std::path::PathBuf>,
    pub files: HashMap<String, Vec<u8>>,
    /// `/version_files/update`: SHA-512 → the newest version.
    pub updates: HashMap<String, Value>,
    /// `/version_files/update` answers this status instead.
    pub updates_status: Option<u16>,
    /// Every `/version_files/update` body.
    pub update_bodies: Vec<Value>,
    /// `/projects` answers this status instead.
    pub projects_status: Option<u16>,
    /// Path and query of every request, with its User-Agent.
    pub seen: Vec<(String, String)>,
}

type Shared = Arc<Mutex<Data>>;

pub struct FakeModrinth {
    /// The API root (`…/v2`).
    pub base: String,
    pub origin: String,
    pub data: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeModrinth {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn note(data: &Shared, uri: &Uri, headers: &HeaderMap) {
    let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    data.lock().unwrap().seen.push((uri.to_string(), agent));
}

async fn search(State(data): State<Shared>, uri: Uri, headers: HeaderMap) -> Response {
    note(&data, &uri, &headers);
    let d = data.lock().unwrap();
    if let Some(status) = d.search_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    match &d.search {
        Some(value) => axum::Json(value.clone()).into_response(),
        None => (StatusCode::OK, "<html>not json</html>").into_response(),
    }
}

async fn versions(
    State(data): State<Shared>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    note(&data, &uri, &headers);
    match data.lock().unwrap().versions.get(&id) {
        Some(value) => axum::Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A file at Modrinth's CDN address (`/data/<project>/versions/<version>/<name>`).
async fn cdn_file(
    State(data): State<Shared>,
    Path((_, _, name)): Path<(String, String, String)>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    note(&data, &uri, &headers);
    match data.lock().unwrap().files.get(&name) {
        Some(body) => body.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn file(
    State(data): State<Shared>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    note(&data, &uri, &headers);
    match data.lock().unwrap().files.get(&name) {
        Some(body) => body.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn version_by_id(
    State(data): State<Shared>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    note(&data, &uri, &headers);
    match data.lock().unwrap().version_ids.get(&id) {
        Some(value) => axum::Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /projects?ids=[…]`: the projects it knows, in any order.
async fn projects(State(data): State<Shared>, uri: Uri, headers: HeaderMap) -> Response {
    note(&data, &uri, &headers);
    let d = data.lock().unwrap();
    if let Some(status) = d.projects_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let ids: Vec<String> = uri
        .query()
        .map(|q| format!("/x?{q}"))
        .map(|q| pairs(&q))
        .unwrap_or_default()
        .into_iter()
        .find(|(k, _)| k == "ids")
        .and_then(|(_, v)| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let found: Vec<Value> = ids.iter().filter_map(|id| d.projects.get(id).cloned()).collect();
    axum::Json(Value::Array(found)).into_response()
}

async fn project(
    State(data): State<Shared>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    note(&data, &uri, &headers);
    match data.lock().unwrap().projects.get(&id) {
        Some(value) => axum::Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn version_files(
    State(data): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    note(&data, &uri, &headers);
    let d = data.lock().unwrap();
    if let Some(status) = d.hashes_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if let Some(path) = &d.touch_on_identify {
        std::fs::write(path, b"edited meanwhile").unwrap();
    }
    let mut answer = serde_json::Map::new();
    for hash in body["hashes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(version) = d.hashes.get(hash) {
            answer.insert(hash.to_string(), version.clone());
        }
    }
    axum::Json(Value::Object(answer)).into_response()
}

async fn version_files_update(
    State(data): State<Shared>,
    uri: Uri,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    note(&data, &uri, &headers);
    let mut d = data.lock().unwrap();
    d.update_bodies.push(body.clone());
    if let Some(status) = d.updates_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let mut answer = serde_json::Map::new();
    for hash in body["hashes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(version) = d.updates.get(hash) {
            answer.insert(hash.to_string(), version.clone());
        }
    }
    axum::Json(Value::Object(answer)).into_response()
}

impl FakeModrinth {
    pub async fn start() -> FakeModrinth {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let data = Shared::default();
        let app = Router::new()
            .route("/v2/search", get(search))
            .route("/v2/project/{id}/version", get(versions))
            .route("/v2/project/{id}", get(project))
            .route("/v2/projects", get(projects))
            .route("/v2/version/{id}", get(version_by_id))
            .route("/v2/version_files", post(version_files))
            .route("/v2/version_files/update", post(version_files_update))
            .route("/files/{name}", get(file))
            .route("/data/{project}/versions/{version}/{name}", get(cdn_file))
            .with_state(data.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeModrinth { base: format!("{origin}/v2"), origin, data, task }
    }

    /// Serves `body` as `/files/<name>`; its address.
    pub fn file(&self, name: &str, body: &[u8]) -> String {
        self.data.lock().unwrap().files.insert(name.to_string(), body.to_vec());
        format!("{}/files/{name}", self.origin)
    }

    pub fn seen(&self) -> Vec<(String, String)> {
        self.data.lock().unwrap().seen.clone()
    }
}

pub fn sha512(body: &[u8]) -> String {
    hex::encode(Sha512::digest(body))
}

/// A version of `project` made for `game_versions` × `loaders`.
pub fn version(
    id: &str,
    project: &str,
    number: &str,
    game_versions: &[&str],
    loaders: &[&str],
    files: Vec<Value>,
) -> Value {
    json!({"id": id, "project_id": project, "version_number": number, "game_versions": game_versions,
           "loaders": loaders, "date_published": "2026-09-01T00:00:00Z", "files": files})
}

/// A version file with the right size and SHA-512 of `body`.
pub fn file_entry(url: &str, filename: &str, body: &[u8], primary: bool) -> Value {
    json!({"url": url, "filename": filename, "size": body.len(), "primary": primary,
           "hashes": {"sha512": sha512(body), "sha1": "0".repeat(40)}})
}

/// The decoded query of a seen `path?query`.
pub fn pairs(seen: &str) -> Vec<(String, String)> {
    reqwest::Url::parse(&format!("http://x{seen}"))
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

impl FakeModrinth {
    /// Publishes `project` with `versions` (newest first, as Modrinth lists them).
    pub fn publish(&self, project: Value, versions: Vec<Value>) {
        let id = project["id"].as_str().unwrap().to_string();
        let mut d = self.data.lock().unwrap();
        for version in &versions {
            d.version_ids.insert(version["id"].as_str().unwrap().to_string(), version.clone());
        }
        d.versions.insert(id.clone(), Value::Array(versions));
        d.projects.insert(id, project);
    }

    /// Modrinth knows `body` as a file of `version` (`/version_files`).
    pub fn know(&self, body: &[u8], version: &Value) {
        self.data.lock().unwrap().hashes.insert(sha512(body), version.clone());
    }

    /// Modrinth offers `newer` for the file `body` (`/version_files/update`).
    pub fn offer(&self, body: &[u8], newer: &Value) {
        self.data.lock().unwrap().updates.insert(sha512(body), newer.clone());
    }
}

/// A Fabric mod jar whose `fabric.mod.json` says `id` and `name`; `salt` changes its bytes.
pub fn mod_jar(id: &str, name: &str, salt: &str) -> Vec<u8> {
    use std::io::Write;
    let mut jar = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    jar.start_file("fabric.mod.json", options).unwrap();
    let meta = json!({"schemaVersion": 1, "id": id, "name": name, "version": salt});
    jar.write_all(meta.to_string().as_bytes()).unwrap();
    jar.start_file("salt.txt", options).unwrap();
    jar.write_all(salt.as_bytes()).unwrap();
    jar.finish().unwrap().into_inner()
}

/// [`release`] whose one primary file `<id>.jar` is a Fabric mod jar of `mod_id` (salted by `id`).
pub fn release_jar(
    server: &FakeModrinth,
    project: &str,
    id: &str,
    mod_id: &str,
    day: u32,
    dependencies: Value,
) -> Value {
    let name = format!("{id}.jar");
    let body = mod_jar(mod_id, mod_id, id);
    let url = server.file(&name, &body);
    json!({"id": id, "project_id": project, "version_number": id, "game_versions": ["1.21.1"], "loaders": ["fabric"],
           "date_published": format!("2026-09-{day:02}T00:00:00Z"), "dependencies": dependencies,
           "files": [file_entry(&url, &name, &body, true)]})
}

/// A resource or shader pack version `id` of `project`, published on `day`, whose one primary file
/// `<id>.zip` holds the bytes of `id`.
pub fn pack_release(server: &FakeModrinth, project: &str, id: &str, loaders: &[&str], day: u32) -> Value {
    let name = format!("{id}.zip");
    let url = server.file(&name, id.as_bytes());
    json!({"id": id, "project_id": project, "version_number": id, "game_versions": ["1.21.1"], "loaders": loaders,
           "date_published": format!("2026-09-{day:02}T00:00:00Z"), "dependencies": [],
           "files": [file_entry(&url, &name, id.as_bytes(), true)]})
}

pub fn project_json(id: &str, title: &str) -> Value {
    json!({"id": id, "slug": id.to_lowercase(), "title": title, "project_type": "mod"})
}

/// A version `id` of `project` for Minecraft 1.21.1 on `loaders`, published on `day` of September
/// 2026, whose one primary file `<id>.jar` holds the bytes of `id`.
pub fn release(
    server: &FakeModrinth,
    project: &str,
    id: &str,
    loaders: &[&str],
    day: u32,
    dependencies: Value,
) -> Value {
    let name = format!("{id}.jar");
    let url = server.file(&name, id.as_bytes());
    json!({"id": id, "project_id": project, "version_number": id, "game_versions": ["1.21.1"], "loaders": loaders,
           "date_published": format!("2026-09-{day:02}T00:00:00Z"), "dependencies": dependencies,
           "files": [file_entry(&url, &name, id.as_bytes(), true)]})
}

/// Keeps the toasts the service shows.
#[derive(Default)]
pub struct Said(Mutex<Vec<Text>>);

impl Said {
    /// The keys of the toasts so far.
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
            // The Java the launcher would pick for this Minecraft.
            Ok(InstalledVersion { id, java: Some(std::path::PathBuf::from(format!("java-for-{}", spec.mc))) })
        })
    }
}

/// Publishes modpack project `id` whose version `vid` carries `mrpack` (the bytes of a .mrpack).
pub fn publish_pack(server: &FakeModrinth, id: &str, vid: &str, mrpack: &[u8]) {
    publish_pack_for(server, id, vid, mrpack, &["1.21.1"], &["fabric"]);
}

/// [`publish_pack`] for other Minecraft versions and loaders.
pub fn publish_pack_for(
    server: &FakeModrinth,
    id: &str,
    vid: &str,
    mrpack: &[u8],
    games: &[&str],
    loaders: &[&str],
) {
    let url = server.file(&format!("{vid}.mrpack"), mrpack);
    let version = json!({"id": vid, "project_id": id, "version_number": vid, "game_versions": games,
        "loaders": loaders, "date_published": "2026-09-01T00:00:00Z",
        "files": [{"url": url, "filename": format!("{vid}.mrpack"), "size": mrpack.len(), "primary": true,
                   "hashes": {"sha512": sha512(mrpack)}}]});
    let project = json!({"id": id, "slug": id.to_lowercase(), "title": id, "project_type": "modpack"});
    // The newest first, as Modrinth lists them; publishing a version again replaces it.
    let mut versions: Vec<Value> =
        server.data.lock().unwrap().versions.get(id).and_then(Value::as_array).cloned().unwrap_or_default();
    versions.retain(|v| v["id"] != json!(vid));
    versions.insert(0, version);
    server.publish(project, versions);
}

/// A `.mrpack` of version `version_number` with `files` (as `mod_file` makes them) and
/// `entries` (overrides and the like): Minecraft 1.21.1 with Fabric 0.16.9.
pub fn mrpack_of(files: Vec<Value>, entries: &[(&str, &[u8])], version_number: &str) -> Vec<u8> {
    mrpack_on(files, entries, version_number, "1.21.1", "0.16.9")
}

/// [`mrpack_of`] for Minecraft `minecraft` with Fabric Loader `fabric`.
pub fn mrpack_on(
    files: Vec<Value>,
    entries: &[(&str, &[u8])],
    version_number: &str,
    minecraft: &str,
    fabric: &str,
) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let index = json!({"formatVersion": 1, "game": "minecraft", "versionId": version_number, "name": "Speedy",
        "summary": "Fast and light", "files": files,
        "dependencies": {"minecraft": minecraft, "fabric-loader": fabric}});
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(module_modrinth::backend::pack::INDEX, zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(index.to_string().as_bytes()).unwrap();
    for (name, body) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// [`pack_mod`] served from Modrinth's CDN address of `project` (as Modrinth's own packs list them).
pub fn pack_mod_of(server: &FakeModrinth, project: &str, name: &str, body: &[u8]) -> Value {
    server.file(name, body);
    let url = format!("{}/data/{project}/versions/v1/{name}", server.origin);
    json!({"path": format!("mods/{name}"), "downloads": [url], "fileSize": body.len(),
           "hashes": {"sha512": sha512(body)}, "env": {"client": "required", "server": "required"}})
}

/// A pack's download entry for `mods/<name>` served by `server`.
pub fn pack_mod(server: &FakeModrinth, name: &str, body: &[u8]) -> Value {
    json!({"path": format!("mods/{name}"), "downloads": [server.file(name, body)], "fileSize": body.len(),
           "hashes": {"sha512": sha512(body)}, "env": {"client": "required", "server": "required"}})
}

pub struct World {
    pub tmp: tempfile::TempDir,
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub service: Arc<ModrinthService>,
    pub server: FakeModrinth,
    /// The build's game "runs".
    pub running: Arc<AtomicBool>,
    /// What the service told the user.
    pub said: Arc<Said>,
    pub feedback: Arc<FeedbackService>,
    pub components: Arc<FakeComponents>,
}

pub async fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
    std::fs::create_dir_all(&state).unwrap();
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let instances = Arc::new(Coordinator::instances());
    let server = FakeModrinth::start().await;
    let running = Arc::new(AtomicBool::new(false));
    let flag = running.clone();
    let said = Arc::new(Said::default());
    let feedback = FeedbackService::new(said.clone());
    let components = Arc::new(FakeComponents::default());
    let downloader = Downloader::new(DownloaderConfig { retries: 0, ..DownloaderConfig::default() }).unwrap();
    let deps = Deps {
        versions: versions.clone(),
        instances: instances.clone(),
        feedback: feedback.clone(),
        components: components.clone(),
        gpu_mode: Arc::new(|| "dgpu".to_string()),
        downloader: Arc::new(downloader),
        running: Arc::new(move |_: &str| flag.load(Ordering::SeqCst)),
    };
    let service = Arc::new(ModrinthService::new(ModrinthApi::new(&server.base).unwrap(), deps));
    World { tmp, versions, instances, service, server, running, said, feedback, components }
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

pub async fn installed(w: &World, key: &str, kind: ContentKind) -> Vec<String> {
    w.service.installed(&BuildArgs { key: key.into(), kind }).await.unwrap()
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
