//! The one backups folder of the launcher (the setting «Каталог бекапів») and each build's folder
//! in it: world, mod and modpack backups, all of a build's together.

use std::path::{Path, PathBuf};

use crate::lock::path_key;
use crate::storage::config::ConfigStore;

/// The setting naming the backups folder (its name is older than the folder holding more than worlds).
pub const DIR_KEY: &str = "world_backups_dir";
/// The setting saying which layout the backups folder has: 2 once world backups sit in their
/// build's `worlds/` (`storage::layout`).
pub const LAYOUT_KEY: &str = "backups_layout";

/// `<minecraft>/backups`.
pub fn default_root(mc_dir: &Path) -> PathBuf {
    mc_dir.join("backups")
}

/// Where world backups went before every backup had one folder: `<minecraft>/backups/worlds`.
pub fn old_default(mc_dir: &Path) -> PathBuf {
    default_root(mc_dir).join("worlds")
}

/// The backups folder: the setting, unless it is blank or names the old default (the wizard
/// saved that one), else `default_root`.
pub fn root(config: &ConfigStore, mc_dir: &Path) -> PathBuf {
    config
        .get_str(DIR_KEY)
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .filter(|dir| path_key(dir) != path_key(&old_default(mc_dir)))
        .unwrap_or_else(|| default_root(mc_dir))
}

/// One build's backups: `<backups>/<build key>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildBackups(PathBuf);

impl BuildBackups {
    pub fn of(root: &Path, key: &str) -> BuildBackups {
        BuildBackups(root.join(key))
    }

    pub fn dir(&self) -> PathBuf {
        self.0.clone()
    }

    /// Its world backups, a folder per world.
    pub fn worlds(&self) -> PathBuf {
        self.0.join("worlds")
    }

    /// Its mods as they were before an update replaced them.
    pub fn mods(&self) -> PathBuf {
        self.0.join("mods")
    }

    /// Its modpack's files as they were before a pack update replaced them, a folder per update.
    pub fn modpacks(&self) -> PathBuf {
        self.0.join("modpacks")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::storage::config::ConfigStore;

    #[test]
    fn the_root_is_the_setting_unless_blank_or_the_old_default() {
        let tmp = tempfile::tempdir().unwrap();
        let mc = tmp.path().join("minecraft");
        let config = ConfigStore::open(tmp.path().join("config.json"));
        assert_eq!(root(&config, &mc), mc.join("backups"), "unset");
        config.set(DIR_KEY, json!("   ")).unwrap();
        assert_eq!(root(&config, &mc), mc.join("backups"), "blank");
        config.set(DIR_KEY, json!(mc.join("backups").join("worlds").to_string_lossy())).unwrap();
        assert_eq!(root(&config, &mc), mc.join("backups"), "the old default saved by the wizard");
        let custom = tmp.path().join("Мої бекапи");
        config.set(DIR_KEY, json!(custom.to_string_lossy())).unwrap();
        assert_eq!(root(&config, &mc), custom);
        assert_eq!(old_default(&mc), mc.join("backups").join("worlds"));
        assert_eq!(default_root(&mc), mc.join("backups"));
    }

    #[test]
    fn a_builds_backups_are_one_folder() {
        let build = BuildBackups::of(Path::new("/b"), "aeronautics");
        assert_eq!(build.dir(), Path::new("/b").join("aeronautics"));
        assert_eq!(build.worlds(), Path::new("/b").join("aeronautics").join("worlds"));
        assert_eq!(build.mods(), Path::new("/b").join("aeronautics").join("mods"));
        assert_eq!(build.modpacks(), Path::new("/b").join("aeronautics").join("modpacks"));
    }
}
