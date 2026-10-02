//! `--apply-update` must be handled before Tauri starts: no window, works without a display.

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn apply_update_mode_runs_without_the_gui() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("cache").join("pending-update").join("pending_update.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_launcher-app"))
        .arg("--apply-update")
        .arg(&marker)
        .args(["--wait-pid", "1"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("--apply-update started the GUI instead of the helper");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(2), "missing marker → EXIT_INVALID");
}
