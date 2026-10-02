//! GitHub Releases HTTP client.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::{self, HeaderMap};
use reqwest::{Client, StatusCode, Url};
use serde::de::DeserializeOwned;

use super::select::{GhAsset, GhRelease, LOOPBACK_TRUSTED, download_url_allowed};

pub const API_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;
const MAX_ASSET_PAGES: u32 = 10;
const PER_PAGE: usize = 100;

pub fn network_error(e: reqwest::Error) -> AppError {
    AppError::new(ErrorCode::Network, e.to_string())
}

/// `https://…` anywhere, or plain `http://` to the local mock server only in a build made for it
/// (`LOOPBACK_TRUSTED`).
pub fn validate_api_base(raw: &str) -> AppResult<Url> {
    api_base_with(raw, LOOPBACK_TRUSTED)
}

/// `validate_api_base`, trusting the local mock only when `trust_loopback`.
pub(crate) fn api_base_with(raw: &str, trust_loopback: bool) -> AppResult<Url> {
    let url = Url::parse(raw)
        .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()).with_param("url", raw))?;
    let loopback = trust_loopback && matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(AppError::new(ErrorCode::InvalidInput, "update API must use https").with_param("url", raw)),
    }
}

/// Maps a non-success response. GitHub rate limits become `RateLimited { minutes }`.
pub fn status_error(status: StatusCode, headers: &HeaderMap) -> AppError {
    let remaining = headers.get("x-ratelimit-remaining").and_then(|v| v.to_str().ok());
    let limited = status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS;
    if limited && remaining == Some("0") {
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let minutes = reset.map(|r| r.saturating_sub(now).div_ceil(60).max(1)).unwrap_or(60);
        return AppError::new(ErrorCode::RateLimited, "GitHub API rate limit exceeded")
            .with_param("minutes", minutes.to_string());
    }
    AppError::new(ErrorCode::Network, format!("HTTP {status}"))
        .with_param("status", status.as_u16().to_string())
}

#[derive(Clone)]
pub struct GithubClient {
    http: Client,
    api_base: Url,
    repo: String,
}

impl GithubClient {
    pub fn new(api_base: &str, repo: &str, user_agent: &str) -> AppResult<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let api_base = validate_api_base(api_base)?;
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() != 2 || parts.iter().any(|p| p.trim().is_empty()) {
            return Err(AppError::new(ErrorCode::InvalidInput, "update repository must be owner/repo")
                .with_param("repo", repo));
        }
        let policy_base = api_base.clone();
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if download_url_allowed(&policy_base, attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect to a host that is not allowed")
            }
        });
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .redirect(policy)
            .build()
            .map_err(network_error)?;
        Ok(Self { http, api_base, repo: repo.to_string() })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub fn api_base(&self) -> &Url {
        &self.api_base
    }

    fn endpoint(&self, path: &str) -> AppResult<Url> {
        let base = self.api_base.as_str().trim_end_matches('/');
        Url::parse(&format!("{base}{path}")).map_err(|e| AppError::internal(e.to_string()))
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> AppResult<T> {
        let resp = self
            .http
            .get(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(API_TIMEOUT)
            .send()
            .await
            .map_err(network_error)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status, resp.headers()));
        }
        let body = resp.bytes().await.map_err(network_error)?;
        serde_json::from_slice(&body)
            .map_err(|e| AppError::new(ErrorCode::Network, format!("unexpected GitHub response: {e}")))
    }

    pub async fn releases(&self) -> AppResult<Vec<GhRelease>> {
        let url = self.endpoint(&format!("/repos/{}/releases?per_page={PER_PAGE}", self.repo))?;
        self.get_json(url).await
    }

    /// All assets of a release, following pagination (at most 10 pages).
    pub async fn release_assets(&self, release_id: u64) -> AppResult<Vec<GhAsset>> {
        if release_id == 0 {
            return Err(AppError::new(ErrorCode::InvalidInput, "release id must be positive"));
        }
        let mut all = Vec::new();
        for page in 1..=MAX_ASSET_PAGES {
            let path =
                format!("/repos/{}/releases/{release_id}/assets?per_page={PER_PAGE}&page={page}", self.repo);
            let chunk: Vec<GhAsset> = self.get_json(self.endpoint(&path)?).await?;
            let last = chunk.len() < PER_PAGE;
            all.extend(chunk);
            if last {
                break;
            }
        }
        Ok(all)
    }
}
