//! Launcher self-update from the GitHub releases of the profile's repository.

pub mod apply;
pub mod download;
pub mod github;
pub mod select;
pub mod stage;
pub mod version;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_shared::branding::{APP_NAME, DEFAULT_UPDATE_API};
use launcher_shared::{AppError, AppResult, ErrorCode, Level, Text, UpdateInfo, UpdateState, UpdateStatus};

use crate::feedback::{EventSink, FeedbackService, OperationSpec};
use crate::net::downloader::Downloader;
use crate::paths::PathEnv;
use download::DownloadRequest;
use github::GithubClient;
use select::{Candidate, GhAsset, Platform};
use stage::ExecContext;
use version::Version;

pub struct UpdateConfig {
    pub api_base: String,
    /// `owner/repo`; empty disables updates.
    pub repo: String,
    pub current_version: String,
    pub cache_dir: PathBuf,
    pub dev_mode: bool,
    pub platform: Platform,
    /// `None` when the running executable is unknown: updates can be found but not installed.
    pub exec: Option<ExecContext>,
    pub env: PathEnv,
    pub updated_from: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Selected {
    pub info: UpdateInfo,
    pub asset: GhAsset,
    pub sha256: String,
}

/// Assets without a SHA-256 digest are never installed.
pub fn selection(candidate: &Candidate, asset: GhAsset) -> AppResult<Selected> {
    let sha256 = asset.sha256().ok_or_else(|| {
        AppError::new(ErrorCode::IntegrityMismatch, "release asset has no sha256 digest")
            .with_param("asset", &asset.name)
    })?;
    Ok(Selected { info: candidate.info(&asset), asset, sha256 })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn error_text(e: &AppError) -> Text {
    Text::Key { key: e.code.i18n_key().to_string(), params: e.params.clone() }
}

/// Why a verified download cannot be installed, in words the user can act on.
fn prepare_message(e: &AppError) -> Text {
    match (e.code, e.params.get("path")) {
        (ErrorCode::InvalidDirectoryPath, Some(path)) => {
            Text::key("update_install_dir_readonly").param("path", path)
        }
        _ => error_text(e),
    }
}

struct Inner {
    status: UpdateStatus,
    selected: Option<Selected>,
    busy: bool,
}

pub struct UpdateService {
    cfg: UpdateConfig,
    client: Option<GithubClient>,
    current: Option<Version>,
    feedback: Arc<FeedbackService>,
    sink: Arc<dyn EventSink>,
    downloader: Arc<Downloader>,
    inner: Mutex<Inner>,
}

impl UpdateService {
    pub fn new(
        cfg: UpdateConfig,
        feedback: Arc<FeedbackService>,
        sink: Arc<dyn EventSink>,
        downloader: Arc<Downloader>,
    ) -> Arc<Self> {
        let current = Version::parse(&cfg.current_version);
        let client = if cfg.repo.trim().is_empty() || current.is_none() {
            None
        } else {
            match GithubClient::new(&cfg.api_base, &cfg.repo, &format!("{APP_NAME}/{}", cfg.current_version))
            {
                Ok(client) => Some(client),
                Err(e) => {
                    tracing::warn!("Launcher updates are disabled: {}", e.detail);
                    None
                }
            }
        };
        let configured = client.is_some();
        let apply_supported = configured && !cfg.dev_mode && cfg.exec.is_some();
        let source = (cfg.api_base.trim_end_matches('/') != DEFAULT_UPDATE_API).then(|| cfg.api_base.clone());
        let status = UpdateStatus::idle(
            configured,
            cfg.current_version.clone(),
            source,
            apply_supported,
            cfg.updated_from.clone(),
        );
        Arc::new(Self {
            cfg,
            client,
            current,
            feedback,
            sink,
            downloader,
            inner: Mutex::new(Inner { status, selected: None, busy: false }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> UpdateStatus {
        self.lock().status.clone()
    }

    fn set_state(&self, state: UpdateState) -> UpdateStatus {
        let snapshot = {
            let mut inner = self.lock();
            inner.status.state = state;
            inner.status.clone()
        };
        self.sink.update(&snapshot);
        snapshot
    }

    /// Only one check/download at a time; a second request just gets the current status.
    fn try_begin(&self) -> bool {
        let mut inner = self.lock();
        if inner.busy {
            false
        } else {
            inner.busy = true;
            true
        }
    }

    fn end(&self) {
        self.lock().busy = false;
    }

    pub async fn check(&self, include_beta: bool, manual: bool) -> UpdateStatus {
        let Some(client) = self.client.as_ref() else { return self.status() };
        if !self.try_begin() {
            return self.status();
        }
        self.set_state(UpdateState::Checking);
        let result = self.find_update(client, include_beta).await;
        self.lock().status.last_checked_ms = Some(now_ms());
        let status = match result {
            Ok(Some(selected)) => {
                let info = selected.info.clone();
                self.lock().selected = Some(selected);
                self.set_state(UpdateState::Available { info })
            }
            Ok(None) => {
                self.lock().selected = None;
                if manual {
                    self.feedback.success(Text::key("update_check_no_updates"));
                }
                self.set_state(UpdateState::UpToDate)
            }
            Err(error) => {
                tracing::warn!("Launcher update check failed: {error}");
                if manual {
                    let title = Text::key("launcher_update_status_failed");
                    self.feedback.toast(Level::Warning, title, Some(error_text(&error)), None);
                }
                self.set_state(UpdateState::Failed { error })
            }
        };
        self.end();
        status
    }

    async fn find_update(&self, client: &GithubClient, include_beta: bool) -> AppResult<Option<Selected>> {
        let Some(current) = self.current.as_ref() else { return Ok(None) };
        let releases = client.releases().await?;
        let Some(candidate) = select::pick_release(releases, current, include_beta) else { return Ok(None) };
        let mut asset = select::pick_asset(&candidate.release.assets, &self.cfg.platform).cloned();
        if asset.is_none() {
            let all = client.release_assets(candidate.release.id).await?;
            asset = select::pick_asset(&all, &self.cfg.platform).cloned();
        }
        let asset = asset.ok_or_else(|| {
            AppError::new(ErrorCode::NoUpdateAsset, "the release has no file for this platform")
                .with_param("version", candidate.version.as_str())
        })?;
        selection(&candidate, asset).map(Some)
    }

    pub async fn download_and_prepare(&self) -> UpdateStatus {
        let Some(client) = self.client.as_ref() else { return self.status() };
        let Some(selected) = self.lock().selected.clone() else { return self.status() };
        if !self.try_begin() {
            return self.status();
        }
        let info = selected.info.clone();
        self.set_state(UpdateState::Downloading { info: info.clone() });
        let title = Text::key("update_downloading_version").param("version", &info.version);
        let op = self.feedback.begin(OperationSpec::new(title, "update"));
        let dest = stage::download_path(&self.cfg.cache_dir, &info.version, &selected.asset.name);
        let req = DownloadRequest {
            url: &selected.asset.browser_download_url,
            dest: &dest,
            size: selected.asset.size,
            sha256: &selected.sha256,
        };
        let downloaded = download::download(&self.downloader, client, &req, |done, total| {
            op.progress(done as f64, total as f64)
        })
        .await;
        let result = match downloaded {
            Ok(path) => self
                .prepare(&path, &info.version, &selected.sha256)
                .map_err(|e| (Text::key("update_prepare_failed"), prepare_message(&e), e)),
            Err(e) => Err((Text::key("update_download_failed"), error_text(&e), e)),
        };
        let status = match result {
            Ok(()) => {
                op.finish();
                self.set_state(UpdateState::Ready { info })
            }
            Err((title, message, error)) => {
                tracing::warn!("Launcher update failed: {error}");
                op.fail(message.clone());
                self.feedback.toast(Level::Error, title, Some(message), None);
                self.set_state(UpdateState::Failed { error })
            }
        };
        self.end();
        status
    }

    fn prepare(&self, payload: &Path, version: &str, sha256: &str) -> AppResult<()> {
        if !self.lock().status.apply_supported {
            return Ok(()); // development mode: stop after the verified download
        }
        let exec = self
            .cfg
            .exec
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "unknown executable"))?;
        stage::prepare(&self.cfg.cache_dir, exec, &self.cfg.env, payload, version, sha256).map(|_| ())
    }

    /// Starts the helper; the caller must exit the process right after. Only once: the service
    /// stays busy afterwards, so a second click cannot start a competing helper.
    pub fn apply(&self) -> AppResult<()> {
        let (status, selected) = {
            let inner = self.lock();
            (inner.status.clone(), inner.selected.clone())
        };
        if !matches!(status.state, UpdateState::Ready { .. }) {
            return Err(AppError::new(ErrorCode::InvalidInput, "no prepared update"));
        }
        if !status.apply_supported {
            return Err(AppError::new(
                ErrorCode::Unsupported,
                "installing updates is disabled in development mode",
            ));
        }
        let exec = self
            .cfg
            .exec
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "unknown executable"))?;
        if !self.try_begin() {
            return Err(AppError::new(ErrorCode::Busy, "the update is already being installed"));
        }
        let result = self.start_helper(exec, selected.as_ref());
        if result.is_err() {
            self.end();
        }
        result
    }

    fn start_helper(&self, exec: &ExecContext, selected: Option<&Selected>) -> AppResult<()> {
        let cache = &self.cfg.cache_dir;
        let verified = stage::validate_for(cache, exec).and_then(|marker| {
            let downloaded = selected.map(|s| (s.sha256.as_str(), s.info.version.as_str()));
            if downloaded == Some((marker.source_sha256.as_str(), marker.version.as_str())) {
                Ok(marker)
            } else {
                Err(AppError::new(
                    ErrorCode::IntegrityMismatch,
                    "the prepared update is not the verified download",
                ))
            }
        });
        let marker = match verified {
            Ok(marker) => marker,
            Err(error) => {
                tracing::warn!("Discarding the prepared launcher update: {error}");
                stage::discard_pending(cache);
                self.set_state(UpdateState::Failed { error: error.clone() });
                return Err(error);
            }
        };
        stage::spawn_helper(cache, &marker, exec.pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::UpdateChannel;

    #[test]
    fn asset_without_digest_is_rejected() {
        let asset = GhAsset {
            id: 1,
            name: "Launcher.exe".into(),
            size: 5,
            state: None,
            digest: None,
            browser_download_url: "https://github.com/o/r/releases/download/v1/Launcher.exe".into(),
        };
        let release = select::GhRelease {
            id: 9,
            tag_name: "v0.2.0".into(),
            name: None,
            body: None,
            draft: false,
            prerelease: false,
            published_at: None,
            assets: vec![asset.clone()],
        };
        let candidate =
            Candidate { release, version: Version::parse("0.2.0").unwrap(), channel: UpdateChannel::Stable };
        assert_eq!(selection(&candidate, asset).unwrap_err().code, ErrorCode::IntegrityMismatch);
    }
}
