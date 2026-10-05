use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use launcher_core::crash::{self, Crash};
use launcher_core::feedback::{FeedbackService, NullSink, ReportContext, ReportKind};
use launcher_core::storage::config::ConfigStore;
use launcher_shared::{ErrorCode, Level, Text};
use module_reports::backend::send::{client, send};
use module_reports::backend::service::{ProblemError, Reports};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[derive(Default)]
struct Seen {
    bodies: Vec<Value>,
    reply: (u16, String),
}

type Shared = Arc<Mutex<Seen>>;

async fn receive(State(seen): State<Shared>, body: String) -> (StatusCode, String) {
    let mut seen = seen.lock().unwrap();
    seen.bodies.push(serde_json::from_str(&body).unwrap_or(Value::Null));
    (StatusCode::from_u16(seen.reply.0).unwrap(), seen.reply.1.clone())
}

/// A reports server on loopback answering `status`/`body`; its address and what it received.
async fn server(status: u16, body: &str) -> (String, Shared) {
    let seen: Shared = Arc::new(Mutex::new(Seen { bodies: Vec::new(), reply: (status, body.to_string()) }));
    let app = Router::new().route("/logs", post(receive)).with_state(seen.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/logs", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

fn bodies(seen: &Shared) -> Vec<Value> {
    seen.lock().unwrap().bodies.clone()
}

fn reports(dir: &Path, endpoint: &str) -> Reports {
    Reports {
        http: client().unwrap(),
        endpoint: endpoint.into(),
        config: Arc::new(ConfigStore::open(dir.join("config.json"))),
        feedback: FeedbackService::new(Arc::new(NullSink)),
        crash_dir: dir.join("crashes"),
        app_log: None,
        home: None,
    }
}

fn write(path: PathBuf, body: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

#[tokio::test]
async fn a_sent_report_returns_its_id() {
    let (url, seen) = server(200, r#"{"ok": true, "report_id": 42}"#).await;
    let id = send(&client().unwrap(), &url, &json!({"type": "error", "log": "x"})).await.unwrap();
    assert_eq!(id, "42");
    assert_eq!(bodies(&seen), [json!({"type": "error", "log": "x"})]);
}

#[tokio::test]
async fn an_answer_without_ok_is_a_failure() {
    let (url, _) = server(200, r#"{"ok": false, "error": "too big"}"#).await;
    let e = send(&client().unwrap(), &url, &json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Network);
    assert!(e.detail.contains("too big"), "{}", e.detail);
    assert_eq!(e.params.get("reason").map(String::as_str), Some("too big"), "the server's own words");
}

#[tokio::test]
async fn a_failed_send_is_an_error_with_the_server_s_reason() {
    let (url, _) = server(500, "boom").await;
    let e = send(&client().unwrap(), &url, &json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Network);
    assert!(e.detail.contains("500") && e.detail.contains("boom"), "{}", e.detail);
    let reason = e.params.get("reason").expect("the server answered: its reason is shown");
    assert!(reason.contains("500") && reason.contains("boom"), "{reason}");
}

#[tokio::test]
async fn no_endpoint_no_report() {
    let e = send(&client().unwrap(), " ", &json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    let dir = tempfile::tempdir().unwrap();
    assert!(!reports(dir.path(), "").enabled());
    assert!(reports(dir.path(), "http://127.0.0.1:9/logs").enabled());
}

#[tokio::test]
async fn a_problem_report_needs_words_or_an_error() {
    let (url, seen) = server(200, r#"{"ok": true}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let e = reports(dir.path(), &url).send_problem("  ", "", None).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    assert!(bodies(&seen).is_empty(), "nothing is sent");
}

#[tokio::test]
async fn a_problem_report_carries_the_launcher_s_log_and_the_error_not_the_game_s() {
    let (url, seen) = server(200, r#"{"ok": true, "report_id": "R-7"}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let mut reports = reports(dir.path(), &url);
    reports.app_log = Some(write(dir.path().join("app.log"), "INFO Installing Aero"));
    write(dir.path().join("mc").join("games").join("aero").join("logs").join("latest.log"), "game log");
    let error = ProblemError {
        title: "Не вдалося встановити Aero".into(),
        code: Some("download_failed".into()),
        detail: Some("HTTP 503 from piston-data.mojang.com".into()),
    };
    let id = reports.send_problem("It hangs at 40%", " me@example.com ", Some(error.clone())).await.unwrap();
    assert_eq!(id, "R-7");
    let body = bodies(&seen).remove(0);
    assert_eq!(
        (body["type"].as_str(), body["title"].as_str()),
        (Some("error"), Some("Launcher error: download_failed"))
    );
    assert_eq!(body["message"], "It hangs at 40%");
    assert_eq!(body["contact"], "me@example.com");
    let metadata = &body["metadata"];
    assert_eq!(metadata["action"], "problem_report");
    assert_eq!(
        (metadata["error_title"].as_str(), metadata["error_detail"].as_str()),
        (Some(error.title.as_str()), error.detail.as_deref())
    );
    let log = body["log"].as_str().unwrap();
    assert!(log.contains("Installing Aero"), "{log}");
    assert!(!log.contains("game log"), "no game's files go");
    assert_eq!(reports.contact(), "me@example.com", "the contact is kept for next time");
    // With the error alone, its words are the message.
    reports.send_problem("", "", Some(error.clone())).await.unwrap();
    assert_eq!(bodies(&seen)[1]["message"], error.title.as_str());
}

#[tokio::test]
async fn a_launcher_crash_is_offered_until_sent() {
    let (url, seen) = server(200, r#"{"ok": true, "report_id": 9}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), &url);
    assert_eq!(reports.last_crash(), None);
    let crash = Crash::now("index out of bounds", "crates/launcher-core/src/x.rs:10:5", "main", None);
    crash::write(&reports.crash_dir, &crash).unwrap();
    assert_eq!(reports.last_crash(), Some(crash.clone()));
    reports.send_crash("I pressed Play", "").await.unwrap();
    let body = bodies(&seen).remove(0);
    assert_eq!((body["type"].as_str(), body["title"].as_str()), (Some("crash"), Some("Launcher crashed")));
    assert_eq!(body["message"], "I pressed Play");
    assert_eq!(body["metadata"]["panic_location"], crash.location.as_str());
    assert_eq!(body["metadata"]["panic_message"], crash.message.as_str());
    assert_eq!(reports.last_crash(), None, "sent: not offered again");
}

#[tokio::test]
async fn a_launcher_crash_not_sent_waits_or_is_dismissed() {
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), "http://127.0.0.1:9/logs");
    crash::write(&reports.crash_dir, &Crash::now("boom", "src/a.rs:1:1", "main", None)).unwrap();
    assert!(reports.send_crash("", "").await.is_err(), "offline");
    assert!(reports.last_crash().is_some(), "it waits for the next try");
    reports.dismiss_crash();
    assert_eq!(reports.last_crash(), None);
}

#[tokio::test]
async fn the_contact_is_saved_and_an_empty_one_removed() {
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), "");
    assert_eq!(reports.contact(), "");
    reports.set_contact(" me ").unwrap();
    assert_eq!(reports.contact(), "me");
    assert_eq!(reports.config.get_str("report_contact").as_deref(), Some("me"));
    reports.set_contact("  ").unwrap();
    assert_eq!(reports.config.get_str("report_contact"), None, "an empty contact is removed");
}

#[tokio::test]
async fn an_alert_report_uses_its_context() {
    let (url, seen) = server(200, r#"{"ok": true, "report_id": 1}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), &url);
    let log = write(dir.path().join("latest.log"), "OutOfMemoryError");
    let id = reports.feedback.alert_with_report(
        Level::Error,
        Text::key("warning"),
        Text::raw("crash"),
        ReportContext {
            kind: ReportKind::Error,
            title: "Server build install failed".into(),
            screen: "Home".into(),
            action: "tensacraft_install".into(),
            metadata: json!({"version_name": "Aero"}),
            attachments: vec![log],
        },
    );
    reports.send_alert(id, "Попередження", "Install failed").await.unwrap();
    let body = bodies(&seen).remove(0);
    assert_eq!(body["type"], "error");
    assert_eq!(body["title"], "Server build install failed");
    assert_eq!(body["message"], "Install failed");
    assert_eq!(body["metadata"]["action"], "tensacraft_install");
    assert_eq!(body["metadata"]["version_name"], "Aero");
    assert!(body["log"].as_str().unwrap().contains("OutOfMemoryError"));
}

#[tokio::test]
async fn a_forgotten_alert_cannot_be_reported() {
    let (url, seen) = server(200, r#"{"ok": true}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let e = reports(dir.path(), &url).send_alert(999, "", "").await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(bodies(&seen).is_empty());
}
