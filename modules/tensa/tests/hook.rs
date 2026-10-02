mod support;

use std::sync::Arc;

use launcher_core::feedback::OperationSpec;
use launcher_core::launch::hooks::{LaunchHook, PrepareContext};
use launcher_core::launch::options::game_dir;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::Build;
use launcher_shared::{ErrorCode, Text};
use module_tensa::backend::hook::ServerSync;
use module_tensa::backend::identity::CLIENT;
use module_tensa::backend::service::force_sync;
use serde_json::json;
use support::World;

/// The server build `aero` (Fabric 0.19.5 for 26.3) lists `mods/a.jar`.
fn server(w: &World) {
    w.catalog(
        json!({"id": "aero", "minecraft_version": "26.3", "loader_id": "fabric", "loader_version": "0.19.5",
                     "force_update_endpoint": w.server.url("/aero/force")}),
    );
    let url = w.file("a.jar", "jar-a");
    w.server.json(
        "/aero/force",
        json!({"files": [{"relative_path": "mods/a.jar", "download_url": url, "sha1": "00"}],
                                         "directories": [{"path": "mods"}]}),
    );
}

fn build(w: &World, client: &str, managed: bool) -> Build {
    let mut build = Build::new("Aero");
    build.version = Some("26.3".into());
    build.loader = Some("fabric-loader-0.19.5-26.3".into());
    build.loader_version = Some("0.19.5".into());
    build.client = Some(client.into());
    if managed {
        build.remote_pack_id = Some(json!("aero"));
    }
    w.deps.versions.create(&mut build).unwrap();
    build
}

async fn prepare(w: &World, build: &Build) -> launcher_shared::AppResult<()> {
    let config = ConfigStore::open(w.tmp.path().join("config.json"));
    let game = game_dir(build, w.deps.versions.minecraft_dir());
    let op = w.deps.feedback.begin(OperationSpec::new(Text::key("syncing_files_check"), "launch").hidden());
    let ctx = PrepareContext {
        build,
        game_dir: &game,
        config: &config,
        feedback: &w.deps.feedback,
        versions: &w.deps.versions,
        components: w.deps.components.as_ref(),
        downloader: &w.deps.downloader,
        op: &op,
        running: false,
    };
    ServerSync::new(w.deps.api.clone()).prepare_build(&ctx).await
}

#[tokio::test]
async fn the_step_syncs_managed_builds() {
    let w = World::start().await;
    server(&w);
    let managed = build(&w, CLIENT, true);
    // The listed hash is not the file's: the download fails, and so does the step (the launch stops).
    let error = prepare(&w, &managed).await.unwrap_err();
    assert_ne!(error.code, ErrorCode::Internal);
    assert!(w.server.seen().iter().any(|s| s == "/files/a.jar"), "it synced");
}

#[tokio::test]
async fn the_step_leaves_builds_it_does_not_manage_alone() {
    let w = World::start().await;
    server(&w);
    let plain = build(&w, "Fabric", false);
    prepare(&w, &plain).await.unwrap();
    let mut copy = Build::new("Copy");
    copy.client = Some(CLIENT.into());
    copy.remote_pack_id = Some(json!("aero"));
    copy.options.insert("syncMode".into(), json!("manual"));
    w.deps.versions.create(&mut copy).unwrap();
    prepare(&w, &copy).await.unwrap();
    assert!(w.server.seen().is_empty(), "the server was not asked: {:?}", w.server.seen());
    assert_eq!(w.deps.versions.get(&plain.key).unwrap(), plain);
}

#[tokio::test]
async fn force_sync_refuses_a_running_build_and_an_unknown_one() {
    let w = World::start().await;
    server(&w);
    let managed = build(&w, CLIENT, true);
    let mut running = (*w.deps).clone();
    running.running = Arc::new(|_: &str| true);
    assert_eq!(force_sync(&running, &managed.key).await.unwrap_err().code, ErrorCode::GameRunning);
    assert_eq!(force_sync(&w.deps, "nope").await.unwrap_err().code, ErrorCode::VersionNotFound);
    assert!(w.server.seen().is_empty());
}

#[tokio::test]
async fn force_sync_refuses_a_build_the_server_does_not_manage() {
    let w = World::start().await;
    server(&w);
    let plain = build(&w, "Fabric", false);
    assert_eq!(force_sync(&w.deps, &plain.key).await.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(w.server.seen().is_empty(), "the server was not asked");
    assert_eq!(w.deps.versions.get(&plain.key).unwrap(), plain);
}
