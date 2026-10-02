//! Local imitation of the GitHub Releases API for testing the launcher updater.

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const REPO: &str = "mock/launcher";
pub const DEFAULT_PORT: u16 = 1430;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Normal,
    /// ~200 KiB/s so the progress is visible.
    Slow,
    /// The first full download of each file is cut in the middle.
    Drop,
    /// Listings report a digest that does not match the file.
    BadHash,
    /// 403 with GitHub rate-limit headers.
    RateLimit,
    /// 500 on every API call.
    ServerError,
    /// Release listings have no assets; the file is on page 2 of the assets endpoint.
    PagedAssets,
    /// Adds a draft `v100.0.0` and a pre-release `v99.0.0-beta.1`.
    Channels,
}

impl Scenario {
    pub const NAMES: [&'static str; 8] =
        ["normal", "slow", "drop", "bad-hash", "rate-limit", "server-error", "paged-assets", "channels"];
}

impl FromStr for Scenario {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "normal" => Scenario::Normal,
            "slow" => Scenario::Slow,
            "drop" => Scenario::Drop,
            "bad-hash" => Scenario::BadHash,
            "rate-limit" => Scenario::RateLimit,
            "server-error" => Scenario::ServerError,
            "paged-assets" => Scenario::PagedAssets,
            "channels" => Scenario::Channels,
            other => {
                return Err(format!("unknown scenario '{other}' (known: {})", Scenario::NAMES.join(", ")));
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAsset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub state: String,
    pub digest: String,
    pub content_type: String,
    /// Path relative to `root/files/`; never sent to clients.
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRelease {
    pub id: u64,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
    pub published_at: String,
    pub assets: Vec<StoredAsset>,
}

pub struct NewRelease<'a> {
    pub version: &'a str,
    pub prerelease: bool,
    pub notes: &'a str,
    pub asset_name: &'a str,
    pub source: &'a Path,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn load_releases(root: &Path) -> io::Result<Vec<StoredRelease>> {
    match std::fs::read(root.join("releases.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Copies `source` into `root/files/<version>/<asset_name>` and records the release (newest first).
pub fn add_release(root: &Path, new: NewRelease<'_>) -> io::Result<StoredRelease> {
    let mut releases = load_releases(root)?;
    let bytes = std::fs::read(new.source)?;
    let dir = root.join("files").join(new.version);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(new.asset_name), &bytes)?;
    let release_id = releases.iter().map(|r| r.id).max().unwrap_or(1000) + 1;
    let asset_id = releases.iter().flat_map(|r| r.assets.iter().map(|a| a.id)).max().unwrap_or(5000) + 1;
    let release = StoredRelease {
        id: release_id,
        tag_name: format!("v{}", new.version),
        name: format!("Launcher {}", new.version),
        body: new.notes.to_string(),
        draft: false,
        prerelease: new.prerelease,
        published_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        assets: vec![StoredAsset {
            id: asset_id,
            name: new.asset_name.to_string(),
            size: bytes.len() as u64,
            state: "uploaded".into(),
            digest: format!("sha256:{}", sha256_hex(&bytes)),
            content_type: "application/octet-stream".into(),
            file: format!("{}/{}", new.version, new.asset_name),
        }],
    };
    releases.retain(|r| r.tag_name != release.tag_name);
    releases.insert(0, release.clone());
    std::fs::create_dir_all(root)?;
    std::fs::write(
        root.join("releases.json"),
        serde_json::to_vec_pretty(&releases).map_err(io::Error::other)?,
    )?;
    Ok(release)
}

struct ServerState {
    root: PathBuf,
    scenario: Scenario,
    base_url: String,
    dropped: Mutex<HashSet<u64>>,
}

type Shared = Arc<ServerState>;

pub struct MockHandle {
    pub base_url: String,
    pub addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockHandle {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = (&mut self.task).await;
    }
}

pub async fn start(root: PathBuf, scenario: Scenario, port: u16) -> io::Result<MockHandle> {
    start_inner(root, scenario, port, false).await
}

/// Same as [`start`], printing every request and its status (`cargo xtask mock-releases serve`).
pub async fn start_logged(root: PathBuf, scenario: Scenario, port: u16) -> io::Result<MockHandle> {
    start_inner(root, scenario, port, true).await
}

async fn log_request(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let line = format!("{} {}", req.method(), req.uri());
    let resp = next.run(req).await;
    println!("{line} -> {}", resp.status().as_u16());
    resp
}

async fn start_inner(root: PathBuf, scenario: Scenario, port: u16, log: bool) -> io::Result<MockHandle> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let state = Arc::new(ServerState {
        root,
        scenario,
        base_url: base_url.clone(),
        dropped: Mutex::new(HashSet::new()),
    });
    let mut app = Router::new()
        .route("/repos/{owner}/{repo}/releases", get(list_releases))
        .route("/repos/{owner}/{repo}/releases/{id}/assets", get(list_assets))
        .route("/download/{release_id}/{name}", get(download))
        .with_state(state);
    if log {
        app = app.layer(axum::middleware::from_fn(log_request));
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    Ok(MockHandle { base_url, addr, shutdown: Some(tx), task })
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": "Not Found", "documentation_url": "https://docs.github.com/rest"})),
    )
        .into_response()
}

/// Wrong repository and the error scenarios answer before any data is read.
fn guard(state: &ServerState, owner: &str, repo: &str) -> Option<Response> {
    if format!("{owner}/{repo}") != REPO {
        return Some(not_found());
    }
    match state.scenario {
        Scenario::RateLimit => {
            let reset = (SystemTime::now() + Duration::from_secs(600))
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let body = json!({
                "message": "API rate limit exceeded for 127.0.0.1.",
                "documentation_url": "https://docs.github.com/rest/overview/resources-in-the-rest-api#rate-limiting"
            });
            let mut resp = (StatusCode::FORBIDDEN, Json(body)).into_response();
            let h = resp.headers_mut();
            h.insert("x-ratelimit-limit", HeaderValue::from_static("60"));
            h.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
            h.insert("x-ratelimit-reset", HeaderValue::from(reset));
            Some(resp)
        }
        Scenario::ServerError => Some(
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"message": "Server Error"}))).into_response(),
        ),
        _ => None,
    }
}

fn effective_releases(state: &ServerState) -> Vec<StoredRelease> {
    let mut list = load_releases(&state.root).unwrap_or_default();
    if state.scenario == Scenario::Channels
        && let Some(top) = list.first().cloned()
    {
        let mut draft = top.clone();
        draft.id = top.id + 100_000;
        draft.tag_name = "v100.0.0".into();
        draft.name = "Draft 100.0.0".into();
        draft.draft = true;
        let mut beta = top.clone();
        beta.id = top.id + 200_000;
        beta.tag_name = "v99.0.0-beta.1".into();
        beta.name = "Launcher 99.0.0 beta 1".into();
        beta.prerelease = true;
        list.insert(0, beta);
        list.insert(0, draft);
    }
    list
}

fn asset_json(state: &ServerState, release_id: u64, a: &StoredAsset) -> Value {
    let digest = if state.scenario == Scenario::BadHash {
        format!("sha256:{}", "0".repeat(64))
    } else {
        a.digest.clone()
    };
    json!({
        "id": a.id,
        "name": a.name,
        "size": a.size,
        "state": a.state,
        "digest": digest,
        "content_type": a.content_type,
        "browser_download_url": format!("{}/download/{}/{}", state.base_url, release_id, a.name),
    })
}

fn release_json(state: &ServerState, r: &StoredRelease) -> Value {
    let assets: Vec<Value> = if state.scenario == Scenario::PagedAssets {
        Vec::new()
    } else {
        r.assets.iter().map(|a| asset_json(state, r.id, a)).collect()
    };
    json!({
        "id": r.id,
        "tag_name": r.tag_name,
        "name": r.name,
        "body": r.body,
        "draft": r.draft,
        "prerelease": r.prerelease,
        "published_at": r.published_at,
        "assets": assets,
    })
}

async fn list_releases(
    State(state): State<Shared>,
    UrlPath((owner, repo)): UrlPath<(String, String)>,
) -> Response {
    if let Some(resp) = guard(&state, &owner, &repo) {
        return resp;
    }
    let list: Vec<Value> = effective_releases(&state).iter().map(|r| release_json(&state, r)).collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
    per_page: Option<usize>,
}

async fn list_assets(
    State(state): State<Shared>,
    UrlPath((owner, repo, id)): UrlPath<(String, String, u64)>,
    Query(q): Query<PageQuery>,
) -> Response {
    if let Some(resp) = guard(&state, &owner, &repo) {
        return resp;
    }
    let Some(release) = effective_releases(&state).into_iter().find(|r| r.id == id) else {
        return not_found();
    };
    let per_page = q.per_page.unwrap_or(30).clamp(1, 100);
    let page = q.page.unwrap_or(1).max(1);
    let mut all: Vec<Value> = Vec::new();
    if state.scenario == Scenario::PagedAssets {
        for n in 0..per_page {
            all.push(json!({
                "id": 900_000 + n,
                "name": format!("Launcher-docs-{n}.txt"),
                "size": 1,
                "state": "uploaded",
                "digest": format!("sha256:{}", "1".repeat(64)),
                "content_type": "text/plain",
                "browser_download_url": format!("{}/download/{}/docs-{n}.txt", state.base_url, id),
            }));
        }
    }
    all.extend(release.assets.iter().map(|a| asset_json(&state, release.id, a)));
    let slice: Vec<Value> = all.into_iter().skip((page - 1) * per_page).take(per_page).collect();
    Json(slice).into_response()
}

async fn download(
    State(state): State<Shared>,
    UrlPath((release_id, name)): UrlPath<(u64, String)>,
    headers: HeaderMap,
) -> Response {
    let asset = effective_releases(&state)
        .into_iter()
        .find(|r| r.id == release_id)
        .and_then(|r| r.assets.into_iter().find(|a| a.name == name));
    let Some(asset) = asset else {
        return not_found();
    };
    let Ok(bytes) = std::fs::read(state.root.join("files").join(&asset.file)) else {
        return not_found();
    };
    let total = bytes.len() as u64;
    let start = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<u64>().ok());
    let (status, from) = match start {
        Some(s) if s < total => (StatusCode::PARTIAL_CONTENT, s),
        Some(_) => return StatusCode::RANGE_NOT_SATISFIABLE.into_response(),
        None => (StatusCode::OK, 0),
    };
    let body = Bytes::from(bytes).slice(from as usize..);
    let len = body.len();
    let drop_now = state.scenario == Scenario::Drop
        && from == 0
        && state.dropped.lock().unwrap_or_else(|e| e.into_inner()).insert(asset.id);
    let slow = state.scenario == Scenario::Slow;
    let cut = if drop_now { len / 2 } else { len };
    const CHUNK: usize = 64 * 1024;
    let stream = futures_util::stream::unfold(0usize, move |pos| {
        let body = body.clone();
        async move {
            if pos >= cut {
                if !(drop_now && pos < len) {
                    return None;
                }
                // Let hyper flush the headers and the first half before the connection is cut,
                // otherwise the client sees no response at all instead of a truncated body.
                tokio::time::sleep(Duration::from_millis(200)).await;
                return Some((Err::<Bytes, io::Error>(io::Error::other("connection dropped by mock")), len));
            }
            if slow {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let end = (pos + CHUNK).min(cut);
            Some((Ok(body.slice(pos..end)), end))
        }
    });
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len as u64));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {from}-{}/{total}", total - 1))
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::new()
    }

    async fn get_json(url: String) -> (u16, Value) {
        let resp = client().get(url).send().await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.bytes().await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn fixture(scenario: Scenario) -> (tempfile::TempDir, MockHandle, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let src = dir.path().join("payload.bin");
        std::fs::write(&src, &payload).unwrap();
        let new = NewRelease {
            version: "0.2.0",
            prerelease: false,
            notes: "Fixes",
            asset_name: "Launcher.exe",
            source: &src,
        };
        add_release(dir.path(), new).unwrap();
        let server = start(dir.path().to_path_buf(), scenario, 0).await.unwrap();
        (dir, server, payload)
    }

    #[tokio::test]
    async fn serves_github_shaped_releases_and_404_for_other_repos() {
        let (_dir, server, payload) = fixture(Scenario::Normal).await;
        let (status, list) =
            get_json(format!("{}/repos/mock/launcher/releases?per_page=100", server.base_url)).await;
        assert_eq!(status, 200);
        assert_eq!(list[0]["tag_name"], "v0.2.0");
        assert_eq!(list[0]["draft"], false);
        let asset = &list[0]["assets"][0];
        assert_eq!(asset["size"], payload.len());
        assert_eq!(asset["state"], "uploaded");
        assert_eq!(asset["digest"], format!("sha256:{}", sha256_hex(&payload)));
        assert!(asset.get("file").is_none());
        let url = asset["browser_download_url"].as_str().unwrap().to_string();
        let body = client().get(url).send().await.unwrap().bytes().await.unwrap();
        assert_eq!(body.as_ref(), payload.as_slice());
        let (status, err) = get_json(format!("{}/repos/other/repo/releases", server.base_url)).await;
        assert_eq!(status, 404);
        assert_eq!(err["message"], "Not Found");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn range_requests_get_the_tail_with_206() {
        let (dir, server, payload) = fixture(Scenario::Normal).await;
        let url =
            format!("{}/download/{}/Launcher.exe", server.base_url, load_releases(dir.path()).unwrap()[0].id);
        let resp = client().get(url).header("Range", "bytes=1000-").send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 206);
        let range = resp.headers()["content-range"].to_str().unwrap().to_string();
        assert_eq!(range, format!("bytes 1000-{}/{}", payload.len() - 1, payload.len()));
        assert_eq!(resp.bytes().await.unwrap().as_ref(), &payload[1000..]);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn drop_scenario_cuts_the_first_download_only() {
        let (dir, server, payload) = fixture(Scenario::Drop).await;
        let url =
            format!("{}/download/{}/Launcher.exe", server.base_url, load_releases(dir.path()).unwrap()[0].id);
        let first = client().get(&url).send().await.unwrap().bytes().await;
        assert!(first.is_err(), "the first download must be cut");
        let second = client().get(&url).send().await.unwrap().bytes().await.unwrap();
        assert_eq!(second.as_ref(), payload.as_slice());
        server.shutdown().await;
    }

    #[tokio::test]
    async fn rate_limit_scenario_sends_github_headers() {
        let (_dir, server, _) = fixture(Scenario::RateLimit).await;
        let resp =
            client().get(format!("{}/repos/mock/launcher/releases", server.base_url)).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 403);
        assert_eq!(resp.headers()["x-ratelimit-remaining"], "0");
        let reset: u64 = resp.headers()["x-ratelimit-reset"].to_str().unwrap().parse().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(reset > now);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn paged_assets_scenario_hides_the_file_until_page_two() {
        let (_dir, server, _) = fixture(Scenario::PagedAssets).await;
        let (_, list) = get_json(format!("{}/repos/mock/launcher/releases", server.base_url)).await;
        assert_eq!(list[0]["assets"].as_array().unwrap().len(), 0);
        let id = list[0]["id"].as_u64().unwrap();
        let page = |n: u32| {
            format!("{}/repos/mock/launcher/releases/{id}/assets?per_page=100&page={n}", server.base_url)
        };
        let (_, first) = get_json(page(1)).await;
        assert_eq!(first.as_array().unwrap().len(), 100);
        assert!(first.as_array().unwrap().iter().all(|a| a["name"] != "Launcher.exe"));
        let (_, second) = get_json(page(2)).await;
        assert!(second.as_array().unwrap().iter().any(|a| a["name"] == "Launcher.exe"));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn channels_scenario_adds_a_draft_and_a_newer_beta() {
        let (_dir, server, _) = fixture(Scenario::Channels).await;
        let (_, list) = get_json(format!("{}/repos/mock/launcher/releases", server.base_url)).await;
        let list = list.as_array().unwrap();
        assert!(list.iter().any(|r| r["tag_name"] == "v100.0.0" && r["draft"] == true));
        assert!(list.iter().any(|r| r["tag_name"] == "v99.0.0-beta.1" && r["prerelease"] == true));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn bad_hash_scenario_reports_a_wrong_digest() {
        let (_dir, server, payload) = fixture(Scenario::BadHash).await;
        let (_, list) = get_json(format!("{}/repos/mock/launcher/releases", server.base_url)).await;
        assert_ne!(list[0]["assets"][0]["digest"], format!("sha256:{}", sha256_hex(&payload)));
        server.shutdown().await;
    }

    #[test]
    fn add_release_replaces_the_same_tag_and_keeps_ids_unique() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.bin");
        std::fs::write(&src, b"one").unwrap();
        let new =
            |v| NewRelease { version: v, prerelease: false, notes: "", asset_name: "Launcher", source: &src };
        let first = add_release(dir.path(), new("0.2.0")).unwrap();
        let again = add_release(dir.path(), new("0.2.0")).unwrap();
        let other = add_release(dir.path(), new("0.3.0")).unwrap();
        let list = load_releases(dir.path()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].tag_name, "v0.3.0");
        assert!(again.id > first.id && other.id > again.id);
        assert_eq!(other.assets[0].size, 3);
        assert_eq!(other.assets[0].digest, format!("sha256:{}", sha256_hex(b"one")));
    }

    #[test]
    fn scenario_names_round_trip() {
        for name in Scenario::NAMES {
            assert!(name.parse::<Scenario>().is_ok(), "{name}");
        }
        assert!("nope".parse::<Scenario>().is_err());
    }
}
