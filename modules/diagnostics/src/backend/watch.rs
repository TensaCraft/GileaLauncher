//! The module's eye on every started game: a mod loader that gave up and waits on its own error
//! window means the game cannot go on; a crash is diagnosed, kept beside the build and announced
//! to the UI.

use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::feedback::EventSink;
use launcher_core::launch::hooks::{GameWatch, GameWatcher, WatchedGame};

use super::artifacts::case_of;
use super::loader::LaunchLog;
use super::rules::rules;
use super::{diagnose, keep};
use crate::dto::{DIAGNOSIS_EVENT, DiagnosisEvent};

pub struct CrashWatcher;

impl GameWatcher for CrashWatcher {
    fn watch(&self, game: &WatchedGame) -> Box<dyn GameWatch> {
        Box::new(CrashWatch { log: LaunchLog::new(&game.game_dir, game.launched_at), game: game.clone() })
    }
}

struct CrashWatch {
    game: WatchedGame,
    log: LaunchLog,
}

impl GameWatch for CrashWatch {
    fn cannot_go_on(&mut self) -> bool {
        let gave_up = self.log.loader_gave_up();
        if gave_up {
            tracing::warn!(
                "The mod loader of {} gave up and waits on its error window",
                self.game.build_name
            );
        }
        gave_up
    }

    fn crashed(&mut self, sink: &dyn EventSink) -> bool {
        let game = &self.game;
        let case = case_of(&game.game_dir, game.launched_at, false);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let diagnosis = diagnose(&case, &rules(), now);
        if let Err(e) = keep(&game.game_dir, &diagnosis) {
            tracing::warn!("Unable to keep the diagnosis of {}: {e}", game.build_name);
        }
        tracing::info!("Diagnosis of {}: {}", game.build_name, diagnosis.findings[0].id);
        let event = DiagnosisEvent {
            build_key: game.build_key.clone(),
            build_name: game.build_name.clone(),
            diagnosis,
        };
        match serde_json::to_value(&event) {
            Ok(payload) => {
                sink.module_event(DIAGNOSIS_EVENT, &payload);
                true
            }
            Err(e) => {
                tracing::warn!("Unable to announce the diagnosis of {}: {e}", game.build_name);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    use launcher_core::launch::process::LAUNCH_LOG;
    use launcher_shared::{ActivityEntry, Alert, OpsSnapshot, Toast};
    use serde_json::Value;

    use super::*;
    use crate::backend::rules::FABRIC_LOG_FOR_TESTS;
    use crate::dto::{DIAGNOSIS_EVENT, DiagnosisEvent};

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for Recorder {
        fn ops(&self, _: &OpsSnapshot) {}
        fn activity(&self, _: &ActivityEntry) {}
        fn toast(&self, _: &Toast) {}
        fn alert(&self, _: &Alert) {}
        fn module_event(&self, event: &str, payload: &Value) {
            self.events.lock().unwrap().push((event.to_string(), payload.clone()));
        }
    }

    fn game(dir: &std::path::Path) -> WatchedGame {
        WatchedGame {
            build_key: "aero".into(),
            build_name: "Aero".into(),
            game_dir: dir.to_path_buf(),
            launched_at: SystemTime::now(),
        }
    }

    fn launch_log(dir: &std::path::Path) -> std::path::PathBuf {
        let log = dir.join("logs").join(LAUNCH_LOG);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        log
    }

    #[test]
    fn a_loader_that_gave_up_means_the_game_cannot_go_on() {
        let dir = tempfile::tempdir().unwrap();
        let log = launch_log(dir.path());
        std::fs::write(&log, "Minecraft process diagnostics\n").unwrap();
        let mut watch = CrashWatcher.watch(&game(dir.path()));
        assert!(!watch.cannot_go_on());
        let body = std::fs::read_to_string(&log).unwrap() + FABRIC_LOG_FOR_TESTS;
        std::fs::write(&log, body).unwrap();
        assert!(watch.cannot_go_on());
    }

    #[test]
    fn a_loader_error_left_from_an_earlier_launch_is_not_this_one() {
        let dir = tempfile::tempdir().unwrap();
        let log = launch_log(dir.path());
        std::fs::write(&log, FABRIC_LOG_FOR_TESTS).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&log).unwrap().set_modified(old).unwrap();
        assert!(!CrashWatcher.watch(&game(dir.path())).cannot_go_on());
    }

    #[test]
    fn a_crash_is_diagnosed_kept_and_announced() {
        let dir = tempfile::tempdir().unwrap();
        let latest = dir.path().join("logs").join("latest.log");
        std::fs::create_dir_all(latest.parent().unwrap()).unwrap();
        let locked = "java.io.IOException: The process cannot access the file because it is being used by another process";
        std::fs::write(&latest, locked).unwrap();
        let recorder = Recorder::default();
        let told = CrashWatcher.watch(&game(dir.path())).crashed(&recorder);
        assert!(told, "the dialog tells of it");
        let events = recorder.events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, DIAGNOSIS_EVENT);
        let event: DiagnosisEvent = serde_json::from_value(events[0].1.clone()).unwrap();
        assert_eq!((event.build_key.as_str(), event.build_name.as_str()), ("aero", "Aero"));
        assert_eq!(event.diagnosis.findings[0].id, "files.locked");
        assert_eq!(crate::backend::last(dir.path()).unwrap().findings[0].id, "files.locked");
    }
}
