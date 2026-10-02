//! Launcher self-update state shared by the backend and the UI.

use serde::{Deserialize, Serialize};

use crate::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    Stable,
    Beta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub channel: UpdateChannel,
    /// Release body (Markdown shown as plain text).
    pub notes: String,
    pub asset_name: String,
    pub size: u64,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available {
        info: UpdateInfo,
    },
    /// Progress is reported by the `update` operation in `app://ops`.
    Downloading {
        info: UpdateInfo,
    },
    Ready {
        info: UpdateInfo,
    },
    Failed {
        error: AppError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// `update_repo` is set for this build.
    pub configured: bool,
    pub current_version: String,
    /// `None` for api.github.com, otherwise the (test) API address.
    pub source: Option<String>,
    pub last_checked_ms: Option<u64>,
    /// False in development mode or when the running executable is unknown.
    pub apply_supported: bool,
    /// Previous version when this is the first start after an update.
    pub updated_from: Option<String>,
    pub state: UpdateState,
}

impl UpdateStatus {
    pub fn idle(
        configured: bool,
        current_version: String,
        source: Option<String>,
        apply_supported: bool,
        updated_from: Option<String>,
    ) -> Self {
        Self {
            configured,
            current_version,
            source,
            last_checked_ms: None,
            apply_supported,
            updated_from,
            state: UpdateState::Idle,
        }
    }
}

/// A system release files are made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseOs {
    Windows,
    Linux,
    MacOs,
}

/// The release files a build updates from, the preferred first: edition `edition` of app `app`
/// on `os` and `arch` (`x86_64`, `aarch64`); `appimage` for a Linux build running as an AppImage,
/// which updates from AppImages only. Each edition updates only from its own files.
pub fn release_file_names(
    app: &str,
    edition: &str,
    os: ReleaseOs,
    arch: &str,
    appimage: bool,
) -> Vec<String> {
    let base = format!("{app}-{edition}");
    match os {
        ReleaseOs::Windows => vec![format!("{base}-{arch}.exe"), format!("{base}.exe")],
        ReleaseOs::Linux if appimage => vec![format!("{base}-{arch}.AppImage")],
        ReleaseOs::Linux => vec![format!("{base}-{arch}")],
        ReleaseOs::MacOs => vec![format!("{base}-{arch}.dmg"), format!("{base}-universal.dmg")],
    }
}
