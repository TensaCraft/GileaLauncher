//! What the launcher's own files take on disk: the lines the «Сховище» settings tab shows, per
//! build and for the launcher, and the rules that clean them on their own.

use serde::{Deserialize, Serialize};

/// One kind of the launcher's files: a build's (`BUILD`) or the launcher's own (`LAUNCHER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageLine {
    WorldBackups,
    ModBackups,
    ModpackBackups,
    ServiceFolder,
    CrashReports,
    GameLogs,
    Thumbnails,
    UpdateDownloads,
    TempFiles,
}

impl StorageLine {
    /// A build's lines, in the order the tab shows them.
    pub const BUILD: [StorageLine; 6] = [
        StorageLine::WorldBackups,
        StorageLine::ModBackups,
        StorageLine::ModpackBackups,
        StorageLine::ServiceFolder,
        StorageLine::CrashReports,
        StorageLine::GameLogs,
    ];
    /// The launcher's own lines.
    pub const LAUNCHER: [StorageLine; 3] =
        [StorageLine::Thumbnails, StorageLine::UpdateDownloads, StorageLine::TempFiles];

    /// «Очистити непотрібне» clears it: every line but the world backups, which the player keeps.
    pub fn unneeded(self) -> bool {
        self != StorageLine::WorldBackups
    }

    /// Only a build with a mod loader has it (a vanilla build shows nothing about mods).
    pub fn mods_only(self) -> bool {
        matches!(self, StorageLine::ModBackups | StorageLine::ModpackBackups)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            StorageLine::WorldBackups => "storage_line_world_backups",
            StorageLine::ModBackups => "storage_line_mod_backups",
            StorageLine::ModpackBackups => "storage_line_modpack_backups",
            StorageLine::ServiceFolder => "storage_line_service_folder",
            StorageLine::CrashReports => "storage_line_crash_reports",
            StorageLine::GameLogs => "storage_line_game_logs",
            StorageLine::Thumbnails => "storage_line_thumbnails",
            StorageLine::UpdateDownloads => "storage_line_update_downloads",
            StorageLine::TempFiles => "storage_line_temp_files",
        }
    }
}

/// A rule the launcher cleans by on its own, once a day; each one a switch in the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageRule {
    ModBackups,
    ModpackBackups,
    JavaDumps,
    CrashReports,
    ServiceFolder,
    Thumbnails,
}

impl StorageRule {
    pub const ALL: [StorageRule; 6] = [
        StorageRule::ModBackups,
        StorageRule::ModpackBackups,
        StorageRule::JavaDumps,
        StorageRule::CrashReports,
        StorageRule::ServiceFolder,
        StorageRule::Thumbnails,
    ];

    /// Its switch in the config (`"no"` turns it off; anything else, or nothing, is on).
    pub fn config_key(self) -> &'static str {
        match self {
            StorageRule::ModBackups => "storage_rule_mod_backups",
            StorageRule::ModpackBackups => "storage_rule_modpack_backups",
            StorageRule::JavaDumps => "storage_rule_java_dumps",
            StorageRule::CrashReports => "storage_rule_crash_reports",
            StorageRule::ServiceFolder => "storage_rule_service_folder",
            StorageRule::Thumbnails => "storage_rule_thumbnails",
        }
    }

    pub fn label_key(self) -> &'static str {
        self.config_key()
    }

    pub fn desc_key(self) -> &'static str {
        match self {
            StorageRule::ModBackups => "storage_rule_mod_backups_desc",
            StorageRule::ModpackBackups => "storage_rule_modpack_backups_desc",
            StorageRule::JavaDumps => "storage_rule_java_dumps_desc",
            StorageRule::CrashReports => "storage_rule_crash_reports_desc",
            StorageRule::ServiceFolder => "storage_rule_service_folder_desc",
            StorageRule::Thumbnails => "storage_rule_thumbnails_desc",
        }
    }
}

/// What one line takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSize {
    pub line: StorageLine,
    pub bytes: u64,
}

/// A build's lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildStorage {
    pub key: String,
    pub name: String,
    /// It has no mod loader.
    pub vanilla: bool,
    /// Its game runs: it shows its sizes, but nothing of it is cleared.
    pub running: bool,
    pub lines: Vec<LineSize>,
}

impl BuildStorage {
    pub fn total(&self) -> u64 {
        self.lines.iter().map(|l| l.bytes).sum()
    }
}

/// A rule's switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleState {
    pub rule: StorageRule,
    pub on: bool,
}

/// What the tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSnapshot {
    pub builds: Vec<BuildStorage>,
    pub launcher: Vec<LineSize>,
    /// The free space on the Minecraft folder's disk, when it could be read.
    pub free: Option<u64>,
    pub rules: Vec<RuleState>,
}

impl StorageSnapshot {
    /// Every line of every build and of the launcher.
    pub fn total(&self) -> u64 {
        self.builds.iter().map(BuildStorage::total).sum::<u64>()
            + self.launcher.iter().map(|l| l.bytes).sum::<u64>()
    }

    /// What «Очистити непотрібне» frees, line by line: the `unneeded` lines that hold something,
    /// a build line summed over the builds whose game does not run (those it leaves).
    pub fn unneeded(&self) -> Vec<LineSize> {
        StorageLine::BUILD
            .iter()
            .chain(&StorageLine::LAUNCHER)
            .filter(|line| line.unneeded())
            .map(|&line| {
                let builds = self.builds.iter().filter(|b| !b.running);
                let lines = builds.flat_map(|b| &b.lines).chain(&self.launcher);
                LineSize { line, bytes: lines.filter(|l| l.line == line).map(|l| l.bytes).sum() }
            })
            .filter(|l| l.bytes > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(line: StorageLine, bytes: u64) -> LineSize {
        LineSize { line, bytes }
    }

    fn build(key: &str, lines: Vec<LineSize>) -> BuildStorage {
        BuildStorage { key: key.into(), name: key.into(), vanilla: false, running: false, lines }
    }

    #[test]
    fn world_backups_are_never_unneeded() {
        assert!(!StorageLine::WorldBackups.unneeded());
        for line in StorageLine::BUILD.iter().chain(&StorageLine::LAUNCHER) {
            assert_eq!(line.unneeded(), *line != StorageLine::WorldBackups, "{line:?}");
        }
    }

    #[test]
    fn the_unneeded_sum_skips_world_backups_and_empty_lines() {
        let snapshot = StorageSnapshot {
            builds: vec![
                build("a", vec![line(StorageLine::WorldBackups, 10), line(StorageLine::ModBackups, 5)]),
                build("b", vec![line(StorageLine::ModBackups, 7), line(StorageLine::GameLogs, 0)]),
            ],
            launcher: vec![line(StorageLine::Thumbnails, 3), line(StorageLine::TempFiles, 0)],
            free: Some(100),
            rules: Vec::new(),
        };
        assert_eq!(
            snapshot.unneeded(),
            vec![line(StorageLine::ModBackups, 12), line(StorageLine::Thumbnails, 3)]
        );
        assert_eq!(snapshot.total(), 25);
        assert_eq!(snapshot.builds[0].total(), 15);
    }

    #[test]
    fn a_running_build_adds_nothing_to_what_is_cleared() {
        let mut running = build("b", vec![line(StorageLine::ModBackups, 7)]);
        running.running = true;
        let snapshot = StorageSnapshot {
            builds: vec![build("a", vec![line(StorageLine::ModBackups, 5)]), running],
            launcher: Vec::new(),
            free: None,
            rules: Vec::new(),
        };
        assert_eq!(snapshot.unneeded(), vec![line(StorageLine::ModBackups, 5)]);
        assert_eq!(snapshot.total(), 12, "it still counts in the total");
    }

    #[test]
    fn keys_follow_the_names() {
        assert_eq!(StorageRule::JavaDumps.config_key(), "storage_rule_java_dumps");
        assert_eq!(StorageRule::JavaDumps.label_key(), "storage_rule_java_dumps");
        assert_eq!(StorageRule::JavaDumps.desc_key(), "storage_rule_java_dumps_desc");
        assert_eq!(StorageLine::GameLogs.label_key(), "storage_line_game_logs");
        assert!(StorageLine::ModpackBackups.mods_only() && !StorageLine::CrashReports.mods_only());
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let snapshot = StorageSnapshot {
            builds: vec![build("a", vec![line(StorageLine::ServiceFolder, 1)])],
            launcher: vec![line(StorageLine::UpdateDownloads, 2)],
            free: None,
            rules: vec![RuleState { rule: StorageRule::Thumbnails, on: false }],
        };
        let text = serde_json::to_string(&snapshot).unwrap();
        assert!(text.contains("\"service_folder\"") && text.contains("\"thumbnails\""), "{text}");
        assert_eq!(serde_json::from_str::<StorageSnapshot>(&text).unwrap(), snapshot);
    }
}
