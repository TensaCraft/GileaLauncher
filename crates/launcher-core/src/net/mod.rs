//! Network and disk plumbing shared by installs: the downloader, metadata requests and storage
//! preflight.

pub mod downloader;
pub mod meta;
pub mod preflight;

use std::sync::Arc;
use std::time::Duration;

use launcher_shared::{AppError, AppResult};
use reqwest::Url;

pub(crate) const HTTPS_ONLY: &str = "only https downloads are allowed";
const MAX_REDIRECTS: usize = 10;

/// A build for development (a debug build, the tests) or for trying updates against the local
/// mock (`mock-updates`) takes plain `http` from this machine; a release never does.
const LOCAL_SERVERS_TRUSTED: bool = cfg!(any(debug_assertions, feature = "mock-updates"));

/// HTTPS everywhere; plain `http` only to this machine, and only in `LOCAL_SERVERS_TRUSTED` builds.
pub(crate) fn url_allowed(url: &Url) -> bool {
    url_allowed_with(url, LOCAL_SERVERS_TRUSTED)
}

/// `url_allowed`, taking plain `http` from this machine only when `trust_local`.
fn url_allowed_with(url: &Url, trust_local: bool) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => {
            trust_local && matches!(url.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"))
        }
        _ => false,
    }
}

/// `redirect_policy`, and no hop may leave the hosts `within` accepts: a request that carries a
/// credential for those hosts stops rather than take it elsewhere.
pub(crate) fn redirect_policy_within(
    within: Arc<dyn Fn(&Url) -> bool + Send + Sync>,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if !url_allowed(attempt.url()) {
            attempt.error(HTTPS_ONLY)
        } else if !within(attempt.url()) {
            attempt.error("the redirect leaves the hosts the request's credential is for")
        } else {
            attempt.follow()
        }
    })
}

/// Redirects are held to `url_allowed` on every hop, at most ten of them.
pub(crate) fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if !url_allowed(attempt.url()) {
            attempt.error(HTTPS_ONLY)
        } else {
            attempt.follow()
        }
    })
}

/// A client for a JSON API: `user_agent`, the timeouts and the downloader's redirect rules.
pub fn api_client(user_agent: &str, connect: Duration, read: Duration) -> AppResult<reqwest::Client> {
    client_with(user_agent, connect, read, redirect_policy())
}

/// `api_client` for an API that takes a key (a header no redirect strips): no redirect leaves the
/// hosts `within` accepts, so the key never goes elsewhere.
pub fn api_client_within(
    user_agent: &str,
    connect: Duration,
    read: Duration,
    within: Arc<dyn Fn(&Url) -> bool + Send + Sync>,
) -> AppResult<reqwest::Client> {
    client_with(user_agent, connect, read, redirect_policy_within(within))
}

/// How many times an API request goes out to a busy server.
const API_ATTEMPTS: u32 = 3;
/// The longest a server's `Retry-After` is waited for by an API request (someone waits on it).
const API_RETRY_AFTER: Duration = Duration::from_secs(10);

/// Sends `request`; a busy server (429, 502, 503, 504) is asked again, up to twice, after its
/// `Retry-After` (at most 10 s) or a pause that grows (0.5 s, 1 s).
pub async fn send_patiently(mut request: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let mut attempt = 1;
    loop {
        let again = request.try_clone();
        let response = request.send().await?;
        let busy = matches!(response.status().as_u16(), 429 | 502 | 503 | 504);
        match again {
            Some(next) if busy && attempt < API_ATTEMPTS => {
                let asked = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(|secs| Duration::from_secs(secs).min(API_RETRY_AFTER));
                tokio::time::sleep(asked.unwrap_or(Duration::from_millis(500) * attempt)).await;
                request = next;
                attempt += 1;
            }
            _ => return Ok(response),
        }
    }
}

fn client_with(
    user_agent: &str,
    connect: Duration,
    read: Duration,
    redirect: reqwest::redirect::Policy,
) -> AppResult<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(connect)
        .read_timeout(read)
        .redirect(redirect)
        .build()
        .map_err(|e| AppError::internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn loopback_downloads_need_a_debug_or_mock_build() {
        let url = |u: &str| reqwest::Url::parse(u).unwrap();
        for local in ["http://127.0.0.1:1430/x", "http://localhost/x", "http://[::1]:9/x"] {
            assert!(!super::url_allowed_with(&url(local), false), "a release takes nothing from {local}");
            assert!(super::url_allowed_with(&url(local), true));
        }
        assert!(super::url_allowed_with(&url("https://cdn.modrinth.com/x"), false));
        assert!(!super::url_allowed_with(&url("http://example.com/x"), true));
        assert!(!super::url_allowed_with(&url("ftp://127.0.0.1/x"), true));
    }

    #[test]
    fn api_clients_build() {
        assert!(
            super::api_client(
                "Launcher/test (https://example.com)",
                Duration::from_secs(5),
                Duration::from_secs(20)
            )
            .is_ok()
        );
    }
}
