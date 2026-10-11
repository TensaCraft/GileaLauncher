//! The module's settings in the launcher's config: backups before a launch on or
//! off, how many automatic ones a world keeps, and where they go.

use std::path::{Path, PathBuf};

use launcher_core::backups;
use launcher_core::storage::config::ConfigStore;

pub const ENABLED: &str = "world_backups_enabled";
pub const KEEP: &str = "world_backups_keep_count";
pub const DIR: &str = backups::DIR_KEY;
pub const DEFAULT_KEEP: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub keep: u32,
    pub dir: PathBuf,
    pub default_dir: PathBuf,
}

/// The backups folder when the config names none (every build's backups, worlds among them).
pub fn default_dir(mc_dir: &Path) -> PathBuf {
    backups::default_root(mc_dir)
}

pub fn read(config: &ConfigStore, mc_dir: &Path) -> Settings {
    let enabled = config.get_str(ENABLED).is_some_and(|v| v.eq_ignore_ascii_case("yes"));
    let keep = config
        .get(KEEP)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_KEEP);
    Settings { enabled, keep, dir: backups::root(config, mc_dir), default_dir: default_dir(mc_dir) }
}
