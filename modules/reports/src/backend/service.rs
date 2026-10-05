//! The module's work over the launcher's services: reports of the launcher's own problems (a
//! failed operation, a problem the user describes, a crash of the launcher) and the contact the
//! reports carry. A game's crash is its build's (its mods), not the launcher's: none is sent.

use std::path::PathBuf;
use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::crash::{self, Crash};
use launcher_core::feedback::{FeedbackService, ReportKind};
use launcher_core::storage::config::ConfigStore;
use launcher_shared::branding::VERSION;
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use super::payload::{Env, ReportInput, build, feedback_snapshot};
use super::send::{client, endpoint, send};

/// The contact reports carry (deleted when empty).
pub const CONTACT_KEY: &str = "report_contact";

pub struct Reports {
    pub http: Client,
    pub endpoint: String,
    pub config: Arc<ConfigStore>,
    pub feedback: Arc<FeedbackService>,
    /// Where the launcher's own crashes wait (`launcher_core::crash`).
    pub crash_dir: PathBuf,
    pub app_log: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

/// The error the user saw when they report a failure: its text, code and detail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProblemError {
    pub title: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// `windows`, `macos` or `linux`.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

fn io(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

impl Reports {
    pub fn of(core: &CoreApp) -> AppResult<Reports> {
        Ok(Reports {
            http: client()?,
            endpoint: endpoint(),
            config: core.config.clone(),
            feedback: core.feedback.clone(),
            crash_dir: core.crash_dir(),
            app_log: Some(core.log_file()),
            home: launcher_core::paths::PathEnv::from_system().home.into(),
        })
    }

    /// Reports have somewhere to go.
    pub fn enabled(&self) -> bool {
        !self.endpoint.trim().is_empty()
    }

    pub fn contact(&self) -> String {
        self.config.get_str(CONTACT_KEY).map(|c| c.trim().to_string()).unwrap_or_default()
    }

    pub fn set_contact(&self, contact: &str) -> AppResult<()> {
        match contact.trim() {
            "" => self.config.delete(CONTACT_KEY),
            contact => self.config.set(CONTACT_KEY, json!(contact)),
        }
        .map_err(io)
    }

    fn env(&self) -> Env {
        let contact = self.contact();
        Env {
            version: VERSION.into(),
            platform: platform(),
            os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            home: self.home.clone(),
            app_log: self.app_log.clone(),
            contact: (!contact.is_empty()).then_some(contact),
            feedback: feedback_snapshot(&self.feedback),
        }
    }

    async fn deliver(&self, input: ReportInput) -> AppResult<String> {
        let env = self.env();
        // Reading the logs is file work: off the async threads.
        let payload = tokio::task::spawn_blocking(move || build(input, &env))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        send(&self.http, &self.endpoint, &payload).await
    }

    /// Reports alert `id` as the user saw it (`title`, `message`), with what its context holds.
    pub async fn send_alert(&self, id: u64, title: &str, message: &str) -> AppResult<String> {
        let report = self.feedback.report_context(id).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("the report of alert {id} is no longer kept"))
        })?;
        let mut metadata = report.metadata;
        if let Value::Object(fields) = &mut metadata
            && !title.trim().is_empty()
        {
            fields.insert("alert_title".into(), json!(title.trim()));
        }
        self.deliver(ReportInput {
            kind: report.kind,
            title: report.title,
            message: message.trim().to_string(),
            screen: report.screen,
            action: report.action,
            metadata,
            attachments: report.attachments,
        })
        .await
    }

    /// Reports a problem with the launcher: the user's words, the error they saw (from a failed
    /// operation) or both, with the launcher's log. `contact` is kept for next time.
    pub async fn send_problem(
        &self,
        message: &str,
        contact: &str,
        error: Option<ProblemError>,
    ) -> AppResult<String> {
        let error = error.filter(|e| !e.title.trim().is_empty());
        if message.trim().is_empty() && error.is_none() {
            return Err(AppError::new(ErrorCode::InvalidInput, "a report needs a description"));
        }
        self.set_contact(contact)?;
        let title = match error.as_ref().and_then(|e| e.code.as_deref()) {
            Some(code) => format!("Launcher error: {code}"),
            None => "Launcher problem".to_string(),
        };
        let message = match (message.trim(), &error) {
            ("", Some(e)) => e.title.trim().to_string(),
            (words, _) => words.to_string(),
        };
        let metadata = match &error {
            Some(e) => json!({"error_title": e.title, "error_code": e.code, "error_detail": e.detail}),
            None => json!({}),
        };
        self.deliver(ReportInput {
            kind: ReportKind::Error,
            title,
            message,
            screen: "launcher".into(),
            action: "problem_report".into(),
            metadata,
            attachments: Vec::new(),
        })
        .await
    }

    /// The launcher's own crash waiting to be reported, the newest.
    pub fn last_crash(&self) -> Option<Crash> {
        crash::pending(&self.crash_dir)
    }

    /// Reports the waiting crash with the user's words (if any) and the launcher's log; once
    /// sent, no crash waits any more.
    pub async fn send_crash(&self, message: &str, contact: &str) -> AppResult<String> {
        let crash = self.last_crash().ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, "no crash of the launcher waits to be reported")
        })?;
        self.set_contact(contact)?;
        let words = message.trim();
        let id = self
            .deliver(ReportInput {
                kind: ReportKind::Crash,
                title: "Launcher crashed".into(),
                message: if words.is_empty() { crash.message.clone() } else { words.to_string() },
                screen: "launcher".into(),
                action: "panic".into(),
                metadata: json!({
                    "panic_message": crash.message, "panic_location": crash.location,
                    "crashed_version": crash.version, "crashed_at": crash.at, "thread": crash.thread,
                }),
                attachments: Vec::new(),
            })
            .await?;
        crash::clear(&self.crash_dir);
        Ok(id)
    }

    /// Forgets the waiting crash: the user chose not to report it.
    pub fn dismiss_crash(&self) {
        crash::clear(&self.crash_dir);
    }
}
