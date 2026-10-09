//! The server builds' API: the catalog, a build's files and its force-update
//! manifest. Plain GETs, asked three times before giving up (an address the server refuses, once).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use launcher_core::net::api_client;
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::{Client, Url};
use serde_json::{Value, json};

/// The API's address from the build profile (`[modules.tensa] api_base`).
pub const API_BASE: &str = match option_env!("LAUNCHER_MOD_TENSA_API_BASE") {
    Some(base) => base,
    None => "https://gigabait.uk/api/mods",
};
const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);
const ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);
/// The longest a picture's version is waited for: the picture shows anyway, its old copy at worst.
const IMAGE_ASK: Duration = Duration::from_secs(4);
/// How long a picture's version is trusted before it is asked again.
const IMAGE_KNOWN: Duration = Duration::from_secs(10 * 60);

fn network(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Network, format!("server builds: {e}"))
}

/// A request worth sending again: anything but an address the server refuses (a 4xx other than
/// 408 and 429), which a second ask will not change.
fn worth_asking_again(e: &AppError) -> bool {
    match e.params.get("status").and_then(|status| status.parse::<u16>().ok()) {
        Some(status) if (400..500).contains(&status) => matches!(status, 408 | 429),
        _ => true,
    }
}

/// The objects of `value` when it is an array; nothing otherwise.
fn objects(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|item| item.is_object()).cloned().collect())
        .unwrap_or_default()
}

/// `url` with `key=value` in its query, replacing an earlier `key`.
fn with_query(url: &str, key: &str, value: &str) -> AppResult<Url> {
    let mut url = Url::parse(url).map_err(network)?;
    let kept: Vec<(String, String)> =
        url.query_pairs().filter(|(k, _)| k != key).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    url.query_pairs_mut().clear().extend_pairs(kept).append_pair(key, value);
    Ok(url)
}

pub struct TensaApi {
    client: Client,
    base: String,
    retry_delay: Duration,
    /// The pictures' versions asked lately, by address.
    image_versions: Mutex<HashMap<String, (Instant, String)>>,
}

/// A picture's version from its answer: its `ETag`, else its `Last-Modified`; letters, digits and
/// dashes only.
fn version_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let said = |name: reqwest::header::HeaderName| {
        let raw = headers.get(name)?.to_str().ok()?;
        let tag: String =
            raw.trim_start_matches("W/").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        (!tag.is_empty()).then_some(tag)
    };
    said(reqwest::header::ETAG).or_else(|| said(reqwest::header::LAST_MODIFIED))
}

impl TensaApi {
    pub fn new(base: &str) -> AppResult<TensaApi> {
        Ok(TensaApi {
            client: api_client(&format!("{APP_NAME}/{VERSION}"), CONNECT, READ)?,
            base: base.trim_end_matches('/').to_string(),
            retry_delay: RETRY_DELAY,
            image_versions: Mutex::new(HashMap::new()),
        })
    }

    /// The pause before the n-th retry is `delay · n`.
    pub fn with_retry_delay(mut self, delay: Duration) -> TensaApi {
        self.retry_delay = delay;
        self
    }

    async fn try_get(&self, url: &str) -> AppResult<Value> {
        let response = self.client.get(url).send().await.map_err(network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(network(format!("HTTP {status}")).with_param("status", status.as_u16().to_string()));
        }
        let body = response.bytes().await.map_err(network)?;
        serde_json::from_slice(&body).map_err(network)
    }

    async fn get_json(&self, url: &str) -> AppResult<Value> {
        let mut attempt = 1;
        loop {
            match self.try_get(url).await {
                Ok(value) => return Ok(value),
                Err(e) if attempt < ATTEMPTS && worth_asking_again(&e) => {
                    tracing::warn!(
                        "A server builds request failed, retrying ({attempt}/{ATTEMPTS}) {url}: {}",
                        e.detail
                    );
                    tokio::time::sleep(self.retry_delay * attempt).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// `endpoint` when one is named, else `{base}/{pack_id}{suffix}`.
    fn address(&self, pack_id: &str, endpoint: Option<&str>, suffix: &str) -> AppResult<String> {
        let pack_id = pack_id.trim();
        if pack_id.is_empty() {
            return Err(AppError::new(ErrorCode::InvalidInput, "a server build needs its id"));
        }
        Ok(endpoint
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map_or_else(|| format!("{}/{pack_id}{suffix}", self.base), str::to_string))
    }

    /// The version of the picture at `url` (`version_of`); `None` when the server does not say or
    /// does not answer in time. A version asked lately is not asked again.
    pub async fn image_version(&self, url: &str) -> Option<String> {
        let versions = || self.image_versions.lock().unwrap_or_else(|e| e.into_inner());
        let known = versions().get(url).filter(|(at, _)| at.elapsed() < IMAGE_KNOWN).map(|(_, v)| v.clone());
        if known.is_some() {
            return known;
        }
        let answer = tokio::time::timeout(IMAGE_ASK, self.client.head(url).send()).await;
        let version = match answer {
            Ok(Ok(response)) if response.status().is_success() => version_of(response.headers()),
            _ => None,
        }?;
        let mut kept = versions();
        kept.retain(|_, (at, _)| at.elapsed() < IMAGE_KNOWN);
        kept.insert(url.to_string(), (Instant::now(), version.clone()));
        Some(version)
    }

    /// The catalog: its entries that are objects.
    pub async fn packs(&self) -> AppResult<Vec<Value>> {
        Ok(objects(Some(&self.get_json(&self.base).await?)))
    }

    /// A build's files: a list, or an object's `files`.
    pub async fn files(&self, pack_id: &str, endpoint: Option<&str>) -> AppResult<Vec<Value>> {
        let data = self.get_json(&self.address(pack_id, endpoint, "")?).await?;
        Ok(if data.is_object() { objects(data.get("files")) } else { objects(Some(&data)) })
    }

    /// A build's force-update manifest, with the files of its folders: `{…, files, directories}`
    /// (checked by `manifest::validate`).
    pub async fn force_manifest(&self, pack_id: &str, endpoint: Option<&str>) -> AppResult<Value> {
        let url =
            with_query(&self.address(pack_id, endpoint, "/force-update")?, "include_directory_files", "1")?;
        let data = self.get_json(url.as_str()).await?;
        Ok(match data {
            Value::Object(mut map) => {
                let files = objects(map.get("files"));
                let directories = objects(map.get("directories"));
                map.insert("files".into(), Value::Array(files));
                map.insert("directories".into(), Value::Array(directories));
                Value::Object(map)
            }
            Value::Array(_) => json!({"files": objects(Some(&data)), "directories": []}),
            _ => json!({"files": [], "directories": []}),
        })
    }
}
