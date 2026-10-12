//! Browser-preview backend for the «Сховище» settings tab: three builds, every line holding
//! something, one build running.

use launcher_shared::{
    AppError, BuildStorage, ErrorCode, LineSize, RuleState, StorageLine, StorageRule, StorageSnapshot,
};
use serde_json::Value;

use crate::mock_builds::{arg, to_value};

const MB: u64 = 1024 * 1024;

fn lines(sizes: [u64; 7]) -> Vec<LineSize> {
    StorageLine::BUILD.iter().zip(sizes).map(|(&line, mb)| LineSize { line, bytes: mb * MB }).collect()
}

pub struct MockStorage {
    snapshot: StorageSnapshot,
}

impl MockStorage {
    pub fn new() -> MockStorage {
        let build = |key: &str, name: &str, vanilla: bool, running: bool, sizes| BuildStorage {
            key: key.into(),
            name: name.into(),
            vanilla,
            running,
            lines: lines(sizes),
        };
        MockStorage {
            snapshot: StorageSnapshot {
                builds: vec![
                    build("aeronautics", "Aeronautics", false, false, [1843, 2458, 640, 310, 12, 35, 48]),
                    build("skyblock", "Скайблок", false, true, [420, 120, 0, 64, 3, 2, 9]),
                    build("vanilna_1_20_1", "Ванільна 1.20.1", true, false, [210, 0, 0, 0, 1, 0, 4]),
                ],
                launcher: [86, 140, 22]
                    .into_iter()
                    .zip(StorageLine::LAUNCHER)
                    .map(|(mb, line)| LineSize { line, bytes: mb * MB })
                    .collect(),
                free: Some(48 * 1024 * MB + 300 * MB),
                rules: StorageRule::ALL.iter().map(|&rule| RuleState { rule, on: true }).collect(),
            },
        }
    }

    fn clear(&mut self, key: Option<&str>, line: StorageLine) -> Result<(), AppError> {
        let lines = match key {
            Some(key) => {
                let build = self.snapshot.builds.iter_mut().find(|b| b.key == key).ok_or_else(|| {
                    AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", key)
                })?;
                if build.running {
                    return Err(AppError::new(ErrorCode::InstanceBusy, "mock")
                        .with_param("version", &build.name)
                        .with_param("kind", "launch"));
                }
                &mut build.lines
            }
            None => &mut self.snapshot.launcher,
        };
        lines.iter_mut().filter(|l| l.line == line).for_each(|l| l.bytes = 0);
        Ok(())
    }

    pub fn handle(&mut self, cmd: &str, args: &Value) -> Option<Result<Value, AppError>> {
        let line = || serde_json::from_value::<StorageLine>(args["line"].clone());
        let result = match cmd {
            "storage_snapshot" => to_value(&self.snapshot),
            "storage_clear" => {
                let key = Some(arg(args, "key")).filter(|k| !k.is_empty());
                match line() {
                    Ok(line) => self.clear(key, line).and_then(|()| to_value(&self.snapshot)),
                    Err(e) => Err(AppError::internal(e.to_string())),
                }
            }
            "storage_clear_unneeded" => {
                for build in self.snapshot.builds.iter_mut().filter(|b| !b.running) {
                    build.lines.iter_mut().filter(|l| l.line.unneeded()).for_each(|l| l.bytes = 0);
                }
                self.snapshot.launcher.iter_mut().for_each(|l| l.bytes = 0);
                to_value(&self.snapshot)
            }
            "storage_set_rule" => {
                let rule = serde_json::from_value::<StorageRule>(args["rule"].clone());
                let on = args["on"].as_bool().unwrap_or(true);
                if let Ok(rule) = rule {
                    self.snapshot.rules.iter_mut().filter(|r| r.rule == rule).for_each(|r| r.on = on);
                }
                to_value(&self.snapshot)
            }
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{ErrorCode, StorageLine, StorageSnapshot};
    use serde_json::json;

    use super::*;

    fn snapshot(value: Result<serde_json::Value, launcher_shared::AppError>) -> StorageSnapshot {
        serde_json::from_value(value.unwrap()).unwrap()
    }

    fn bytes(snapshot: &StorageSnapshot, key: &str, line: StorageLine) -> u64 {
        let build = snapshot.builds.iter().find(|b| b.key == key).unwrap();
        build.lines.iter().find(|l| l.line == line).map_or(0, |l| l.bytes)
    }

    #[test]
    fn the_mock_clears_a_line_and_refuses_a_running_build() {
        let mut mock = MockStorage::new();
        let first = snapshot(mock.handle("storage_snapshot", &json!({"refresh": false})).unwrap());
        assert!(first.builds.iter().any(|b| b.running) && first.builds.iter().any(|b| b.vanilla));
        assert!(bytes(&first, "aeronautics", StorageLine::ModBackups) > 2_000_000_000);
        let cleared = mock.handle("storage_clear", &json!({"key": "aeronautics", "line": "mod_backups"}));
        assert_eq!(bytes(&snapshot(cleared.unwrap()), "aeronautics", StorageLine::ModBackups), 0);
        let running = first.builds.iter().find(|b| b.running).unwrap().key.clone();
        let refused = mock.handle("storage_clear", &json!({"key": running, "line": "game_logs"})).unwrap();
        assert_eq!(refused.unwrap_err().code, ErrorCode::InstanceBusy);
        let rules = mock.handle("storage_set_rule", &json!({"rule": "thumbnails", "on": false}));
        assert!(snapshot(rules.unwrap()).rules.iter().any(|r| !r.on));
        assert!(mock.handle("components_list", &json!({})).is_none(), "not its command");
    }

    #[test]
    fn clearing_everything_unneeded_keeps_world_backups() {
        let mut mock = MockStorage::new();
        let after = snapshot(mock.handle("storage_clear_unneeded", &json!({})).unwrap());
        assert!(bytes(&after, "aeronautics", StorageLine::WorldBackups) > 0);
        assert_eq!(bytes(&after, "aeronautics", StorageLine::GameLogs), 0);
        let running = after.builds.iter().find(|b| b.running).unwrap();
        assert!(running.total() > running.lines[0].bytes, "a running build keeps its files");
        assert!(after.launcher.iter().all(|l| l.bytes == 0));
    }
}
