//! Browser-preview backend for build sync: two groups over the mock builds (one by the latest
//! edit, one from a base build), a tree with sizes and locks, passes and history.

use launcher_shared::{AppError, AppResult, ErrorCode};
use module_sync::dto::{
    GroupState, GroupView, HistoryEntry, Lock, PassReport, Reason, SyncGroup, SyncMode, TreeEntry,
};
use serde_json::Value;

use crate::mock_builds::to_value;

const MB: u64 = 1024 * 1024;
/// A moment of the mock's: 12.10.2026 10:13 UTC.
const NOW_MS: u64 = 1_791_800_000_000;

fn group(id: &str, name: &str, paths: &[&str], mode: SyncMode) -> GroupView {
    GroupView {
        group: SyncGroup {
            id: id.into(),
            name: name.into(),
            builds: vec!["aeronautics".into(), "vanilna_1_20_1".into()],
            paths: paths.iter().map(|p| p.to_string()).collect(),
            mode,
            start: "aeronautics".into(),
        },
        last_ms: Some(NOW_MS - 3_600_000),
        state: GroupState::Synced,
        note: None,
    }
}

fn entry(path: &str, dir: bool, megabytes: u64, present: u32) -> TreeEntry {
    TreeEntry {
        path: path.into(),
        name: path.rsplit('/').next().unwrap_or(path).into(),
        dir,
        size: megabytes * MB + 4096,
        present,
        total: 2,
        lock: None,
        managed_in: Vec::new(),
    }
}

pub struct MockSync {
    groups: Vec<GroupView>,
    history: Vec<HistoryEntry>,
    made: u32,
}

impl MockSync {
    pub fn new() -> MockSync {
        let history = vec![
            HistoryEntry {
                id: "vanilna_1_20_1/2026-10-12_09-13-20-1/options.txt".into(),
                build: "vanilna_1_20_1".into(),
                group: "settings".into(),
                path: "options.txt".into(),
                from: Some("aeronautics".into()),
                reason: Reason::Changed,
                at_ms: NOW_MS - 3_600_000,
                size: 6 * 1024,
            },
            HistoryEntry {
                id: "aeronautics/2026-10-11_20-02-11-1/options.txt".into(),
                build: "aeronautics".into(),
                group: "settings".into(),
                path: "options.txt".into(),
                from: Some("vanilna_1_20_1".into()),
                reason: Reason::Conflict,
                at_ms: NOW_MS - 50_000_000,
                size: 6 * 1024,
            },
            HistoryEntry {
                id: "vanilna_1_20_1/2026-10-11_20-02-11-1/xaero/minimap.txt".into(),
                build: "vanilna_1_20_1".into(),
                group: "minimap".into(),
                path: "xaero/minimap.txt".into(),
                from: Some("aeronautics".into()),
                reason: Reason::Base,
                at_ms: NOW_MS - 50_000_000,
                size: 2 * 1024,
            },
        ];
        MockSync {
            groups: vec![
                group(
                    "settings",
                    "Налаштування й ресурспаки",
                    &["options.txt", "resourcepacks"],
                    SyncMode::Latest,
                ),
                group("minimap", "Мінікарта", &["xaero"], SyncMode::Base),
            ],
            history,
            made: 0,
        }
    }

    fn tree(&self, dir: &str, editing: Option<&str>) -> Vec<TreeEntry> {
        let taken = |path: &str| {
            self.groups
                .iter()
                .filter(|g| Some(g.group.id.as_str()) != editing)
                .find(|g| g.group.paths.iter().any(|p| p == path))
                .map(|g| Lock::Taken { group: g.group.name.clone() })
        };
        let mut rows = match dir {
            "" => vec![
                entry("config", true, 3, 2),
                entry("mods", true, 412, 1),
                entry("resourcepacks", true, 1350, 2),
                entry("shaderpacks", true, 96, 1),
                entry("xaero", true, 1, 2),
                entry("options.txt", false, 0, 2),
                entry("servers.dat", false, 0, 2),
            ],
            "config" => {
                let mut server = entry("config/server", true, 1, 1);
                server.lock = Some(Lock::Managed);
                server.managed_in = vec!["Ванільна 1.20.1".into()];
                vec![
                    entry("config/jei", true, 0, 2),
                    server,
                    entry("config/sodium-options.json", false, 0, 1),
                ]
            }
            "resourcepacks" => vec![
                entry("resourcepacks/Faithful 32x.zip", false, 1300, 2),
                entry("resourcepacks/UI.zip", false, 50, 1),
            ],
            _ => Vec::new(),
        };
        for row in &mut rows {
            if row.lock.is_none() {
                row.lock = taken(&row.path);
            }
        }
        rows
    }

    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        let id = args["id"].as_str().unwrap_or_default().to_string();
        match command {
            "groups" => to_value(&self.groups),
            "save" => {
                let mut group: SyncGroup = serde_json::from_value(args["group"].clone())
                    .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
                if group.name.trim().is_empty() || group.builds.len() < 2 || group.paths.is_empty() {
                    return Err(AppError::new(ErrorCode::InvalidInput, "mock: an incomplete group"));
                }
                if group.id.is_empty() {
                    self.made += 1;
                    group.id = format!("new{}", self.made);
                }
                let view =
                    GroupView { group: group.clone(), last_ms: None, state: GroupState::New, note: None };
                match self.groups.iter_mut().find(|g| g.group.id == group.id) {
                    Some(old) => old.group = group,
                    None => self.groups.push(view.clone()),
                }
                to_value(view)
            }
            "delete" => {
                self.groups.retain(|g| g.group.id != id);
                Ok(Value::Null)
            }
            "sync" | "restore" => {
                let group = if command == "restore" {
                    args["group"].as_str().unwrap_or_default().to_string()
                } else {
                    id
                };
                if let Some(view) = self.groups.iter_mut().find(|g| g.group.id == group) {
                    view.state = GroupState::Synced;
                    view.last_ms = Some(NOW_MS);
                }
                to_value(PassReport { written: 2, removed: 0, conflicts: Vec::new(), waiting: false })
            }
            "sync_build" => {
                let key = args["key"].as_str().unwrap_or_default();
                let names: Vec<(String, Option<String>)> = self
                    .groups
                    .iter()
                    .filter(|g| g.group.builds.iter().any(|b| b == key))
                    .map(|g| (g.group.name.clone(), None))
                    .collect();
                to_value(names)
            }
            "tree" => to_value(self.tree(args["dir"].as_str().unwrap_or_default(), args["editing"].as_str())),
            "history" => to_value(self.history.iter().filter(|h| h.group == id).cloned().collect::<Vec<_>>()),
            other => Err(AppError::internal(format!("mock: no sync command {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use module_sync::dto::{GroupState, GroupView, HistoryEntry, PassReport, TreeEntry};
    use serde_json::json;

    use super::*;

    #[test]
    fn groups_sync_and_history() {
        let mut mock = MockSync::new();
        let groups: Vec<GroupView> =
            serde_json::from_value(mock.handle("groups", &json!(null)).unwrap()).unwrap();
        assert_eq!(groups.len(), 2);
        let id = groups[0].group.id.clone();
        let report: PassReport =
            serde_json::from_value(mock.handle("sync", &json!({"id": id})).unwrap()).unwrap();
        assert!(report.written > 0 && !report.waiting);
        let groups: Vec<GroupView> =
            serde_json::from_value(mock.handle("groups", &json!(null)).unwrap()).unwrap();
        assert_eq!(groups[0].state, GroupState::Synced);
        let history: Vec<HistoryEntry> =
            serde_json::from_value(mock.handle("history", &json!({"id": id})).unwrap()).unwrap();
        assert!(!history.is_empty());
        let args = json!({"builds": ["aeronautics", "vanilna_1_20_1"], "dir": "", "editing": null});
        let tree: Vec<TreeEntry> = serde_json::from_value(mock.handle("tree", &args).unwrap()).unwrap();
        assert!(tree.iter().any(|e| e.lock.is_some()) && tree.iter().any(|e| e.present < e.total));
        let group = json!({"group": {"id": "", "name": "Нова", "builds": ["aeronautics", "vanilna_1_20_1"],
                                     "paths": ["servers.dat"], "mode": "latest", "start": "aeronautics"}});
        mock.handle("save", &group).unwrap();
        assert_eq!(
            serde_json::from_value::<Vec<GroupView>>(mock.handle("groups", &json!(null)).unwrap())
                .unwrap()
                .len(),
            3
        );
        assert!(
            mock.handle(
                "save",
                &json!({"group": {"id": "", "name": "", "builds": [], "paths": [], "start": ""}})
            )
            .is_err()
        );
    }
}
