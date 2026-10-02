use std::path::PathBuf;

use launcher_core::loaders::processors::{JavaProcessorRunner, ProcessorCall, ProcessorRunner};
use serde_json::{Value, json};

/// Records how it was started in `fake-game.json`; `-Dfake.exit=N` sets its exit code.
fn fake_java() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-game"))
}

#[test]
fn processors_run_as_java_with_their_classpath() {
    let dir = tempfile::tempdir().unwrap();
    let call = ProcessorCall {
        jar: "net.fake:patcher:1.0".into(),
        classpath: vec![dir.path().join("a.jar"), dir.path().join("b.jar")],
        main_class: "net.fake.Patcher".into(),
        args: vec!["--output".into(), "out.jar".into()],
        outputs: Vec::new(),
    };
    JavaProcessorRunner.run(&fake_java(), &call, dir.path()).unwrap();
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("fake-game.json")).unwrap()).unwrap();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = format!(
        "{}{separator}{}",
        dir.path().join("a.jar").to_string_lossy(),
        dir.path().join("b.jar").to_string_lossy()
    );
    assert_eq!(record["args"], json!(["-cp", classpath, "net.fake.Patcher", "--output", "out.jar"]));
    let failing = ProcessorCall { args: vec!["-Dfake.exit=3".into()], ..call };
    let error = JavaProcessorRunner.run(&fake_java(), &failing, dir.path()).unwrap_err();
    assert!(error.contains("fake game started") && error.contains("net.fake.Patcher"), "{error}");
}
