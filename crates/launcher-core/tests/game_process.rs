use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use launcher_core::launch::process::{
    GameCommand, GameProcess, LAUNCH_LOG, Spawner, SystemSpawner, crash_artifact, log_tail,
    prepare_launch_log,
};
use serde_json::{Value, json};

fn fake_game() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-game"))
}

fn wait(process: &mut Box<dyn GameProcess>) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(code) = process.try_wait().unwrap() {
            return code;
        }
        assert!(Instant::now() < deadline, "the fake game did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn age(path: &std::path::Path, secs: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
}

#[test]
fn the_game_runs_in_its_folder_with_output_in_the_launch_log() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("games").join("aero");
    fs::create_dir_all(&game).unwrap();
    let log = prepare_launch_log(&game, "fabric-loader-0.16.5-1.21.1", "1.21.1").unwrap();
    assert_eq!(log, game.join("logs").join(LAUNCH_LOG));
    let env = vec![
        ("DRI_PRIME".to_string(), "1".to_string()),
        ("JAVA_TOOL_OPTIONS".to_string(), "-Xmx1M".to_string()),
    ];
    let argv = vec![fake_game().to_string_lossy().into_owned(), "-Dfake.exit=3".into(), "Main".into()];
    let command = GameCommand::new(argv, &game, env, Some(log.clone()));
    assert_eq!(
        (command.program.clone(), command.args.clone()),
        (fake_game(), vec!["-Dfake.exit=3".to_string(), "Main".to_string()])
    );
    let mut process = SystemSpawner.spawn(&command).unwrap();
    assert!(process.pid() > 0);
    assert_eq!(wait(&mut process), Some(3));
    let record: Value =
        serde_json::from_str(&fs::read_to_string(game.join("fake-game.json")).unwrap()).unwrap();
    assert_eq!(record["args"], json!(["-Dfake.exit=3", "Main"]));
    assert_eq!(record["dri_prime"], json!("1"));
    assert_eq!(record["java_tool_options"], Value::Null, "Java option variables are removed");
    assert_eq!(fs::canonicalize(record["cwd"].as_str().unwrap()).unwrap(), fs::canonicalize(&game).unwrap());
    let text = fs::read_to_string(&log).unwrap();
    assert!(
        text.starts_with(
            "Minecraft process diagnostics\nloader=fabric-loader-0.16.5-1.21.1\nminecraft=1.21.1\ngame_dir="
        ),
        "{text}"
    );
    assert!(text.contains("\nstarted_at=") && text.contains("\n\nfake game started"), "{text}");
}

#[test]
fn a_game_can_be_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let argv = vec![fake_game().to_string_lossy().into_owned(), "-Dfake.after=30000".into()];
    let mut process = SystemSpawner.spawn(&GameCommand::new(argv, dir.path(), Vec::new(), None)).unwrap();
    assert_eq!(process.try_wait().unwrap(), None);
    process.kill().unwrap();
    let started = Instant::now();
    wait(&mut process);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn log_tails_keep_the_last_lines_of_the_last_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latest.log");
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    fs::write(&path, lines.join("\n")).unwrap();
    let tail = log_tail(&path).unwrap();
    assert_eq!(tail.lines().count(), 40);
    assert!(tail.starts_with("line 60") && tail.ends_with("line 99"), "{tail}");
    let big = format!("{}\nend", "x".repeat(600 * 1024));
    fs::write(&path, big).unwrap();
    let tail = log_tail(&path).unwrap();
    assert!(tail.len() <= 256 * 1024 && tail.ends_with("end"));
}

#[test]
fn the_crash_artifact_is_the_freshest_explanation() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path();
    let launched = SystemTime::now();
    assert_eq!(crash_artifact(game, launched), None);
    fs::create_dir_all(game.join("logs")).unwrap();
    fs::create_dir_all(game.join("crash-reports")).unwrap();
    let launch_log = game.join("logs").join(LAUNCH_LOG);
    fs::write(&launch_log, "header").unwrap();
    assert_eq!(crash_artifact(game, launched), Some(launch_log.clone()));
    let latest = game.join("logs").join("latest.log");
    fs::write(&latest, "log").unwrap();
    assert_eq!(crash_artifact(game, launched), Some(latest.clone()));
    let old_report = game.join("crash-reports").join("crash-2020-01-01_00.00.00-client.txt");
    fs::write(&old_report, "old").unwrap();
    age(&old_report, 3600);
    assert_eq!(crash_artifact(game, launched), Some(latest.clone()), "an old report is someone else's");
    let report = game.join("crash-reports").join("crash-2026-09-27_12.00.00-client.txt");
    fs::write(&report, "fresh").unwrap();
    assert_eq!(crash_artifact(game, launched), Some(report));
}
