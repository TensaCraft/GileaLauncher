mod support;

use launcher_core::auth::AuthTimings;
use launcher_core::auth::http::{AuthHttp, FlowError};
use launcher_core::auth::xbox::minecraft_session;
use launcher_shared::ErrorCode;
use serde_json::json;
use support::fake_ms::{self, MC_ID, MC_NAME, XUID};

fn http() -> AuthHttp {
    AuthHttp::new(&AuthTimings::fast()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_retries_server_errors_then_succeeds() {
    let fake = fake_ms::start().await;
    fake.knobs().token_failures = 2;
    let form = [("grant_type", "refresh_token"), ("refresh_token", "r")];
    let reply = http().post_form("t", &fake.endpoints().token, &form).await.unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(fake.requests("/token").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_returns_the_last_server_error_after_three_attempts() {
    let fake = fake_ms::start().await;
    fake.knobs().token_failures = 10;
    let form = [("grant_type", "refresh_token"), ("refresh_token", "r")];
    let reply = http().post_form("microsoft token refresh", &fake.endpoints().token, &form).await.unwrap();
    assert_eq!(reply.status, 503);
    assert_eq!(reply.body, json!({ "raw": "busy" }));
    assert_eq!(fake.requests("/token").len(), 3);
    match reply.ok("microsoft token refresh") {
        Err(FlowError::Failed(detail)) => {
            assert!(detail.starts_with("microsoft token refresh failed: HTTP 503"))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signs_into_minecraft_through_xbox_live() {
    let fake = fake_ms::start().await;
    let session = minecraft_session(&http(), &fake.endpoints(), "ms-access-1", 1_000).await.unwrap();
    assert_eq!((session.id.as_str(), session.name.as_str()), (MC_ID, MC_NAME));
    assert_eq!(session.xuid.as_deref(), Some(XUID));
    assert_eq!(session.expires_at, 1_000 + 86_400);
    let xbl = fake.requests("/xbl")[0].json.clone().unwrap();
    assert_eq!(xbl["Properties"]["RpsTicket"], json!("d=ms-access-1"));
    assert_eq!(xbl["RelyingParty"], json!("http://auth.xboxlive.com"));
    let xsts = fake.requests("/xsts")[0].json.clone().unwrap();
    assert_eq!(xsts["Properties"]["UserTokens"], json!(["xbl-token"]));
    assert_eq!(xsts["RelyingParty"], json!("rp://api.minecraftservices.com/"));
    let login = fake.requests("/mc-login")[0].json.clone().unwrap();
    assert_eq!(login["identityToken"], json!("XBL3.0 x=uhs-1;xsts-token"));
    assert_eq!(fake.requests("/mc-profile")[0].bearer.as_deref(), Some(session.access_token.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn jwt_expiry_is_used_when_the_login_has_no_expires_in() {
    let fake = fake_ms::start().await;
    {
        let mut k = fake.knobs();
        k.mc_expires_in = None;
        k.jwt_exp = 5_000;
    }
    let session = minecraft_session(&http(), &fake.endpoints(), "ms-access-1", 1_000).await.unwrap();
    assert_eq!(session.expires_at, 5_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn xsts_refusals_become_friendly_errors() {
    let fake = fake_ms::start().await;
    fake.knobs().xsts_xerr = Some(2_148_916_233);
    match minecraft_session(&http(), &fake.endpoints(), "a", 0).await {
        Err(FlowError::App(e)) => assert_eq!(e.code, ErrorCode::XboxAccountMissing),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn minecraft_services_outage_is_reported_as_such() {
    let fake = fake_ms::start().await;
    fake.knobs().mc_login_status = Some(503);
    let err = minecraft_session(&http(), &fake.endpoints(), "a", 0).await.unwrap_err();
    assert!(matches!(err, FlowError::ServicesUnavailable(_)), "{err:?}");
    assert_eq!(err.into_app_error().code, ErrorCode::MinecraftServicesUnavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn account_without_minecraft_is_explained() {
    let fake = fake_ms::start().await;
    fake.knobs().mc_profile_status = Some(404);
    match minecraft_session(&http(), &fake.endpoints(), "a", 0).await {
        Err(FlowError::App(e)) => assert_eq!(e.code, ErrorCode::MinecraftNotOwned),
        other => panic!("{other:?}"),
    }
}

use std::time::Duration;

use launcher_core::auth::msa::{Loopback, browser_sign_in, refresh_tokens};
use launcher_core::auth::{AuthEndpoints, NoOpener};
use support::fake_ms::{Browser, FakeBrowser, GOOD_CODE, visit};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(5);

async fn wait_in_background(
    loopback: Loopback,
    state: &'static str,
) -> tokio::task::JoinHandle<Result<String, FlowError>> {
    tokio::spawn(async move { loopback.wait_for_code(state, "en_US", WAIT, &CancellationToken::new()).await })
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_ignores_other_paths_and_returns_the_code() {
    let loopback = Loopback::bind(0).await.unwrap();
    let port = loopback.port();
    assert_eq!(loopback.redirect_uri, format!("http://localhost:{port}/callback"));
    let waiting = wait_in_background(loopback, "s1").await;
    let other = visit(&format!("http://127.0.0.1:{port}/favicon.ico")).await;
    assert!(other.starts_with("HTTP/1.1 404"), "{other}");
    let page = visit(&format!("http://127.0.0.1:{port}/callback?code=abc&state=s1")).await;
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("<html"), "{page}");
    assert_eq!(waiting.await.unwrap(), Ok("abc".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_turns_away_a_foreign_state_and_keeps_waiting() {
    // A stale tab, another program or a page's <img> must not end this sign-in.
    let loopback = Loopback::bind(0).await.unwrap();
    let port = loopback.port();
    let waiting = wait_in_background(loopback, "mine").await;
    let page = visit(&format!("http://127.0.0.1:{port}/callback?code=abc&state=other")).await;
    assert!(page.starts_with("HTTP/1.1 400"), "{page}");
    let page = visit(&format!("http://127.0.0.1:{port}/callback?error=access_denied")).await;
    assert!(page.starts_with("HTTP/1.1 400"), "an error without this sign-in's state: {page}");
    visit(&format!("http://127.0.0.1:{port}/callback?code=real&state=mine")).await;
    assert_eq!(waiting.await.unwrap(), Ok("real".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_reports_denial_and_escapes_the_text() {
    let loopback = Loopback::bind(0).await.unwrap();
    let port = loopback.port();
    let waiting = wait_in_background(loopback, "s").await;
    let page = visit(&format!(
        "http://127.0.0.1:{port}/callback?error=access_denied&error_description=%3Cb%3Eno%3C%2Fb%3E&state=s"
    ))
    .await;
    assert!(page.contains("&lt;b&gt;no&lt;/b&gt;") && !page.contains("<b>no"), "{page}");
    assert_eq!(waiting.await.unwrap(), Err(FlowError::Denied));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_answers_on_ipv6_localhost_too() {
    let loopback = Loopback::bind(0).await.unwrap();
    let port = loopback.port();
    if tokio::net::TcpStream::connect(("::1", port)).await.is_err() {
        eprintln!("IPv6 loopback is not available here; skipping");
        return;
    }
    let waiting = wait_in_background(loopback, "s6").await;
    let page = visit(&format!("http://[::1]:{port}/callback?code=v6&state=s6")).await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert_eq!(waiting.await.unwrap(), Ok("v6".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_times_out_and_can_be_cancelled() {
    let loopback = Loopback::bind(0).await.unwrap();
    let result =
        loopback.wait_for_code("s", "en_US", Duration::from_millis(100), &CancellationToken::new()).await;
    assert_eq!(result, Err(FlowError::Timeout));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(loopback.wait_for_code("s", "en_US", WAIT, &cancel).await, Err(FlowError::Cancelled));
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_sign_in_exchanges_the_code_without_a_client_secret() {
    let fake = fake_ms::start().await;
    let browser = FakeBrowser::new(Browser::Consent);
    let tokens = browser_sign_in(
        &http(),
        &fake.endpoints(),
        "client",
        browser.as_ref(),
        "en_US",
        WAIT,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "ms-access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("ms-refresh-1"));
    let exchange = &fake.requests("/token")[0].form;
    assert_eq!(exchange.get("grant_type").map(String::as_str), Some("authorization_code"));
    assert_eq!(exchange.get("code").map(String::as_str), Some(GOOD_CODE));
    assert_eq!(exchange.get("client_id").map(String::as_str), Some("client"));
    assert_eq!(exchange.get("code_verifier").map(String::len), Some(86));
    assert!(exchange.get("redirect_uri").unwrap().starts_with("http://localhost:"));
    assert!(!exchange.contains_key("client_secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_sign_in_is_unavailable_without_a_browser_or_port() {
    let fake = fake_ms::start().await;
    let no_browser =
        browser_sign_in(&http(), &fake.endpoints(), "c", &NoOpener, "en_US", WAIT, &CancellationToken::new())
            .await;
    assert!(matches!(no_browser, Err(FlowError::Unavailable(_))), "{no_browser:?}");
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoints = AuthEndpoints { redirect_port: busy.local_addr().unwrap().port(), ..fake.endpoints() };
    let browser = FakeBrowser::new(Browser::Consent);
    let no_port =
        browser_sign_in(&http(), &endpoints, "c", browser.as_ref(), "en_US", WAIT, &CancellationToken::new())
            .await;
    assert!(matches!(no_port, Err(FlowError::Unavailable(_))), "{no_port:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_consent_ends_the_browser_flow() {
    let fake = fake_ms::start().await;
    let browser = FakeBrowser::new(Browser::Deny);
    let result = browser_sign_in(
        &http(),
        &fake.endpoints(),
        "c",
        browser.as_ref(),
        "en_US",
        WAIT,
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result, Err(FlowError::Denied));
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_sends_the_refresh_grant() {
    let fake = fake_ms::start().await;
    fake.knobs().omit_refresh_token = true;
    let tokens = refresh_tokens(&http(), &fake.endpoints(), "client", "old-refresh").await.unwrap();
    assert_eq!((tokens.access_token.as_str(), tokens.refresh_token), ("ms-access-2", None));
    let form = &fake.requests("/token")[0].form;
    assert_eq!(form.get("grant_type").map(String::as_str), Some("refresh_token"));
    assert_eq!(form.get("refresh_token").map(String::as_str), Some("old-refresh"));
    assert_eq!(form.get("scope").map(String::as_str), Some("XboxLive.signin offline_access"));
    assert!(!form.contains_key("client_secret"));
}

use launcher_core::auth::msa::{device_poll, device_start};
use support::fake_ms::USER_CODE;

async fn device_run(
    fake: &fake_ms::Fake,
    cancel: &CancellationToken,
) -> Result<launcher_core::auth::msa::MsTokens, FlowError> {
    let http = http();
    let code = device_start(&http, &fake.endpoints(), "client").await.unwrap();
    device_poll(&http, &fake.endpoints(), "client", &code, &AuthTimings::fast(), cancel).await
}

#[tokio::test(flavor = "multi_thread")]
async fn device_code_waits_for_the_user_then_succeeds() {
    let fake = fake_ms::start().await;
    fake.knobs().device_pending = 2;
    let code = device_start(&http(), &fake.endpoints(), "client").await.unwrap();
    assert_eq!(code.user_code, USER_CODE);
    assert_eq!(code.open_url, "https://www.microsoft.com/link?otc=ABCD-1234");
    let init = &fake.requests("/devicecode")[0].form;
    assert_eq!(init.get("client_id").map(String::as_str), Some("client"));
    assert_eq!(init.get("scope").map(String::as_str), Some("XboxLive.signin offline_access"));
    let tokens = device_poll(
        &http(),
        &fake.endpoints(),
        "client",
        &code,
        &AuthTimings::fast(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "ms-access-d");
    let polls = fake.requests("/token");
    assert_eq!(polls.len(), 3);
    assert_eq!(
        polls[0].form.get("grant_type").map(String::as_str),
        Some("urn:ietf:params:oauth:grant-type:device_code")
    );
    assert_eq!(polls[0].form.get("device_code").map(String::as_str), Some("dev-1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_down_makes_polling_slower() {
    let fake = fake_ms::start().await;
    fake.knobs().device_slow_down = 1;
    let started = std::time::Instant::now();
    device_run(&fake, &CancellationToken::new()).await.unwrap();
    // 10 ms before the first poll, then 10 + 20 ms after `slow_down`.
    assert!(started.elapsed() >= Duration::from_millis(40), "{:?}", started.elapsed());
    assert_eq!(fake.requests("/token").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn declined_and_expired_device_codes_end_the_flow() {
    let fake = fake_ms::start().await;
    fake.knobs().device_error = Some("authorization_declined".into());
    assert_eq!(device_run(&fake, &CancellationToken::new()).await, Err(FlowError::Denied));
    let fake = fake_ms::start().await;
    fake.knobs().device_error = Some("expired_token".into());
    assert_eq!(
        device_run(&fake, &CancellationToken::new()).await,
        Err(FlowError::Failed("Device code expired".into()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_device_polling_promptly() {
    let fake = fake_ms::start().await;
    fake.knobs().device_pending = 100_000;
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    assert_eq!(device_run(&fake, &cancel).await, Err(FlowError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_is_unavailable_when_another_program_holds_ipv6_localhost() {
    let Ok(foreign) = std::net::TcpListener::bind("[::1]:0") else {
        eprintln!("IPv6 loopback is not available here; skipping");
        return;
    };
    let port = foreign.local_addr().unwrap().port();
    assert!(Loopback::bind(port).await.is_err(), "browsers trying [::1] first would reach the other program");
}
