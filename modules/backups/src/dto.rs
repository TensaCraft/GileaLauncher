//! What the backups module's commands take and answer (its backend and its UI share them).

use serde::{Deserialize, Serialize};

/// A world of a build: a folder of `saves/` with a `level.dat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldDto {
    pub folder: String,
    /// Bytes, `session.lock` left out.
    pub size: u64,
    /// Unix seconds of its `level.dat`.
    pub modified: i64,
    pub backups: usize,
    /// Where its backups are.
    pub backups_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupDto {
    pub zip_name: String,
    pub kind: Kind,
    /// Unix seconds.
    pub created: i64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSettings {
    pub enabled: bool,
    pub keep: u32,
    pub dir: String,
    pub default_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArgs {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldArgs {
    pub key: String,
    pub world: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupArgs {
    pub key: String,
    pub world: String,
    pub zip_name: String,
}
