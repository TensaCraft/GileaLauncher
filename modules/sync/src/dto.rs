//! What the sync module's backend and interface say to each other.

use launcher_shared::Text;
use serde::{Deserialize, Serialize};

/// How a group decides which version of a file every build gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// «За останніми змінами»: the edit made last wins, in whichever build.
    #[default]
    Latest,
    /// «З базової збірки»: the starting build is the base, and the others always take its files.
    Base,
}

/// Files and folders kept the same across builds.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncGroup {
    /// Empty for a group not saved yet.
    pub id: String,
    pub name: String,
    /// Build keys, two or more.
    pub builds: Vec<String>,
    /// Paths relative to a build's folder, `/`-separated; a folder means everything in it.
    pub paths: Vec<String>,
    #[serde(default)]
    pub mode: SyncMode,
    /// The build a first sync takes every path from; the base in mode `Base`.
    pub start: String,
}

/// Where a group stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// Never synced yet.
    #[default]
    New,
    Synced,
    /// A build it needs runs (or it has fewer than two builds that can take part).
    Waiting,
    Failed,
    /// Left with fewer than two builds that exist.
    Idle,
}

/// A group as the groups page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupView {
    pub group: SyncGroup,
    pub last_ms: Option<u64>,
    pub state: GroupState,
    pub note: Option<Text>,
}

/// Why a tree row cannot be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lock {
    /// A module keeps it in line in every chosen build that has it («цим керує сервер»).
    Managed,
    /// Another group holds it for a chosen build.
    Taken { group: String },
}

/// A row of the editor's tree: a file or folder in the union of the chosen builds' folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub name: String,
    pub dir: bool,
    /// The largest it is in any chosen build.
    pub size: u64,
    /// In how many of the chosen builds it is.
    pub present: u32,
    pub total: u32,
    pub lock: Option<Lock>,
    /// The chosen builds (names) where a module keeps it in line: never touched there.
    #[serde(default)]
    pub managed_in: Vec<String>,
}

/// Why a version went to history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Changed,
    Removed,
    Conflict,
    Base,
    Restored,
}

/// A version a pass overwrote or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// `<build>/<pass folder>/<path>`: what restore names.
    pub id: String,
    pub build: String,
    pub group: String,
    pub path: String,
    /// The build the new version came from (none for a removal).
    pub from: Option<String>,
    pub reason: Reason,
    pub at_ms: u64,
    pub size: u64,
}

/// What a pass did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PassReport {
    pub written: u32,
    pub removed: u32,
    /// The paths changed in two builds since the last pass.
    pub conflicts: Vec<String>,
    /// It waited for a build it needs (running, or the base missing).
    pub waiting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupArgs {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveArgs {
    pub group: SyncGroup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeArgs {
    pub builds: Vec<String>,
    /// The folder whose rows are asked for (`""`: the builds' roots).
    pub dir: String,
    /// The group being edited: its own paths are not taken.
    pub editing: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArgs {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreArgs {
    pub group: String,
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_round_trips() {
        let group = SyncGroup {
            id: "g1".into(),
            name: "Мінікарта".into(),
            builds: vec!["aero".into(), "lite".into()],
            paths: vec!["options.txt".into(), "xaero".into()],
            mode: SyncMode::Base,
            start: "aero".into(),
        };
        let text = serde_json::to_string(&group).unwrap();
        assert!(text.contains("\"mode\":\"base\""), "{text}");
        assert_eq!(serde_json::from_str::<SyncGroup>(&text).unwrap(), group);
        let locked = TreeEntry {
            path: "config/server".into(),
            name: "server".into(),
            dir: true,
            size: 10,
            present: 1,
            total: 2,
            lock: Some(Lock::Taken { group: "Мінікарта".into() }),
            managed_in: Vec::new(),
        };
        let text = serde_json::to_string(&locked).unwrap();
        assert!(text.contains("\"taken\""), "{text}");
        assert_eq!(serde_json::from_str::<TreeEntry>(&text).unwrap(), locked);
        let view = GroupView { group, last_ms: None, state: GroupState::New, note: None };
        assert_eq!(serde_json::from_value::<GroupView>(serde_json::to_value(&view).unwrap()).unwrap(), view);
    }
}
