//! «Сайт TensaCraft» against a local imitation of Azuriom's auth API (`/api/auth/*`).

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use launcher_core::auth::provider::AccountProvider;
use launcher_shared::ErrorCode;
use module_tensa::backend::site::TensaSite;
use serde_json::{Value, json};

const GAME_ID: &str = "2ca6c1be-e03b-32d5-a995-86a64e1a3b2a";

#[derive(Clone, Default)]
struct Site {
    two_factor: bool,
    disabled: bool,
    token: Arc<Mutex<Option<String>>>,
    logged_out: Arc<Mutex<Vec<String>>>,
}

fn user(token: &str) -> Value {
    json!({ "id": 1, "username": "Builder", "uuid": GAME_ID, "access_token": token, "banned": false })
}

fn refused(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

async fn authenticate(State(s): State<Site>, Json(body): Json<Value>) -> Response {
    if s.disabled {
        return refused(
            StatusCode::BAD_REQUEST,
            json!({"status": "error", "message": "Auth API is not enabled"}),
        );
    }
    if body["email"] != "Builder" || body["password"] != "pw" {
        return refused(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"status": "error", "reason": "invalid_credentials"}),
        );
    }
    if s.two_factor {
        match body["code"].as_str() {
            None | Some("") => {
                return refused(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({"status": "pending", "reason": "2fa", "message": "Missing 2FA code"}),
                );
            }
            Some("123456") => {}
            Some(_) => {
                return refused(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({"status": "error", "reason": "invalid_2fa"}),
                );
            }
        }
    }
    *s.token.lock().unwrap() = Some("site-token".into());
    Json(user("site-token")).into_response()
}

async fn verify(State(s): State<Site>, Json(body): Json<Value>) -> Response {
    let known = s.token.lock().unwrap().clone();
    if known.is_some() && body["access_token"].as_str() == known.as_deref() {
        Json(user(known.as_deref().unwrap())).into_response()
    } else {
        refused(StatusCode::UNAUTHORIZED, json!({"status": "error", "reason": "invalid_token"}))
    }
}

async fn logout(State(s): State<Site>, Json(body): Json<Value>) -> Response {
    s.logged_out.lock().unwrap().push(body["access_token"].as_str().unwrap_or_default().to_string());
    *s.token.lock().unwrap() = None;
    Json(json!({"status": "success"})).into_response()
}

async fn start(site: Site) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/auth/authenticate", post(authenticate))
        .route("/api/auth/verify", post(verify))
        .route("/api/auth/logout", post(logout))
        .with_state(site);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, task)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_site_signs_a_player_in_and_vouches_for_them_until_they_sign_out() {
    let site = Site::default();
    let (base, _task) = start(site.clone()).await;
    let tensa = TensaSite::new(&base).unwrap();
    assert_eq!(tensa.id(), "tensa");
    let account = tensa.sign_in("Builder", "pw", None).await.unwrap();
    assert_eq!(
        (account.id.as_str(), account.name.as_str(), account.token.as_str()),
        (GAME_ID, "Builder", "site-token")
    );
    assert_eq!(tensa.verify("site-token").await.unwrap().name, "Builder");
    tensa.sign_out("site-token").await.unwrap();
    assert_eq!(*site.logged_out.lock().unwrap(), ["site-token"]);
    assert_eq!(tensa.verify("site-token").await.unwrap_err().code, ErrorCode::ReauthRequired);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_a_second_factor_and_a_closed_api_each_say_so() {
    let site = Site { two_factor: true, ..Site::default() };
    let (base, _task) = start(site).await;
    let tensa = TensaSite::new(&base).unwrap();
    assert_eq!(
        tensa.sign_in("Builder", "nope", None).await.unwrap_err().code,
        ErrorCode::AuthCredentialsInvalid
    );
    assert_eq!(tensa.sign_in("Builder", "pw", None).await.unwrap_err().code, ErrorCode::AuthCodeRequired);
    assert_eq!(
        tensa.sign_in("Builder", "pw", Some("000000")).await.unwrap_err().code,
        ErrorCode::AuthCredentialsInvalid
    );
    assert!(tensa.sign_in("Builder", "pw", Some("123456")).await.is_ok());

    let closed = Site { disabled: true, ..Site::default() };
    let (base, _task) = start(closed).await;
    let err = TensaSite::new(&base).unwrap().sign_in("Builder", "pw", None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthServerInvalid);
}

#[test]
fn the_skin_is_the_site_s_skin_of_that_nick() {
    let tensa = TensaSite::new("https://site.example").unwrap();
    assert_eq!(
        tensa.skin_url("Builder").as_deref(),
        Some("https://site.example/api/skin-api/skins/Builder.png")
    );
}
