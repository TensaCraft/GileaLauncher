use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable, user-facing error categories. The UI translates them via `i18n_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    Io,
    InvalidInput,
    InvalidDirectoryPath,
    DirectoryCreateFailed,
    StorageUnavailable,
    NotFound,
    Busy,
    Cancelled,
    Unsupported,
    Network,
    RateLimited,
    IntegrityMismatch,
    NoUpdateAsset,
    AuthTimeout,
    AuthDenied,
    AuthFailed,
    MinecraftServicesUnavailable,
    XboxAccountMissing,
    XboxChildAccount,
    XboxUnavailable,
    MinecraftNotOwned,
    ReauthRequired,
    CredentialStorageUnavailable,
    ProfileSaveFailed,
    ProfileExists,
    ProfileNameInvalid,
    NoProfile,
    InstanceBusy,
    SharedBusy,
    NotEnoughSpace,
    VersionNotFound,
    VersionExists,
    VersionNameEmpty,
    DownloadFailed,
    VersionFilesRemain,
    JavaRuntimeFailed,
    VersionRunning,
    LaunchThrottled,
    LaunchFailed,
    InvalidJavaExecutable,
    BuildRunning,
    ShortcutFailed,
    LoaderInstallFailed,
    GameRunning,
    ContentConflict,
    NoCompatibleVersion,
    NoFileFound,
    BackupFailed,
    BackupNotFound,
    FileInUse,
    ProviderKeyRejected,
    /// Files the provider may not hand to other apps: the user downloads them by hand first
    /// (`provider::held_files`).
    ProviderFilesHeld,
    /// A game runs and less memory is free than the next one asks for.
    LowMemory,
    /// A name a file cannot have (path characters, a device name, too long).
    FileNameInvalid,
    /// Another file already has the name.
    FileNameTaken,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 56] = [
        ErrorCode::Internal,
        ErrorCode::Io,
        ErrorCode::InvalidInput,
        ErrorCode::InvalidDirectoryPath,
        ErrorCode::DirectoryCreateFailed,
        ErrorCode::StorageUnavailable,
        ErrorCode::NotFound,
        ErrorCode::Busy,
        ErrorCode::Cancelled,
        ErrorCode::Unsupported,
        ErrorCode::Network,
        ErrorCode::RateLimited,
        ErrorCode::IntegrityMismatch,
        ErrorCode::NoUpdateAsset,
        ErrorCode::AuthTimeout,
        ErrorCode::AuthDenied,
        ErrorCode::AuthFailed,
        ErrorCode::MinecraftServicesUnavailable,
        ErrorCode::XboxAccountMissing,
        ErrorCode::XboxChildAccount,
        ErrorCode::XboxUnavailable,
        ErrorCode::MinecraftNotOwned,
        ErrorCode::ReauthRequired,
        ErrorCode::CredentialStorageUnavailable,
        ErrorCode::ProfileSaveFailed,
        ErrorCode::ProfileExists,
        ErrorCode::ProfileNameInvalid,
        ErrorCode::NoProfile,
        ErrorCode::InstanceBusy,
        ErrorCode::SharedBusy,
        ErrorCode::NotEnoughSpace,
        ErrorCode::VersionNotFound,
        ErrorCode::VersionExists,
        ErrorCode::VersionNameEmpty,
        ErrorCode::DownloadFailed,
        ErrorCode::VersionFilesRemain,
        ErrorCode::JavaRuntimeFailed,
        ErrorCode::VersionRunning,
        ErrorCode::LaunchThrottled,
        ErrorCode::LaunchFailed,
        ErrorCode::InvalidJavaExecutable,
        ErrorCode::BuildRunning,
        ErrorCode::ShortcutFailed,
        ErrorCode::LoaderInstallFailed,
        ErrorCode::GameRunning,
        ErrorCode::ContentConflict,
        ErrorCode::NoCompatibleVersion,
        ErrorCode::NoFileFound,
        ErrorCode::BackupFailed,
        ErrorCode::BackupNotFound,
        ErrorCode::FileInUse,
        ErrorCode::ProviderKeyRejected,
        ErrorCode::ProviderFilesHeld,
        ErrorCode::LowMemory,
        ErrorCode::FileNameInvalid,
        ErrorCode::FileNameTaken,
    ];

    pub fn i18n_key(self) -> &'static str {
        match self {
            ErrorCode::Internal => "unknown_error",
            ErrorCode::Io => "error_io",
            ErrorCode::InvalidInput => "error_invalid_input",
            ErrorCode::InvalidDirectoryPath => "invalid_directory_path",
            ErrorCode::DirectoryCreateFailed => "directory_create_failed",
            ErrorCode::StorageUnavailable => "setup_wizard_storage_issue",
            ErrorCode::NotFound => "error_not_found",
            ErrorCode::Busy => "installation_already_running",
            ErrorCode::Cancelled => "error_cancelled",
            ErrorCode::Unsupported => "error_unsupported",
            ErrorCode::Network => "error_network",
            ErrorCode::RateLimited => "update_rate_limited",
            ErrorCode::IntegrityMismatch => "update_hash_mismatch",
            ErrorCode::NoUpdateAsset => "update_no_asset",
            ErrorCode::AuthTimeout => "microsoft_auth_timeout",
            ErrorCode::AuthDenied => "microsoft_auth_denied",
            ErrorCode::AuthFailed => "microsoft_auth_failed",
            ErrorCode::MinecraftServicesUnavailable => "minecraft_services_auth_unavailable",
            ErrorCode::XboxAccountMissing => "xbox_account_missing",
            ErrorCode::XboxChildAccount => "xbox_child_account",
            ErrorCode::XboxUnavailable => "xbox_unavailable",
            ErrorCode::MinecraftNotOwned => "minecraft_not_owned",
            ErrorCode::ReauthRequired => "profile_reauth_required",
            ErrorCode::CredentialStorageUnavailable => "credential_encryption_unavailable",
            ErrorCode::ProfileSaveFailed => "profile_save_failed",
            ErrorCode::ProfileExists => "profile_exists",
            ErrorCode::ProfileNameInvalid => "profile_name_invalid",
            ErrorCode::NoProfile => "no_default_profile",
            ErrorCode::InstanceBusy => "instance_operation_busy",
            ErrorCode::SharedBusy => "shared_minecraft_operation_busy",
            ErrorCode::NotEnoughSpace => "not_enough_space",
            ErrorCode::VersionNotFound => "version_not_found",
            ErrorCode::VersionExists => "version_exists",
            ErrorCode::VersionNameEmpty => "empty_version_name",
            ErrorCode::DownloadFailed => "download_error",
            ErrorCode::VersionFilesRemain => "version_delete_files_remain",
            ErrorCode::JavaRuntimeFailed => "java_runtime_install_failed",
            ErrorCode::VersionRunning => "version_already_running",
            ErrorCode::LaunchThrottled => "version_launch_throttled",
            ErrorCode::LaunchFailed => "version_integrity_check_failed",
            ErrorCode::InvalidJavaExecutable => "custom_java_invalid",
            ErrorCode::BuildRunning => "version_delete_close_game_first",
            ErrorCode::ShortcutFailed => "desktop_shortcut_failed",
            ErrorCode::LoaderInstallFailed => "loader_install_failed",
            ErrorCode::GameRunning => "instance_game_running",
            ErrorCode::ContentConflict => "content_file_exists",
            ErrorCode::NoCompatibleVersion => "no_compatible_version",
            ErrorCode::NoFileFound => "no_file_found",
            ErrorCode::BackupFailed => "backup_failed",
            ErrorCode::BackupNotFound => "backup_not_found",
            ErrorCode::FileInUse => "error_file_in_use",
            ErrorCode::ProviderKeyRejected => "provider_key_rejected",
            ErrorCode::ProviderFilesHeld => "provider_files_held",
            ErrorCode::LowMemory => "version_low_memory",
            ErrorCode::FileNameInvalid => "file_name_invalid",
            ErrorCode::FileNameTaken => "file_name_taken",
        }
    }

    /// `Io`, or `FileInUse` for a file another program holds open (Windows refuses to move or
    /// delete a file a running game has loaded).
    pub fn of_io(e: &std::io::Error) -> ErrorCode {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)) {
            ErrorCode::FileInUse
        } else {
            ErrorCode::Io
        }
    }
}

/// Error crossing the IPC boundary. `detail` is technical text for logs, never shown raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {detail}")]
pub struct AppError {
    pub code: ErrorCode,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub detail: String,
}

impl AppError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self { code, params: BTreeMap::new(), detail: detail.into() }
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, detail)
    }

    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }
}

pub type AppResult<T> = Result<T, AppError>;
