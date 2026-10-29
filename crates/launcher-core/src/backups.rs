//! The one backups folder of the launcher (the setting «Каталог бекапів») and each build's folder
//! in it: world, mod and modpack backups, all of a build's together.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::lock::path_key;
use crate::storage::config::ConfigStore;
use crate::storage::versions::Build;

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

/// The backups folder as it is when asked: the setting may change while the launcher runs.
pub type BackupsRoot = Arc<dyn Fn() -> PathBuf + Send + Sync>;

/// `root` of `config`, asked each time.
pub fn resolver(config: Arc<ConfigStore>, mc_dir: PathBuf) -> BackupsRoot {
    Arc::new(move || root(&config, &mc_dir))
}

/// One build's backups: `<backups>/<build's folder name>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildBackups(PathBuf);

impl BuildBackups {
    pub fn of(root: &Path, folder: &str) -> BuildBackups {
        BuildBackups(root.join(folder))
    }

    /// `build`'s, named as its folders are, by its version id: an old record's key is its raw name.
    pub fn of_build(root: &Path, build: &Build) -> BuildBackups {
        BuildBackups::of(root, &build.version_id)
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

    /// Where build sync kept what it overwrote, a folder per pass (`<time>-<n>`), up to 0.0.17:
    /// only leftovers the daily clean removes.
    pub fn sync(&self) -> PathBuf {
        self.0.join("sync")
    }

    /// Removes its mods' and modpack's backups: what the launcher wrote there (a mod's
    /// `<file>.backup`, an update's folder named by its time), then the folders left empty.
    /// Anything else there is the player's: the backups folder may be one they keep things in.
    pub fn remove_mods_and_modpacks(&self) -> io::Result<()> {
        let mods = self.mods();
        for entry in fs::read_dir(&mods).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_file())
                && (name.ends_with(".backup") || name.ends_with(".backup.part"))
            {
                fs::remove_file(entry.path())?;
            }
        }
        let modpacks = self.modpacks();
        for entry in fs::read_dir(&modpacks).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_dir()) && stamped(&name) {
                fs::remove_dir_all(entry.path())?;
            }
        }
        let _ = fs::remove_dir(&mods);
        let _ = fs::remove_dir(&modpacks);
        Ok(())
    }
}

/// `name` is a folder of copies named by its time (`2026-10-01_10-00-00`), with `-2`… after it
/// for a second one in the same second: a modpack update's, or a sync pass's.
pub fn stamped(name: &str) -> bool {
    let (Some(time), Some(rest)) = (name.get(..19), name.get(19..)) else { return false };
    chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d_%H-%M-%S").is_ok()
        && (rest.is_empty()
            || rest.strip_prefix('-').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

/// When the copies in folder `name` (`stamped`) were made.
pub fn stamp_time(name: &str) -> Option<std::time::SystemTime> {
    let time = chrono::NaiveDateTime::parse_from_str(name.get(..19)?, "%Y-%m-%d_%H-%M-%S").ok()?.and_utc();
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(time.timestamp()).ok()?))
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

    #[test]
    fn a_builds_mod_and_modpack_backups_go_and_the_players_files_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let build = BuildBackups::of(tmp.path(), "aero");
        std::fs::create_dir_all(build.mods()).unwrap();
        std::fs::write(build.mods().join("sodium.jar.backup"), b"old").unwrap();
        std::fs::write(build.mods().join("iris.jar.backup.part"), b"half").unwrap();
        let update = build.modpacks().join("2026-10-01_10-00-00");
        std::fs::create_dir_all(update.join("config")).unwrap();
        std::fs::write(update.join("config/a.toml"), b"copy").unwrap();
        std::fs::create_dir_all(build.modpacks().join("2026-10-01_10-00-00-2")).unwrap();
        build.remove_mods_and_modpacks().unwrap();
        assert!(!build.mods().exists() && !build.modpacks().exists());
        std::fs::create_dir_all(build.modpacks().join("My pack")).unwrap();
        std::fs::create_dir_all(build.modpacks().join("Мій пак 2 копія")).unwrap();
        std::fs::create_dir_all(build.mods()).unwrap();
        std::fs::write(build.mods().join("notes.txt"), b"mine").unwrap();
        std::fs::write(build.mods().join("x.jar.backup"), b"old").unwrap();
        build.remove_mods_and_modpacks().unwrap();
        assert!(!build.mods().join("x.jar.backup").exists());
        assert!(build.mods().join("notes.txt").is_file(), "the player's file stays");
        assert!(build.modpacks().join("My pack").is_dir(), "the player's folder stays");
        assert!(build.modpacks().join("Мій пак 2 копія").is_dir());
    }

    #[test]
    fn an_old_record_s_backups_are_named_like_its_folders() {
        let mut build = crate::storage::versions::Build::new("Aeronautics");
        build.key = "Aeronautics 1.20".into();
        build.version_id = "aeronautics_1_20".into();
        assert_eq!(
            BuildBackups::of_build(Path::new("/b"), &build).dir(),
            Path::new("/b").join("aeronautics_1_20")
        );
    }
}
