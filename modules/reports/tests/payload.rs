use std::path::{Path, PathBuf};

use launcher_core::feedback::ReportKind;
use module_reports::backend::log::{FILE_CAP, LOG_CAP, combined, excerpt};
use module_reports::backend::payload::{Env, ReportInput, build};
use module_reports::backend::redact::Redactor;
use serde_json::{Value, json};

const HOME: &str = r"C:\Users\Steve";
const SECRETS: [&str; 7] = ["Steve", "steve", "abc.def", "tok123", "zzz", "yyy", "www"];
const RAW: &str = r#"C:\Users\Steve\AppData c:/users/STEVE/x --accessToken abc.def Authorization: Bearer tok123 {"access_token": "zzz", refresh-token=yyy, client_secret: 'www'}"#;

fn env(app_log: Option<PathBuf>, contact: Option<&str>) -> Env {
    Env {
        version: "1.2.3".into(),
        platform: "windows",
        os: "Windows 11".into(),
        home: Some(PathBuf::from(HOME)),
        app_log,
        contact: contact.map(str::to_string),
        feedback: json!({"busy": false, "active_operations": [], "recent_activity": []}),
    }
}

fn input(title: &str, message: &str, attachments: Vec<PathBuf>) -> ReportInput {
    ReportInput {
        kind: ReportKind::Error,
        title: title.into(),
        message: message.into(),
        screen: "Home".into(),
        action: "tensacraft_install".into(),
        metadata: json!({"pack_id": "aero"}),
        attachments,
    }
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn tokens_and_home_are_hidden_everywhere() {
    let out = Redactor::new(Some(Path::new(HOME))).text(RAW);
    for secret in SECRETS {
        assert!(!out.contains(secret), "{secret} left in {out}");
    }
    assert_eq!(out.matches("<USER_HOME>").count(), 2, "both forms of the home folder: {out}");
    assert!(out.contains("--accessToken <redacted>") && out.contains("Bearer <redacted>"), "{out}");

    let dir = tempfile::tempdir().unwrap();
    let log = write(dir.path(), "launch.log", &format!("java --accessToken abc.def -Dhome={HOME}"));
    let mut report = input(RAW, RAW, vec![log]);
    report.metadata =
        json!({"nested": {"path": format!(r"{HOME}\x")}, "list": ["refresh_token=yyy"], "n": 3});
    let payload = build(report, &env(None, None));
    let text = payload.to_string();
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} left in the payload");
    }
    assert_eq!(payload["metadata"]["n"], 3, "numbers stay");
}

#[test]
fn legacy_session_tokens_are_hidden() {
    // Minecraft 1.7–1.8 print the session and very old ones pass it on the command line.
    let raw = "[Client thread/INFO]: (Session ID is token:abc123def:0f9e8d7c)
               java_command: net.minecraft.client.main.Main --session token:abc123def:0f9e8d7c --version 1.7.10";
    let out = Redactor::new(None).text(raw);
    assert!(!out.contains("abc123def"), "{out}");
    assert!(out.contains("Session ID is token:<redacted>") && out.contains("--session <redacted>"), "{out}");
}

#[test]
fn a_big_file_gives_its_head_and_tail() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!("HEAD{}TAIL", "x".repeat(3 * 1024 * 1024));
    let path = write(dir.path(), "latest.log", &body);
    let part = excerpt(&path, FILE_CAP);
    assert!(part.starts_with("HEAD") && part.ends_with("TAIL"));
    assert!(part.contains("--- middle of file omitted by "), "the cut is marked");
    assert!(part.len() <= FILE_CAP + 100, "{} bytes", part.len());
}

#[test]
fn the_combined_log_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> =
        (0..6).map(|n| write(dir.path(), &format!("{n}.log"), &"y".repeat(FILE_CAP))).collect();
    let log = combined(&files, None);
    assert!(log.len() <= LOG_CAP + 100, "{} bytes", log.len());
    assert!(log.contains("--- middle of combined log omitted by "));
    assert!(log.starts_with("--- diagnostic file: 0.log ---\n"));
}

#[test]
fn an_unreadable_attachment_is_noted() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.log");
    let app_log = write(dir.path(), "app.log", "launcher started");
    let log = combined(std::slice::from_ref(&missing), Some(&app_log));
    assert!(log.contains(&format!("Unable to read {}", missing.display())), "{log}");
    assert!(log.contains("--- launcher app.log ---\nlauncher started"));
}

#[test]
fn the_same_file_is_attached_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = write(dir.path(), "latest.log", "boom");
    let again = dir.path().join(".").join("latest.log");
    let log = combined(&[file, again], None);
    assert_eq!(log.matches("--- diagnostic file: latest.log ---").count(), 1, "{log}");
}

#[test]
fn a_payload_has_the_original_s_shape() {
    let payload =
        build(input("Install failed", "It broke", Vec::new()), &env(None, Some(" steve@example.com ")));
    for (key, value) in [
        ("type", json!("error")),
        ("severity", json!("error")),
        ("platform", json!("windows")),
        ("launcher_version", json!("1.2.3")),
        ("os", json!("Windows 11")),
        ("title", json!("Install failed")),
        ("message", json!("It broke")),
        ("contact", json!("steve@example.com")),
    ] {
        assert_eq!(payload[key], value, "{key}");
    }
    let metadata = &payload["metadata"];
    assert_eq!(metadata["screen"], "Home");
    assert_eq!(metadata["action"], "tensacraft_install");
    assert_eq!(metadata["runtime"], "rust");
    assert_eq!(metadata["pack_id"], "aero");
    assert_eq!(metadata["contact"], "steve@example.com");
    assert!(metadata["feedback"].is_object());

    let anonymous = build(input("", "It broke", Vec::new()), &env(None, Some("  ")));
    assert_eq!(anonymous["title"], "Launcher report");
    assert!(anonymous.get("contact").is_none() && anonymous["metadata"].get("contact").is_none());
    let crash = build(ReportInput { kind: ReportKind::Crash, ..input("", "", Vec::new()) }, &env(None, None));
    assert_eq!(crash["type"], "crash");
}

#[test]
fn an_empty_log_falls_back_to_the_message() {
    let payload = build(input("Install failed", "It broke", Vec::new()), &env(None, None));
    assert_eq!(payload["log"], "It broke");
    let payload = build(input("Install failed", " ", Vec::new()), &env(None, None));
    assert_eq!(payload["log"], "Install failed");
    assert_eq!(payload["message"], Value::String(" ".into()));
}
