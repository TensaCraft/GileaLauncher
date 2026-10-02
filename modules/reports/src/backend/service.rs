//! The module's work over the launcher's services: reports of alerts and of builds, and the
//! contact the reports carry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_core::builds::settings::split_jvm_arguments;
use launcher_core::core_app::CoreApp;
use launcher_core::feedback::{FeedbackService, ReportKind};
use launcher_core::launch::options::game_dir;
use launcher_core::launch::process::LAUNCH_LOG;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::branding::VERSION;
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Client;
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
    pub versions: Arc<VersionStore>,
    pub minecraft_dir: PathBuf,
    pub app_log: Option<PathBuf>,
    pub home: Option<PathBuf>,
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

/// The newest file in `dir` whose name `keep` accepts.
fn newest(dir: &Path, keep: impl Fn(&str) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| keep(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(_, path)| path.is_file())
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// A build's files a report of it attaches: the game's log, the launch log,
/// the newest crash report and the newest JVM crash log — those there are.
pub fn build_attachments(game_dir: &Path) -> Vec<PathBuf> {
    let logs = game_dir.join("logs");
    [
        Some(logs.join("latest.log")),
        Some(logs.join(LAUNCH_LOG)),
        newest(&game_dir.join("crash-reports"), |_| true),
        newest(game_dir, |name| name.starts_with("hs_err_") && name.ends_with(".log")),
    ]
    .into_iter()
    .flatten()
    .filter(|path| path.is_file())
    .collect()
}

fn names(files: &[PathBuf]) -> Vec<String> {
    files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).collect()
}

impl Reports {
    pub fn of(core: &CoreApp) -> AppResult<Reports> {
        Ok(Reports {
            http: client()?,
            endpoint: endpoint(),
            config: core.config.clone(),
            feedback: core.feedback.clone(),
            versions: core.versions.clone(),
            minecraft_dir: core.paths.minecraft_dir.clone(),
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

    fn build_of(&self, key: &str) -> AppResult<(Build, PathBuf)> {
        let build = self.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })?;
        let dir = game_dir(&build, &self.minecraft_dir);
        Ok((build, dir))
    }

    /// The names of the files a report of build `key` attaches.
    pub fn attachments(&self, key: &str) -> AppResult<Vec<String>> {
        let (_, dir) = self.build_of(key)?;
        Ok(names(&build_attachments(&dir)))
    }

    /// Reports build `key` with the user's description; `contact` is kept for next time.
    pub async fn send_build(&self, key: &str, message: &str, contact: &str) -> AppResult<String> {
        if message.trim().is_empty() {
            return Err(AppError::new(ErrorCode::InvalidInput, "a build report needs a description"));
        }
        let (build, dir) = self.build_of(key)?;
        self.set_contact(contact)?;
        let files = build_attachments(&dir);
        let (max_ram_gb, jvm_arguments) = split_jvm_arguments(&build.options);
        let option = |key: &str| build.options.get(key).cloned().unwrap_or(Value::Null);
        let metadata = json!({
            "version_id": build.key, "version_name": build.name, "client": build.client, "loader": build.loader,
            "minecraft": build.version, "path": dir.to_string_lossy(), "java_path": option("executablePath"),
            "gpu_mode": option("gpuMode"), "max_ram_gb": max_ram_gb, "jvm_arguments": jvm_arguments,
            "server": option("server"), "force_update": build.force_update, "attachments": names(&files),
        });
        self.deliver(ReportInput {
            kind: ReportKind::Error,
            title: format!("Version report: {}", build.name),
            message: message.trim().to_string(),
            screen: "version".into(),
            action: "manual_version_report".into(),
            metadata,
            attachments: files,
        })
        .await
    }
}
