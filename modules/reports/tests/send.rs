use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use launcher_core::feedback::{FeedbackService, NullSink, ReportContext, ReportKind};
use launcher_core::launch::options::game_dir;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{ErrorCode, Level, Text};
use module_reports::backend::send::{client, send};
use module_reports::backend::service::Reports;
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
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    Reports {
        http: client().unwrap(),
        endpoint: endpoint.into(),
        config: Arc::new(ConfigStore::open(dir.join("config.json"))),
        feedback: FeedbackService::new(Arc::new(NullSink)),
        versions: Arc::new(VersionStore::open(&state, &dir.join("mc"))),
        minecraft_dir: dir.join("mc"),
        app_log: None,
        home: None,
    }
}

fn aero(reports: &Reports) -> (Build, PathBuf) {
    let mut build = Build::new("Aero");
    build.version = Some("26.3".into());
    build.client = Some("Fabric".into());
    reports.versions.create(&mut build).unwrap();
    let dir = game_dir(&build, &reports.minecraft_dir);
    (build, dir)
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
async fn a_build_report_needs_a_message() {
    let (url, seen) = server(200, r#"{"ok": true}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), &url);
    let (build, _) = aero(&reports);
    let e = reports.send_build(&build.key, "  ", "").await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    assert!(bodies(&seen).is_empty(), "nothing is sent");
}

#[tokio::test]
async fn a_build_report_attaches_the_build_s_logs() {
    let (url, seen) = server(200, r#"{"ok": true, "report_id": "R-7"}"#).await;
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), &url);
    let (build, game) = aero(&reports);
    write(game.join("logs").join("latest.log"), "game log");
    write(game.join("logs").join("launch.log"), "launch log");
    let old = write(game.join("crash-reports").join("crash-old.txt"), "old crash");
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    write(game.join("crash-reports").join("crash-new.txt"), "new crash");
    write(game.join("hs_err_pid1.log"), "jvm died");
    assert_eq!(
        reports.attachments(&build.key).unwrap(),
        ["latest.log", "launch.log", "crash-new.txt", "hs_err_pid1.log"]
    );
    let id = reports.send_build(&build.key, "It crashes", " me@example.com ").await.unwrap();
    assert_eq!(id, "R-7");
    let body = bodies(&seen).remove(0);
    assert_eq!(body["type"], "error");
    assert_eq!(body["message"], "It crashes");
    assert_eq!(body["contact"], "me@example.com");
    let metadata = &body["metadata"];
    assert_eq!(metadata["action"], "manual_version_report");
    assert_eq!(metadata["version_name"], "Aero");
    assert_eq!(metadata["minecraft"], "26.3");
    let log = body["log"].as_str().unwrap();
    assert!(log.contains("--- diagnostic file: crash-new.txt ---\nnew crash"), "{log}");
    assert!(!log.contains("old crash"), "only the newest crash report");
    assert!(log.contains("jvm died") && log.contains("launch log"));
    assert_eq!(reports.contact(), "me@example.com", "the contact is kept for next time");
}

#[test]
fn a_build_report_leaves_out_crash_files_of_long_ago() {
    // A report about a crash sent the game's JVM error file of a month before (from another
    // launcher's run of the folder): only crash files of the last day explain what just happened.
    let dir = tempfile::tempdir().unwrap();
    let reports = reports(dir.path(), "");
    let (build, game) = aero(&reports);
    write(game.join("logs").join("latest.log"), "game log");
    let month = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
    for old in [game.join("hs_err_pid32976.log"), game.join("crash-reports").join("crash-2026-08-29.txt")] {
        write(old.clone(), "long ago");
        File::options().write(true).open(&old).unwrap().set_modified(month).unwrap();
    }
    assert_eq!(reports.attachments(&build.key).unwrap(), ["latest.log"]);
    write(game.join("hs_err_pid34608.log"), "now");
    assert_eq!(reports.attachments(&build.key).unwrap(), ["latest.log", "hs_err_pid34608.log"]);
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
            kind: ReportKind::Crash,
            title: "Minecraft exited after launch".into(),
            screen: "game".into(),
            action: "launch".into(),
            metadata: json!({"version_name": "Aero"}),
            attachments: vec![log],
        },
    );
    reports.send_alert(id, "Попередження", "Minecraft crashed").await.unwrap();
    let body = bodies(&seen).remove(0);
    assert_eq!(body["type"], "crash");
    assert_eq!(body["title"], "Minecraft exited after launch");
    assert_eq!(body["message"], "Minecraft crashed");
    assert_eq!(body["metadata"]["action"], "launch");
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
