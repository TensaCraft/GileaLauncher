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
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(connect)
        .read_timeout(read)
        .redirect(redirect_policy())
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
