//! A local server builds API for the module's tests: each path answers from a queue of replies
//! (the last one repeats) and every request is remembered; and the module's services around it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use tokio::net::TcpListener;

#[derive(Default)]
struct Data {
    replies: HashMap<String, Vec<(u16, Vec<u8>)>>,
    headers: HashMap<String, Vec<(String, String)>>,
    seen: Vec<String>,
}

type Shared = Arc<Mutex<Data>>;

pub struct Server {
    pub origin: String,
    data: Shared,
    _task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub async fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let data = Shared::default();
        let app = Router::new().fallback(reply).with_state(data.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Server { origin, data, _task: task }
    }

    /// `path` answers `status` with `body`, after the replies queued before.
    pub fn reply(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.data.lock().unwrap().replies.entry(path.to_string()).or_default().push((status, body.into()));
    }

    /// `path` answers with header `name` too.
    pub fn header(&self, path: &str, name: &str, value: &str) {
        let mut data = self.data.lock().unwrap();
        let headers = data.headers.entry(path.to_string()).or_default();
        headers.retain(|(n, _)| n != name);
        headers.push((name.to_string(), value.to_string()));
    }

    pub fn json(&self, path: &str, body: Value) {
        self.reply(path, 200, body.to_string());
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    /// Path and query of every request, in order.
    pub fn seen(&self) -> Vec<String> {
        self.data.lock().unwrap().seen.clone()
    }
}

async fn reply(State(data): State<Shared>, uri: Uri) -> Response {
    let mut data = data.lock().unwrap();
    data.seen.push(uri.path_and_query().map(|p| p.to_string()).unwrap_or_default());
    let (status, body) = match data.replies.get_mut(uri.path()) {
        Some(queue) if queue.len() > 1 => queue.remove(0),
        Some(queue) if !queue.is_empty() => queue[0].clone(),
        _ => (404, b"{}".to_vec()),
    };
    let mut response =
        (StatusCode::from_u16(status).unwrap(), [(header::CONTENT_TYPE, "application/json")], body)
            .into_response();
    for (name, value) in data.headers.get(uri.path()).into_iter().flatten() {
        response.headers_mut().insert(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            header::HeaderValue::from_str(value).unwrap(),
        );
    }
    response
}

/// A component installer that installs nothing: it names the component and a Java for it, or
/// fails when told to.
#[derive(Default)]
pub struct FakeComponents {
    pub asked: Mutex<Vec<String>>,
    pub fail: std::sync::atomic::AtomicBool,
}

impl launcher_core::loaders::ComponentSource for FakeComponents {
    fn install<'a>(
        &'a self,
        spec: &'a launcher_core::loaders::ComponentSpec,
        _: launcher_core::minecraft::InstallProgressFn<'a>,
    ) -> launcher_core::loaders::ComponentFuture<'a> {
        Box::pin(async move {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(launcher_shared::AppError::new(
                    launcher_shared::ErrorCode::LoaderInstallFailed,
                    "fake",
                ));
            }
            let id = spec.component_id();
            self.asked.lock().unwrap().push(id.clone());
            Ok(launcher_core::minecraft::install::InstalledVersion {
                id,
                java: Some(std::path::PathBuf::from(format!("java-for-{}", spec.mc))),
            })
        })
    }
}

/// The alerts the module raised.
#[derive(Default)]
pub struct Alerts(Mutex<Vec<launcher_shared::Alert>>);

impl Alerts {
    pub fn list(&self) -> Vec<launcher_shared::Alert> {
        self.0.lock().unwrap().clone()
    }
}

impl launcher_core::feedback::EventSink for Alerts {
    fn ops(&self, _: &launcher_shared::OpsSnapshot) {}
    fn activity(&self, _: &launcher_shared::ActivityEntry) {}
    fn toast(&self, _: &launcher_shared::Toast) {}
    fn alert(&self, alert: &launcher_shared::Alert) {
        self.0.lock().unwrap().push(alert.clone());
    }
}

/// The module's services over a temporary Minecraft folder, the local API and fake components.
pub struct World {
    pub tmp: tempfile::TempDir,
    pub server: Server,
    pub components: Arc<FakeComponents>,
    pub alerts: Arc<Alerts>,
    pub deps: Arc<module_tensa::backend::service::Deps>,
}

impl World {
    pub async fn start() -> World {
        use std::time::Duration;

        use launcher_core::feedback::FeedbackService;
        use launcher_core::lock::Coordinator;
        use launcher_core::net::downloader::{Downloader, DownloaderConfig};
        use launcher_core::storage::versions::VersionStore;
        use module_tensa::backend::api::TensaApi;

        let tmp = tempfile::tempdir().unwrap();
        let (state, mc) = (tmp.path().join("state"), tmp.path().join("mc"));
        std::fs::create_dir_all(&state).unwrap();
        let server = Server::start().await;
        let components = Arc::new(FakeComponents::default());
        let alerts = Arc::new(Alerts::default());
        let downloader = Downloader::new(DownloaderConfig {
            retry_delay: Duration::from_millis(1),
            timeout: Duration::from_secs(5),
            ..DownloaderConfig::default()
        })
        .unwrap();
        let deps = Arc::new(module_tensa::backend::service::Deps {
            api: Arc::new(
                TensaApi::new(&server.url("/api/mods")).unwrap().with_retry_delay(Duration::from_millis(1)),
            ),
            versions: Arc::new(VersionStore::open(&state, &mc)),
            instances: Arc::new(Coordinator::instances()),
            feedback: FeedbackService::new(alerts.clone()),
            downloader: Arc::new(downloader),
            components: components.clone(),
            gpu_mode: Arc::new(|| "dgpu"),
            running: Arc::new(|_: &str| false),
        });
        World { tmp, server, components, alerts, deps }
    }

    /// The catalog lists `client` as its one server build.
    pub fn catalog(&self, client: Value) {
        self.server.json("/api/mods", serde_json::json!([{"client": client}]));
    }

    /// Serves `body` as `/files/<name>`; its address.
    pub fn file(&self, name: &str, body: &str) -> String {
        self.server.reply(&format!("/files/{name}"), 200, body.as_bytes().to_vec());
        self.server.url(&format!("/files/{name}"))
    }
}
