//! Small metadata requests (version lists, runtime manifests): HTTPS only, `200` answers kept in
//! memory for an hour.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::{Client, StatusCode, Url};
use serde_json::Value;
use sha1::{Digest, Sha1};

use super::{HTTPS_ONLY, redirect_policy, url_allowed};

pub const META_TTL: Duration = Duration::from_secs(60 * 60);

type Cache = HashMap<String, (Instant, Vec<u8>)>;

/// The largest answer kept in memory (a version list or a runtime manifest is far smaller).
const KEPT_MOST: usize = 8 * 1024 * 1024;

/// Keeps `body` of `url` (unless it is larger than `KEPT_MOST`); answers past `ttl` go meanwhile,
/// so the cache holds only what may still be reused.
fn keep(cache: &mut Cache, ttl: Duration, url: String, body: Vec<u8>) {
    cache.retain(|_, (at, _)| at.elapsed() < ttl);
    if body.len() <= KEPT_MOST {
        cache.insert(url, (Instant::now(), body));
    }
}

pub struct MetaClient {
    client: Client,
    ttl: Duration,
    cache: Mutex<Cache>,
}

fn network(url: &str, why: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Network, format!("{url}: {why}")).with_param("error", why.to_string())
}

impl MetaClient {
    pub fn new(timeout: Duration, ttl: Duration) -> AppResult<MetaClient> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .user_agent(format!("{APP_NAME}/{VERSION}"))
            .connect_timeout(timeout)
            .timeout(timeout)
            .redirect(redirect_policy())
            .build()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(MetaClient { client, ttl, cache: Mutex::new(HashMap::new()) })
    }

    fn cache(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The body of `url`; a `200` answer is reused until `ttl` passes.
    pub async fn get_bytes(&self, url: &str) -> AppResult<Vec<u8>> {
        let parsed = Url::parse(url).map_err(|e| network(url, e))?;
        if !url_allowed(&parsed) {
            return Err(network(url, HTTPS_ONLY));
        }
        let cached =
            self.cache().get(url).filter(|(at, _)| at.elapsed() < self.ttl).map(|(_, body)| body.clone());
        if let Some(body) = cached {
            return Ok(body);
        }
        let response = self.client.get(parsed).send().await.map_err(|e| network(url, e))?;
        if response.status() != StatusCode::OK {
            return Err(network(url, format!("HTTP {}", response.status())));
        }
        let body = response.bytes().await.map_err(|e| network(url, e))?.to_vec();
        keep(&mut self.cache(), self.ttl, url.to_string(), body.clone());
        Ok(body)
    }

    /// `get_bytes` checked against a SHA-1 when one is known; a mismatch is `DownloadFailed` and
    /// is not kept.
    pub async fn get_verified(&self, url: &str, sha1: Option<&str>) -> AppResult<Vec<u8>> {
        let body = self.get_bytes(url).await?;
        if let Some(expected) = sha1 {
            let actual = hex::encode(Sha1::digest(&body));
            if !actual.eq_ignore_ascii_case(expected) {
                self.cache().remove(url);
                let why = format!("sha1 mismatch: expected {expected}, got {actual}");
                return Err(AppError::new(ErrorCode::DownloadFailed, format!("{url}: {why}"))
                    .with_param("error", why));
            }
        }
        Ok(body)
    }

    pub async fn get_json(&self, url: &str) -> AppResult<Value> {
        let body = self.get_bytes(url).await?;
        serde_json::from_slice(&body).map_err(|e| {
            self.cache().remove(url);
            network(url, e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_past_their_time_leave_the_cache_when_another_comes() {
        let ttl = Duration::from_secs(60);
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(120)).unwrap();
        let mut cache: Cache = HashMap::new();
        cache.insert("https://a/old".into(), (old, vec![1; 10]));
        cache.insert("https://a/fresh".into(), (now, vec![2; 10]));
        keep(&mut cache, ttl, "https://a/new".into(), vec![3; 10]);
        let mut urls: Vec<&str> = cache.keys().map(String::as_str).collect();
        urls.sort();
        assert_eq!(urls, ["https://a/fresh", "https://a/new"]);
    }

    #[test]
    fn a_large_answer_is_not_kept() {
        let mut cache: Cache = HashMap::new();
        keep(&mut cache, Duration::from_secs(60), "https://a/huge".into(), vec![0; KEPT_MOST + 1]);
        assert!(cache.is_empty());
    }
}
