//! Vanilla Minecraft: metadata, rules, libraries, assets, install and integrity, in the file
//! layout minecraft-launcher-lib and the original launcher use.

pub mod assets;
pub mod catalog;
pub mod command;
pub mod install;
pub mod integrity;
pub mod library;
pub mod manifest;
pub mod platform;
pub mod rules;
pub mod version;

use launcher_shared::Text;

use crate::net::downloader::DownloadProgress;

/// What an install is doing: a status line and, while files download, the counts.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallProgress {
    pub status: Text,
    pub download: Option<DownloadProgress>,
}

impl InstallProgress {
    pub fn status(status: Text) -> InstallProgress {
        InstallProgress { status, download: None }
    }

    pub fn download(status: Text, progress: DownloadProgress) -> InstallProgress {
        InstallProgress { status, download: Some(progress) }
    }
}

pub type InstallProgressFn<'a> = &'a (dyn Fn(InstallProgress) + Send + Sync);
