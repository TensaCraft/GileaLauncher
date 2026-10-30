mod support;

use launcher_core::auth::AuthTimings;
use launcher_core::auth::http::AuthHttp;
use launcher_core::auth::yggdrasil::{self, Credentials};
use launcher_shared::ErrorCode;
use support::fake_ygg::{self, LOGIN, PASSWORD, PROFILE_ID, PROFILE_NAME, SERVER_NAME, TOTP};

fn http() -> AuthHttp {
    AuthHttp::new(&AuthTimings::fast()).unwrap()
}

fn credentials<'a>(login: &'a str, password: &'a str, code: Option<&'a str>) -> Credentials<'a> {
    Credentials { login, password, code, client_token: "client-1" }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_site_address_leads_to_its_api_through_the_ali_header() {
    let fake = fake_ygg::start().await;
    let server = yggdrasil::resolve(&http(), &fake.base).await.unwrap();
    assert_eq!(server.api_root, fake.api_root());
    assert_eq!(server.name, SERVER_NAME);
    let meta: serde_json::Value = serde_json::from_str(&server.metadata).unwrap();
    assert_eq!(meta["meta"]["serverName"], SERVER_NAME, "the raw body, for the game's prefetch");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_root_itself_is_taken_as_is() {
    let fake = fake_ygg::start().await;
    fake.set(|k| k.no_ali = true);
    let server = yggdrasil::resolve(&http(), &fake.api_root()).await.unwrap();
    assert_eq!(server.api_root, fake.api_root());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_without_an_auth_server_says_so() {
    let fake = fake_ygg::start().await;
    fake.set(|k| k.no_ali = true);
    for address in [fake.base.clone(), format!("{}/not-an-api", fake.base)] {
        let err = yggdrasil::resolve(&http(), &address).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AuthServerInvalid, "{address}");
    }
}

#[test]
fn an_address_without_a_scheme_is_https_and_plain_http_only_stays_on_this_computer() {
    assert_eq!(yggdrasil::address("ely.by").unwrap().as_str(), "https://ely.by/");
    assert_eq!(yggdrasil::address(" https://ely.by ").unwrap().as_str(), "https://ely.by/");
    assert_eq!(
        yggdrasil::address("http://127.0.0.1:25585/api").unwrap().as_str(),
        "http://127.0.0.1:25585/api"
    );
    assert_eq!(yggdrasil::address("http://localhost:8080").unwrap().as_str(), "http://localhost:8080/");
    for bad in ["http://ely.by", "ftp://ely.by", "", "   "] {
        assert_eq!(yggdrasil::address(bad).unwrap_err().code, ErrorCode::AuthServerInvalid, "{bad:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_gives_the_session_and_its_game_profile() {
    let fake = fake_ygg::start().await;
    let session = yggdrasil::authenticate(&http(), &fake.api_root(), &credentials(LOGIN, PASSWORD, None))
        .await
        .unwrap();
    assert!(!session.access_token.is_empty());
    assert_eq!(session.client_token, "client-1");
    let profile = session.profile.unwrap();
    assert_eq!((profile.id.as_str(), profile.name.as_str()), (PROFILE_ID, PROFILE_NAME));
    let sent = &fake.posts("authserver/authenticate")[0];
    assert_eq!(sent["agent"]["name"], "Minecraft");
    assert_eq!(sent["requestUser"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_is_wrong_credentials() {
    let fake = fake_ygg::start().await;
    let err = yggdrasil::authenticate(&http(), &fake.api_root(), &credentials(LOGIN, "nope", None))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthCredentialsInvalid);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_factor_is_asked_for_then_sent_after_the_password() {
    let fake = fake_ygg::start().await;
    fake.set(|k| k.two_factor = true);
    let asked = yggdrasil::authenticate(&http(), &fake.api_root(), &credentials(LOGIN, PASSWORD, None))
        .await
        .unwrap_err();
    assert_eq!(asked.code, ErrorCode::AuthCodeRequired);
    let session =
        yggdrasil::authenticate(&http(), &fake.api_root(), &credentials(LOGIN, PASSWORD, Some(TOTP)))
            .await
            .unwrap();
    assert!(session.profile.is_some());
    assert_eq!(fake.posts("authserver/authenticate")[1]["password"], format!("{PASSWORD}:{TOTP}"));
    let wrong =
        yggdrasil::authenticate(&http(), &fake.api_root(), &credentials(LOGIN, PASSWORD, Some("000000")))
            .await
            .unwrap_err();
    assert_eq!(wrong.code, ErrorCode::AuthCredentialsInvalid, "a wrong code is wrong credentials");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_is_checked_renewed_and_ended() {
    let fake = fake_ygg::start().await;
    let api = fake.api_root();
    let first = yggdrasil::authenticate(&http(), &api, &credentials(LOGIN, PASSWORD, None)).await.unwrap();
    assert!(yggdrasil::validate(&http(), &api, &first.access_token, "client-1").await.unwrap());
    let renewed = yggdrasil::refresh(&http(), &api, &first.access_token, "client-1").await.unwrap();
    assert_ne!(renewed.access_token, first.access_token);
    assert_eq!(renewed.client_token, "client-1", "the client keeps its token");
    assert!(
        !yggdrasil::validate(&http(), &api, &first.access_token, "client-1").await.unwrap(),
        "the old one ended"
    );
    let err = yggdrasil::refresh(&http(), &api, &first.access_token, "client-1").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ReauthRequired, "an ended token asks for the password again");
    yggdrasil::invalidate(&http(), &api, &renewed.access_token, "client-1").await;
    assert!(!yggdrasil::validate(&http(), &api, &renewed.access_token, "client-1").await.unwrap());
}
