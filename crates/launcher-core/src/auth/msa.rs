//! Microsoft sign-in: browser + PKCE with a loopback redirect, and token refresh.
//! The device code flow lives here too.

use std::io;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::http::{AuthHttp, FlowError};
use super::page::callback_page;
use super::{AuthEndpoints, AuthTimings, DEVICE_GRANT, SCOPE, UrlOpener};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn generate() -> Pkce {
        let mut verifier = [0u8; 64];
        let mut state = [0u8; 32];
        getrandom::fill(&mut verifier).expect("the OS random number generator is available");
        getrandom::fill(&mut state).expect("the OS random number generator is available");
        Pkce::from_bytes(&verifier, &state)
    }

    /// The verifier is 86 characters (64 bytes), like Python's `secrets.token_urlsafe(64)`.
    pub fn from_bytes(verifier: &[u8; 64], state: &[u8; 32]) -> Pkce {
        let verifier = URL_SAFE_NO_PAD.encode(verifier);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge, state: URL_SAFE_NO_PAD.encode(state) }
    }
}

/// Query parameters in the original launcher's order; never a client secret.
pub fn authorize_url(ep: &AuthEndpoints, client_id: &str, redirect_uri: &str, pkce: &Pkce) -> String {
    Url::parse_with_params(
        &ep.authorize,
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("response_mode", "query"),
            ("scope", SCOPE),
            ("prompt", "select_account"),
            ("state", pkce.state.as_str()),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
        ],
    )
    .map(String::from)
    .unwrap_or_default()
}

/// The redirect target `http://localhost:<port>/callback`, served on 127.0.0.1 and, when the
/// system has it, on [::1] too: browsers may resolve `localhost` to either.
pub struct Loopback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    pub redirect_uri: String,
}

impl Loopback {
    pub async fn bind(port: u16) -> io::Result<Loopback> {
        let v4 = TcpListener::bind(("127.0.0.1", port)).await?;
        let port = v4.local_addr()?.port();
        let v6 = match TcpListener::bind(("::1", port)).await {
            Ok(listener) => Some(listener),
            // Another program owns `[::1]:port`: browsers that try IPv6 first would reach it.
            Err(e) if matches!(e.kind(), io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied) => {
                return Err(e);
            }
            // No IPv6 loopback on this system.
            Err(_) => None,
        };
        Ok(Loopback { v4, v6, redirect_uri: format!("http://localhost:{port}/callback") })
    }

    pub fn port(&self) -> u16 {
        self.v4.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    async fn accept(&self) -> io::Result<TcpStream> {
        match &self.v6 {
            Some(v6) => tokio::select! {
                accepted = self.v4.accept() => accepted.map(|(stream, _)| stream),
                accepted = v6.accept() => accepted.map(|(stream, _)| stream),
            },
            None => self.v4.accept().await.map(|(stream, _)| stream),
        }
    }

    /// Serves requests until `/callback` arrives; other paths get 404 and waiting continues.
    pub async fn wait_for_code(
        &self,
        state: &str,
        lang: &str,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<String, FlowError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let accepted = tokio::select! {
                _ = cancel.cancelled() => return Err(FlowError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => return Err(FlowError::Timeout),
                accepted = self.accept() => accepted,
            };
            let Ok(stream) = accepted else { continue };
            if let Some(outcome) = serve_callback(stream, state, lang).await {
                return outcome;
            }
        }
    }
}

/// Reads the request head (≤ 8 KiB, 5 s).
async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = async {
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 8192 {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(Duration::from_secs(5), read).await.ok()??;
    String::from_utf8(buf).ok()
}

async fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// `None` — not the callback (answered already), keep waiting.
async fn serve_callback(mut stream: TcpStream, state: &str, lang: &str) -> Option<Result<String, FlowError>> {
    let head = read_head(&mut stream).await?;
    let mut request_line = head.lines().next()?.split_whitespace();
    let (method, target) = (request_line.next()?, request_line.next()?);
    if method != "GET" {
        respond(&mut stream, 405, "Method Not Allowed", "").await;
        return None;
    }
    let url = Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != "/callback" {
        respond(&mut stream, 404, "Not Found", "").await;
        return None;
    }
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    let outcome = if let Some(error) = param("error") {
        let description = param("error_description").unwrap_or_else(|| error.clone());
        respond(&mut stream, 400, "Bad Request", &callback_page(lang, Some(&description))).await;
        if error == "access_denied" || error == "authorization_declined" {
            Err(FlowError::Denied)
        } else {
            Err(FlowError::Failed(format!("microsoft authorization failed: {error}: {description}")))
        }
    } else if param("state").as_deref() != Some(state) {
        respond(&mut stream, 400, "Bad Request", &callback_page(lang, Some("invalid_state"))).await;
        Err(FlowError::Failed("invalid_state".into()))
    } else if let Some(code) = param("code").filter(|c| !c.is_empty()) {
        respond(&mut stream, 200, "OK", &callback_page(lang, None)).await;
        Ok(code)
    } else {
        respond(&mut stream, 400, "Bad Request", &callback_page(lang, Some("missing code"))).await;
        Err(FlowError::Failed("the redirect carried no code".into()))
    };
    Some(outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

fn ms_tokens(body: &Value, context: &str) -> Result<MsTokens, FlowError> {
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| FlowError::Failed(format!("{context} failed: no access_token")))?;
    let refresh_token =
        body.get("refresh_token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    Ok(MsTokens { access_token, refresh_token })
}

/// Public client: never sends a client secret.
pub async fn exchange_code(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<MsTokens, FlowError> {
    let context = "microsoft authorization code exchange";
    let form = [
        ("client_id", client_id),
        ("scope", SCOPE),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier),
    ];
    let body = http.post_form(context, &ep.token, &form).await?.ok(context)?;
    ms_tokens(&body, context)
}

pub async fn refresh_tokens(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    client_id: &str,
    refresh_token: &str,
) -> Result<MsTokens, FlowError> {
    let context = "microsoft token refresh";
    let form = [
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
        ("scope", SCOPE),
    ];
    let body = http.post_form(context, &ep.token, &form).await?.ok(context)?;
    ms_tokens(&body, context)
}

/// Browser sign-in. `Unavailable` when the redirect port or the browser cannot be used.
pub async fn browser_sign_in(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    client_id: &str,
    opener: &dyn UrlOpener,
    lang: &str,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<MsTokens, FlowError> {
    let loopback = Loopback::bind(ep.redirect_port)
        .await
        .map_err(|e| FlowError::Unavailable(format!("cannot listen on the redirect port: {e}")))?;
    let pkce = Pkce::generate();
    let url = authorize_url(ep, client_id, &loopback.redirect_uri, &pkce);
    if !opener.open(&url) {
        return Err(FlowError::Unavailable("the browser could not be opened".into()));
    }
    let code = loopback.wait_for_code(&pkce.state, lang, timeout, cancel).await?;
    exchange_code(http, ep, client_id, &code, &loopback.redirect_uri, &pkce.verifier).await
}

pub const DEFAULT_VERIFICATION_URI: &str = "https://www.microsoft.com/link";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// What the browser opens: `verification_uri_complete`, or microsoft.com/link with `otc`.
    pub open_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

pub fn device_open_url(verification_uri: &str, complete: Option<&str>, user_code: &str) -> String {
    if let Some(complete) = complete.filter(|c| !c.is_empty()) {
        return complete.to_string();
    }
    match Url::parse(verification_uri) {
        Ok(mut url)
            if matches!(url.host_str(), Some("www.microsoft.com") | Some("microsoft.com"))
                && url.path().trim_end_matches('/') == "/link" =>
        {
            url.query_pairs_mut().append_pair("otc", user_code);
            url.into()
        }
        _ => verification_uri.to_string(),
    }
}

pub async fn device_start(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    client_id: &str,
) -> Result<DeviceCode, FlowError> {
    let context = "microsoft device code init";
    let body = http
        .post_form(context, &ep.device_code, &[("client_id", client_id), ("scope", SCOPE)])
        .await?
        .ok(context)?;
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    let missing = |key: &str| FlowError::Failed(format!("{context} failed: no {key}"));
    let user_code = text("user_code").ok_or_else(|| missing("user_code"))?;
    let verification_uri = text("verification_uri").unwrap_or_else(|| DEFAULT_VERIFICATION_URI.to_string());
    Ok(DeviceCode {
        device_code: text("device_code").ok_or_else(|| missing("device_code"))?,
        open_url: device_open_url(
            &verification_uri,
            text("verification_uri_complete").as_deref(),
            &user_code,
        ),
        user_code,
        verification_uri,
        interval: body.get("interval").and_then(Value::as_u64).unwrap_or(5),
        expires_in: body.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
    })
}

/// Polls until the user finishes: `slow_down` adds a step to the interval,
/// the deadline is max(30 s, `expires_in`), cancellation is honoured while waiting.
pub async fn device_poll(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    client_id: &str,
    code: &DeviceCode,
    timings: &AuthTimings,
    cancel: &CancellationToken,
) -> Result<MsTokens, FlowError> {
    let context = "microsoft device code poll";
    let mut interval = timings.device_min_interval.max(Duration::from_secs(code.interval));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in.max(30));
    let form =
        [("client_id", client_id), ("grant_type", DEVICE_GRANT), ("device_code", code.device_code.as_str())];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Err(FlowError::Cancelled),
            _ = tokio::time::sleep(interval) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(FlowError::Failed("Device code expired".into()));
        }
        let reply = http.post_form(context, &ep.token, &form).await?;
        if reply.is_success() && reply.body.get("access_token").is_some() {
            return ms_tokens(&reply.body, context);
        }
        match reply.body.get("error").and_then(Value::as_str).unwrap_or_default() {
            "authorization_pending" => {}
            "slow_down" => interval += timings.slow_down_step,
            "authorization_declined" | "access_denied" => return Err(FlowError::Denied),
            "expired_token" | "bad_verification_code" | "code_expired" => {
                return Err(FlowError::Failed("Device code expired".into()));
            }
            _ => {
                return Err(FlowError::Failed(format!(
                    "{context} failed: HTTP {} {}",
                    reply.status, reply.body
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_follows_rfc7636_s256() {
        let bytes: [u8; 64] = std::array::from_fn(|i| i as u8);
        let pkce = Pkce::from_bytes(&bytes, &[7; 32]);
        assert_eq!(pkce.verifier.len(), 86);
        assert_eq!(pkce.challenge, "wsNdZaf3VpLTsEDmR5gPk2C6xYVWxKb0xcaG3O6kX10");
        assert_eq!(pkce.state.len(), 43);
        assert_ne!(Pkce::generate(), Pkce::generate());
    }

    #[test]
    fn authorize_url_matches_the_original_parameters() {
        let pkce = Pkce::from_bytes(&[0; 64], &[1; 32]);
        let url = Url::parse(&authorize_url(
            &AuthEndpoints::default(),
            "client",
            "http://localhost:8080/callback",
            &pkce,
        ))
        .unwrap();
        assert_eq!(
            url.as_str().split('?').next(),
            Some("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize")
        );
        let keys: Vec<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
        assert_eq!(
            keys,
            [
                "client_id",
                "response_type",
                "redirect_uri",
                "response_mode",
                "scope",
                "prompt",
                "state",
                "code_challenge",
                "code_challenge_method"
            ]
        );
        let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).unwrap().1.into_owned();
        assert_eq!(get("redirect_uri"), "http://localhost:8080/callback");
        assert_eq!(get("scope"), "XboxLive.signin offline_access");
        assert_eq!(get("prompt"), "select_account");
        assert_eq!(get("code_challenge"), pkce.challenge);
        assert_eq!(get("code_challenge_method"), "S256");
        assert!(!url.as_str().contains("client_secret"));
    }

    #[test]
    fn device_page_prefills_the_code() {
        assert_eq!(
            device_open_url("https://www.microsoft.com/link", None, "ABCD-1234"),
            "https://www.microsoft.com/link?otc=ABCD-1234"
        );
        assert_eq!(
            device_open_url("https://www.microsoft.com/link", Some("https://x.test/c?code=1"), "A"),
            "https://x.test/c?code=1"
        );
        assert_eq!(
            device_open_url("https://login.example/device", None, "A"),
            "https://login.example/device"
        );
    }
}
