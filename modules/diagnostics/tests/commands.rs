use std::fs;

use launcher_core::launch::options::game_dir;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::ErrorCode;
use module_diagnostics::backend::build_diagnostics;

#[test]
fn a_build_s_diagnostics_name_its_folders_and_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("mc"));
    fs::create_dir_all(&state).unwrap();
    let versions = VersionStore::open(&state, &mc);
    let mut build = Build::new("Aero");
    build.version = Some("1.21.1".into());
    versions.create(&mut build).unwrap();
    let game = game_dir(&build, versions.minecraft_dir());
    let found = build_diagnostics(&versions, &build.key).unwrap();
    assert_eq!(found.game_dir, game.to_string_lossy());
    assert_eq!(found.latest_log, game.join("logs").join("latest.log").to_string_lossy());
    assert_eq!(found.last, None, "no crash yet");
    let missing = build_diagnostics(&versions, "nope").unwrap_err();
    assert_eq!(missing.code, ErrorCode::VersionNotFound);
}
