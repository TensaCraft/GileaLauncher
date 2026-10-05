//! Watches a started game: an exit within the first seconds is a crash
//! whatever the code; later, anything but code 0 is. A game the launcher stopped just exits.
//! Modules' watches (`GameWatch`) may see that a running game cannot go on (it is stopped and
//! counts as a crash) and may tell the user what a crash was; otherwise a crash raises an alert
//! that opens its report and logs (nothing of a game's crash is sent).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use launcher_shared::{AlertFile, GameEvent, GameState, Level, Text};

use super::hooks::GameWatch;
use super::process::{LAUNCH_LOG, SharedProcess, crash_artifact, log_tail, ran_out_of_memory};
use super::registry::LaunchRegistry;
use crate::feedback::{EventSink, FeedbackService};

pub const EARLY_EXIT: Duration = Duration::from_secs(5);
pub const POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct Watch {
    /// Registry key of the game folder.
    pub key: String,
    pub build_key: String,
    pub build_name: String,
    pub game_dir: PathBuf,
    pub launched_at: SystemTime,
    pub close_launcher: bool,
    pub early_exit: Duration,
    pub poll: Duration,
}

fn emit(sink: &dyn EventSink, watch: &Watch, state: GameState) {
    sink.game(&GameEvent { build_key: watch.build_key.clone(), build_name: watch.build_name.clone(), state });
}

/// What the crash's alert opens: the crash's own file (a crash report, Java's `hs_err` log), the
/// game's log and the folder of its logs, each that is there. A game's crash is its build's (its
/// mods, most often), not the launcher's: nothing of it is sent.
fn crash_files(watch: &Watch, artifact: Option<&Path>) -> Vec<AlertFile> {
    let logs = watch.game_dir.join("logs");
    let latest = logs.join("latest.log");
    let file = |label: &str, path: &Path, folder: bool| AlertFile {
        label: Text::key(label),
        path: path.to_string_lossy().into_owned(),
        folder,
    };
    let mut files = Vec::new();
    if let Some(own) = artifact.filter(|a| *a != latest && *a != logs.join(LAUNCH_LOG)) {
        files.push(file("open_crash_report", own, false));
    }
    if latest.is_file() {
        files.push(file("open_latest_log", &latest, false));
    }
    if logs.is_dir() {
        files.push(file("open_version_logs", &logs, true));
    }
    files
}

fn crashed(
    code: Option<i32>,
    early: bool,
    watch: &Watch,
    watches: &mut [Box<dyn GameWatch>],
    feedback: &FeedbackService,
    sink: &dyn EventSink,
) -> GameState {
    let artifact = crash_artifact(&watch.game_dir, watch.launched_at);
    let when = if early { "during startup" } else { "with an error" };
    tracing::error!("Minecraft {} exited {when} (code {code:?})", watch.build_name);
    if let Some(path) = &artifact
        && let Ok(tail) = log_tail(path)
    {
        tracing::error!("Last lines of {}:\n{tail}", path.display());
    }
    let shown = artifact.clone().unwrap_or_else(|| watch.game_dir.clone());
    let key = if ran_out_of_memory(&watch.game_dir, artifact.as_deref()) {
        "version_crashed_out_of_memory"
    } else {
        "version_crashed_open_logs"
    };
    let message = Text::key(key).param("path", shown.to_string_lossy());
    // Every watch hears of it; one that told the user leaves the alert to the activity log.
    let told = watches.iter_mut().fold(false, |told, w| w.crashed(sink) | told);
    if told {
        feedback.note(Level::Error, message);
    } else {
        let files = crash_files(watch, artifact.as_deref());
        feedback.alert_with_files(Level::Error, Text::key("warning"), message, files);
    }
    GameState::Crashed { code, early, log: artifact.map(|p| p.to_string_lossy().into_owned()) }
}

/// Blocks until the game exits: `Running` once it outlives the early window, then its final
/// state. Unregisters it at the end. Meant for a thread of its own.
pub fn watch_game(
    process: SharedProcess,
    watch: Watch,
    mut watches: Vec<Box<dyn GameWatch>>,
    registry: &LaunchRegistry,
    feedback: &FeedbackService,
    sink: &dyn EventSink,
) -> GameState {
    let pid = process.lock().unwrap_or_else(|e| e.into_inner()).pid();
    let started = Instant::now();
    let mut running = false;
    let state = loop {
        let exit = process.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
        match exit {
            Ok(Some(code)) if registry.take_stopped(pid) => break GameState::Exited { code },
            Ok(Some(code)) if !running || code != Some(0) => {
                break crashed(code, !running, &watch, &mut watches, feedback, sink);
            }
            Ok(Some(code)) => break GameState::Exited { code },
            Ok(None) if watches.iter_mut().any(|w| w.cannot_go_on()) => {
                tracing::warn!("Minecraft {} cannot go on; stopping it (pid {pid})", watch.build_name);
                let mut game = process.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = game.kill() {
                    tracing::warn!("Unable to stop Minecraft (pid {pid}): {e}");
                }
                let _ = game.try_wait();
                drop(game);
                break crashed(None, !running, &watch, &mut watches, feedback, sink);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Unable to watch Minecraft {} (pid {pid}): {e}", watch.build_name);
                break GameState::Exited { code: None };
            }
        }
        if !running && started.elapsed() >= watch.early_exit {
            running = true;
            emit(sink, &watch, GameState::Running { close_launcher: watch.close_launcher });
        }
        std::thread::sleep(watch.poll);
    };
    tracing::info!("Minecraft {} ended (pid {pid}): {state:?}", watch.build_name);
    registry.unregister(&watch.key, pid);
    emit(sink, &watch, state.clone());
    state
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use launcher_shared::{ActivityEntry, Alert, OpsSnapshot, Toast};

    use super::*;
    use crate::launch::process::fake::FakeProcess;
    use crate::launch::registry::LAUNCH_COOLDOWN;

    #[derive(Default)]
    struct Recorder {
        games: Mutex<Vec<GameState>>,
        alerts: Mutex<Vec<Alert>>,
        activity: Mutex<Vec<ActivityEntry>>,
    }

    impl EventSink for Recorder {
        fn ops(&self, _: &OpsSnapshot) {}
        fn activity(&self, entry: &ActivityEntry) {
            self.activity.lock().unwrap().push(entry.clone());
        }
        fn toast(&self, _: &Toast) {}
        fn alert(&self, alert: &Alert) {
            self.alerts.lock().unwrap().push(alert.clone());
        }
        fn game(&self, event: &GameEvent) {
            self.games.lock().unwrap().push(event.state.clone());
        }
    }

    /// Gives up from `gives_up_at` on; counts the crashes it hears of and tells of them if `tells`.
    struct TestWatch {
        gives_up_at: Option<Instant>,
        tells: bool,
        crashes: Arc<AtomicUsize>,
    }

    impl GameWatch for TestWatch {
        fn cannot_go_on(&mut self) -> bool {
            self.gives_up_at.is_some_and(|at| Instant::now() >= at)
        }
        fn crashed(&mut self, _sink: &dyn EventSink) -> bool {
            self.crashes.fetch_add(1, Ordering::SeqCst);
            self.tells
        }
    }

    fn test_watch(gives_up_after: Option<Duration>, tells: bool) -> (Box<dyn GameWatch>, Arc<AtomicUsize>) {
        let crashes = Arc::new(AtomicUsize::new(0));
        let gives_up_at = gives_up_after.map(|after| Instant::now() + after);
        (Box::new(TestWatch { gives_up_at, tells, crashes: crashes.clone() }), crashes)
    }

    struct Run {
        state: GameState,
        recorder: Arc<Recorder>,
        registry: Arc<LaunchRegistry>,
        feedback: Arc<FeedbackService>,
    }

    fn watch(dir: &Path) -> Watch {
        Watch {
            key: "k".into(),
            build_key: "aero".into(),
            build_name: "Aero".into(),
            game_dir: dir.to_path_buf(),
            launched_at: SystemTime::now(),
            close_launcher: true,
            early_exit: Duration::from_millis(150),
            poll: Duration::from_millis(5),
        }
    }

    fn run_watched(
        process: FakeProcess,
        dir: &Path,
        stop_after: Option<Duration>,
        watches: Vec<Box<dyn GameWatch>>,
    ) -> Run {
        let recorder = Arc::new(Recorder::default());
        let feedback = FeedbackService::new(recorder.clone());
        let registry = Arc::new(LaunchRegistry::new(LAUNCH_COOLDOWN));
        let process = process.shared();
        registry.register("k", process.clone());
        if let Some(after) = stop_after {
            let registry = registry.clone();
            std::thread::spawn(move || {
                std::thread::sleep(after);
                registry.terminate("k");
            });
        }
        let state = watch_game(process, watch(dir), watches, &registry, &feedback, recorder.as_ref());
        Run { state, recorder, registry, feedback }
    }

    fn run(process: FakeProcess, dir: &Path, stop_after: Option<Duration>) -> Run {
        run_watched(process, dir, stop_after, Vec::new())
    }

    fn crash_alerts(recorder: &Recorder) -> Vec<Text> {
        recorder.alerts.lock().unwrap().iter().map(|a| a.message.clone()).collect()
    }

    #[test]
    fn an_exit_during_startup_is_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(10), Some(0)), dir.path(), None);
        assert_eq!(run.state, GameState::Crashed { code: Some(0), early: true, log: None });
        assert_eq!(
            run.recorder.games.lock().unwrap().clone(),
            std::slice::from_ref(&run.state),
            "no Running before a crash"
        );
        let expected = Text::key("version_crashed_open_logs").param("path", dir.path().to_string_lossy());
        assert_eq!(crash_alerts(&run.recorder), [expected]);
        assert!(!dir.path().join(".launcher").exists(), "nothing is kept beside the build");
        assert!(!run.registry.is_active("k"));
    }

    #[test]
    fn a_clean_exit_after_startup_is_normal() {
        let dir = tempfile::tempdir().unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(600), Some(0)), dir.path(), None);
        assert_eq!(
            run.recorder.games.lock().unwrap().clone(),
            [GameState::Running { close_launcher: true }, GameState::Exited { code: Some(0) }]
        );
        assert!(crash_alerts(&run.recorder).is_empty());
    }

    #[test]
    fn a_later_error_is_a_crash_naming_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let latest = dir.path().join("logs").join("latest.log");
        std::fs::create_dir_all(latest.parent().unwrap()).unwrap();
        std::fs::write(&latest, "java.lang.NullPointerException").unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(600), Some(1)), dir.path(), None);
        let log = Some(latest.to_string_lossy().into_owned());
        assert_eq!(run.state, GameState::Crashed { code: Some(1), early: false, log });
        let expected = Text::key("version_crashed_open_logs").param("path", latest.to_string_lossy());
        assert_eq!(crash_alerts(&run.recorder), [expected]);
    }

    #[test]
    fn a_game_out_of_memory_is_told_so() {
        // Two games of 16 GB each on a 32 GB computer: Java could not get memory and stopped. The
        // alert says what to do rather than "an error".
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("latest.log"),
            "[11:29:15] [Render thread/INFO] [Voxy/]: Creating new world engine",
        )
        .unwrap();
        let hs_err = dir.path().join("hs_err_pid34608.log");
        std::fs::write(
            &hs_err,
            "#
# There is insufficient memory for the Java Runtime Environment to continue.
# Native memory allocation (malloc) failed to allocate 984736 bytes.
",
        )
        .unwrap();
        let run = run(
            FakeProcess::exiting_after(1, Duration::from_millis(600), Some(-1073740791)),
            dir.path(),
            None,
        );
        let expected = Text::key("version_crashed_out_of_memory").param("path", hs_err.to_string_lossy());
        assert_eq!(crash_alerts(&run.recorder), [expected]);
        let alert = run.recorder.alerts.lock().unwrap()[0].clone();
        assert_eq!(alert.files[0].path, hs_err.to_string_lossy(), "the reason opens first");
        assert_eq!(alert.files[0].label, Text::key("open_crash_report"));
    }

    fn opened(alert: &launcher_shared::Alert) -> Vec<(String, String, bool)> {
        let key = |t: &Text| match t {
            Text::Key { key, .. } => key.clone(),
            Text::Raw { text } => text.clone(),
        };
        alert.files.iter().map(|f| (key(&f.label), f.path.clone(), f.folder)).collect()
    }

    #[test]
    fn a_crash_opens_its_report_and_logs_and_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let reports = dir.path().join("crash-reports");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(logs.join("latest.log"), "java.lang.IllegalStateException").unwrap();
        let crash = reports.join("crash-2026-10-05_18.27.26-client.txt");
        std::fs::write(&crash, "---- Minecraft Crash Report ----").unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(600), Some(1)), dir.path(), None);
        let alert = run.recorder.alerts.lock().unwrap()[0].clone();
        assert!(!alert.allow_report, "a game's crash is its builds', not the launcher's");
        assert_eq!(run.feedback.report_context(alert.id), None);
        let path = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            opened(&alert),
            [
                ("open_crash_report".to_string(), path(&crash), false),
                ("open_latest_log".to_string(), path(&logs.join("latest.log")), false),
                ("open_version_logs".to_string(), path(&logs), true),
            ]
        );
    }

    #[test]
    fn a_crash_with_only_its_log_opens_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("latest.log"), "java.lang.OutOfMemoryError").unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(600), Some(1)), dir.path(), None);
        let alert = run.recorder.alerts.lock().unwrap()[0].clone();
        let keys: Vec<String> = opened(&alert).into_iter().map(|(k, ..)| k).collect();
        assert_eq!(keys, ["open_latest_log", "open_version_logs"]);
    }

    #[test]
    fn a_crash_without_logs_offers_nothing_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let run = run(FakeProcess::exiting_after(1, Duration::from_millis(10), Some(1)), dir.path(), None);
        assert!(run.recorder.alerts.lock().unwrap()[0].files.is_empty());
    }

    #[test]
    fn a_stopped_game_just_exits() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, crashes) = test_watch(None, true);
        let run =
            run_watched(FakeProcess::running(1), dir.path(), Some(Duration::from_millis(100)), vec![watch]);
        assert_eq!(run.state, GameState::Exited { code: None });
        assert!(crash_alerts(&run.recorder).is_empty());
        assert_eq!(crashes.load(Ordering::SeqCst), 0, "a stop is no crash to tell of");
        assert!(!run.registry.is_active("k"));
    }

    #[test]
    fn a_game_a_watch_says_cannot_go_on_is_stopped_as_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, crashes) = test_watch(Some(Duration::from_millis(20)), false);
        let started = Instant::now();
        // Only the launcher's stop, long after, would end it otherwise.
        let run = run_watched(FakeProcess::running(1), dir.path(), Some(Duration::from_secs(3)), vec![watch]);
        assert!(started.elapsed() < Duration::from_secs(2), "stopped at once");
        assert!(matches!(run.state, GameState::Crashed { code: None, early: true, .. }), "{:?}", run.state);
        assert_eq!(crashes.load(Ordering::SeqCst), 1);
        assert_eq!(crash_alerts(&run.recorder).len(), 1, "nobody told of it: the plain alert");
        assert!(!run.registry.is_active("k"));
    }

    #[test]
    fn a_crash_a_watch_told_of_raises_no_alert_but_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let (quiet, heard) = test_watch(None, false);
        let (telling, told) = test_watch(None, true);
        let run = run_watched(
            FakeProcess::exiting_after(1, Duration::from_millis(10), Some(1)),
            dir.path(),
            None,
            vec![quiet, telling],
        );
        assert!(matches!(run.state, GameState::Crashed { .. }));
        assert_eq!(
            (heard.load(Ordering::SeqCst), told.load(Ordering::SeqCst)),
            (1, 1),
            "every watch hears of it"
        );
        assert!(crash_alerts(&run.recorder).is_empty());
        let logged = run.recorder.activity.lock().unwrap().clone();
        assert!(
            logged
                .iter()
                .any(|e| matches!(&e.message, Text::Key { key, .. } if key == "version_crashed_open_logs")),
            "{logged:?}"
        );
    }
}
