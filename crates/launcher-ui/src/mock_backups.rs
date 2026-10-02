//! The backups module in the browser preview: every build has two worlds; backups live in memory.

use std::collections::HashMap;

use launcher_shared::{AppError, AppResult, ErrorCode};
use module_backups::dto::{BackupDto, BackupSettings, Kind, WorldDto};
use serde_json::{Value, to_value};

const WORLDS: [&str; 2] = ["New World", "Хардкор"];
const DEFAULT_DIR: &str = "C:\\Users\\Steve\\AppData\\Roaming\\Launcher\\minecraft\\backups\\worlds";
/// 2026-09-29 10:00 UTC.
const START: i64 = 1_790_676_000;

pub struct MockBackups {
    /// (build, world) → its backups, the newest first.
    backups: HashMap<(String, String), Vec<BackupDto>>,
    settings: BackupSettings,
    made: i64,
}

impl Default for MockBackups {
    fn default() -> MockBackups {
        MockBackups {
            backups: HashMap::new(),
            settings: BackupSettings {
                enabled: false,
                keep: 3,
                dir: DEFAULT_DIR.into(),
                default_dir: DEFAULT_DIR.into(),
            },
            made: 0,
        }
    }
}

fn text(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn json<T: serde::Serialize>(value: T) -> AppResult<Value> {
    to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

impl MockBackups {
    /// A world's backups; each world starts with one automatic backup.
    fn list(&mut self, key: &str, world: &str) -> &mut Vec<BackupDto> {
        self.backups.entry((key.to_string(), world.to_string())).or_insert_with(|| {
            vec![BackupDto {
                zip_name: "[Auto] 2026-09-28_18-30-00.zip".into(),
                kind: Kind::Auto,
                created: START - 55_800,
                size: 48_300_000,
            }]
        })
    }

    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        let (key, world) = (text(args, "key"), text(args, "world"));
        match command {
            "settings" => json(&self.settings),
            "set_settings" => {
                let wanted: BackupSettings = serde_json::from_value(args.clone())
                    .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
                if wanted.keep < 1 {
                    return Err(AppError::new(ErrorCode::InvalidInput, "keep at least one"));
                }
                self.settings = BackupSettings { default_dir: DEFAULT_DIR.into(), ..wanted };
                json(&self.settings)
            }
            "worlds" => {
                let worlds: Vec<WorldDto> = WORLDS
                    .iter()
                    .map(|name| WorldDto {
                        folder: (*name).into(),
                        size: 126_000_000,
                        modified: START - 3_600,
                        backups: self.list(&key, name).len(),
                        backups_dir: format!("{DEFAULT_DIR}\\{key}\\{name}"),
                    })
                    .collect();
                json(worlds)
            }
            "backups" => {
                let list = self.list(&key, &world).clone();
                json(list)
            }
            "create" => {
                self.made += 1;
                let created = START + self.made * 60;
                let backup = BackupDto {
                    zip_name: format!("2026-09-29_10-{:02}-00.zip", self.made),
                    kind: Kind::Manual,
                    created,
                    size: 51_200_000,
                };
                self.list(&key, &world).insert(0, backup.clone());
                json(backup)
            }
            "restore" => Ok(Value::Null),
            "delete" => {
                let zip = text(args, "zip_name");
                self.list(&key, &world).retain(|b| b.zip_name != zip);
                Ok(Value::Null)
            }
            "delete_build" => {
                self.backups.retain(|(build, _), _| *build != key);
                for name in WORLDS {
                    self.backups.insert((key.clone(), name.to_string()), Vec::new());
                }
                Ok(Value::Null)
            }
            other => Err(AppError::new(ErrorCode::NotFound, format!("mock: no backups command {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_mock_world_is_backed_up_restored_and_its_backups_deleted() {
        let mut mock = MockBackups::default();
        let worlds: Vec<WorldDto> =
            serde_json::from_value(mock.handle("worlds", &json!({"key": "aero"})).unwrap()).unwrap();
        assert_eq!(worlds.len(), 2);
        let made: BackupDto = serde_json::from_value(
            mock.handle("create", &json!({"key": "aero", "world": worlds[0].folder})).unwrap(),
        )
        .unwrap();
        let listed = |mock: &mut MockBackups| -> Vec<BackupDto> {
            serde_json::from_value(
                mock.handle("backups", &json!({"key": "aero", "world": worlds[0].folder})).unwrap(),
            )
            .unwrap()
        };
        assert_eq!(listed(&mut mock).len(), 2, "one made here, one there already");
        let args = json!({"key": "aero", "world": worlds[0].folder, "zip_name": made.zip_name});
        mock.handle("restore", &args).unwrap();
        mock.handle("delete", &args).unwrap();
        assert_eq!(listed(&mut mock).len(), 1);
        mock.handle("delete_build", &json!({"key": "aero"})).unwrap();
        assert!(listed(&mut mock).is_empty());
        let settings: BackupSettings =
            serde_json::from_value(mock.handle("settings", &json!(null)).unwrap()).unwrap();
        let changed = BackupSettings { enabled: true, keep: 5, ..settings };
        mock.handle("set_settings", &serde_json::to_value(&changed).unwrap()).unwrap();
        let again: BackupSettings =
            serde_json::from_value(mock.handle("settings", &json!(null)).unwrap()).unwrap();
        assert_eq!((again.enabled, again.keep), (true, 5));
    }
}
