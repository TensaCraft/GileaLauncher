//! A local file host for downloader tests: `GET /files/{name}` with `Range`/`If-Range`, ETags and
//! failure knobs.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt;
use tokio::net::TcpListener;

#[derive(Debug, Clone, Default)]
pub struct Served {
    pub body: Vec<u8>,
    pub etag: Option<String>,
    /// Answer 503 this many times first.
    pub fail_times: u32,
    /// Answer with this status instead of the file.
    pub status: Option<u16>,
    /// The first full answer breaks off half-way.
    pub drop_first: bool,
    /// Right after that broken answer the file changes to this body and ETag.
    pub replace_after_drop: Option<(Vec<u8>, String)>,
    /// Range answers claim to start one byte late.
    pub bad_range: bool,
    /// Range requests are answered 206 even when `If-Range` no longer matches.
    pub ignore_if_range: bool,
    /// Range requests are answered with this status.
    pub range_status: Option<u16>,
    /// Answer `302 Found` to this address.
    pub redirect_to: Option<String>,
    /// Answer after this pause, as a far server would.
    pub delay: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub name: String,
    pub range: Option<String>,
    pub if_range: Option<String>,
    /// The `x-test-key` header: a stand-in for a host's API key.
    pub key: Option<String>,
}

#[derive(Default)]
struct Files {
    served: HashMap<String, Served>,
    seen: Vec<Seen>,
}

pub struct FileServer {
    pub base: String,
    files: Arc<Mutex<Files>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FileServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FileServer {
    pub fn url(&self, name: &str) -> String {
        format!("{}/files/{name}", self.base)
    }

    pub fn put(&self, name: &str, served: Served) {
        self.files.lock().unwrap().served.insert(name.to_string(), served);
    }

    pub fn seen(&self, name: &str) -> Vec<Seen> {
        self.files.lock().unwrap().seen.iter().filter(|s| s.name == name).cloned().collect()
    }

    pub fn total_requests(&self) -> usize {
        self.files.lock().unwrap().seen.len()
    }
}

pub async fn start() -> FileServer {
    let files = Arc::new(Mutex::new(Files::default()));
    let app = Router::new().route("/files/{*name}", get(serve)).with_state(files.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    FileServer { base, files, task }
}

fn with_etag(mut response: Response, etag: Option<&str>) -> Response {
    if let Some(etag) = etag {
        response.headers_mut().insert(header::ETAG, HeaderValue::from_str(etag).unwrap());
    }
    response
}

async fn serve(
    State(files): State<Arc<Mutex<Files>>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let delay = files.lock().unwrap().served.get(&name).and_then(|s| s.delay);
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    let text = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let (range, if_range) = (text(header::RANGE), text(header::IF_RANGE));
    let key = text(header::HeaderName::from_static("x-test-key"));
    let mut guard = files.lock().unwrap();
    guard.seen.push(Seen { name: name.clone(), range: range.clone(), if_range: if_range.clone(), key });
    let Some(served) = guard.served.get_mut(&name) else { return StatusCode::NOT_FOUND.into_response() };
    if let Some(status) = served.status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if served.fail_times > 0 {
        served.fail_times -= 1;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(target) = &served.redirect_to {
        return (StatusCode::FOUND, [(header::LOCATION, target.clone())]).into_response();
    }
    if let (Some(status), Some(_)) = (served.range_status, &range) {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let total = served.body.len();
    let start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|s| s.parse::<usize>().ok());
    let validator_ok = if_range.is_none() || if_range == served.etag || served.ignore_if_range;
    if let Some(start) = start.filter(|s| *s < total && validator_ok) {
        let shown = if served.bad_range { start + 1 } else { start };
        let mut response = (StatusCode::PARTIAL_CONTENT, served.body[start..].to_vec()).into_response();
        let content_range = format!("bytes {shown}-{}/{total}", total - 1);
        response.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&content_range).unwrap());
        return with_etag(response, served.etag.as_deref());
    }
    let body = served.body.clone();
    let etag = served.etag.clone();
    if served.drop_first {
        served.drop_first = false;
        if let Some((new_body, new_etag)) = served.replace_after_drop.take() {
            served.body = new_body;
            served.etag = Some(new_etag);
        }
        drop(guard);
        let head = Bytes::copy_from_slice(&body[..body.len() / 2]);
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(head)]).chain(
            futures_util::stream::once(async {
                // Give hyper time to flush the headers and the first half before breaking off.
                tokio::time::sleep(Duration::from_millis(200)).await;
                Err(std::io::Error::other("connection dropped"))
            }),
        );
        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        return with_etag(response, etag.as_deref());
    }
    with_etag((StatusCode::OK, body).into_response(), etag.as_deref())
}
