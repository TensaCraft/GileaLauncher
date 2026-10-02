mod support;

use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::Build;
use module_tensa::backend::home::{home_packs, install_from_home, set_shown, shown};
use module_tensa::backend::identity::CLIENT;
use serde_json::json;
use support::World;

fn config(w: &World) -> ConfigStore {
    ConfigStore::open(w.tmp.path().join("config.json"))
}

fn two(w: &World) {
    w.server.json(
        "/api/mods",
        json!([
            {"client": {"id": "aero", "name": "Aero", "minecraft_version": "26.3", "loader_id": "fabric",
                        "loader_version": "0.19.5", "description": "Planes", "image": "a.png",
                        "files_endpoint": w.server.url("/files-list")}},
            {"client": {"id": "tensa-lite", "name": "Tensa", "minecraft_version": "1.21.1", "loader_id": "neoforge"}}
        ]),
    );
    w.server.json("/files-list", json!({"files": []}));
}

fn server_build(w: &World, name: &str, pack: &str) -> Build {
    let mut build = Build::new(name);
    build.version = Some("26.3".into());
    build.client = Some(CLIENT.into());
    build.remote_pack_id = Some(json!(pack));
    w.deps.versions.create(&mut build).unwrap();
    build
}

#[tokio::test]
async fn home_packs_list_the_server_builds_not_installed_yet() {
    let w = World::start().await;
    two(&w);
    let packs = home_packs(&w.deps, &config(&w)).await.unwrap();
    assert_eq!(packs.len(), 2);
    assert_eq!(
        packs[0],
        json!({"id": "aero", "name": "Aero", "description": "Planes", "image": "a.png", "runs": "Fabric 26.3"})
    );
    assert_eq!(packs[1]["description"], json!(null));
}

#[tokio::test]
async fn installed_server_builds_are_no_stubs() {
    let w = World::start().await;
    two(&w);
    server_build(&w, "My Aero", "AERO");
    let mut plain = Build::new("Tensa");
    plain.client = Some("Fabric".into());
    w.deps.versions.create(&mut plain).unwrap();
    let packs = home_packs(&w.deps, &config(&w)).await.unwrap();
    assert_eq!(packs.iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["tensa-lite"]);
}

#[tokio::test]
async fn home_packs_are_none_when_switched_off() {
    let w = World::start().await;
    two(&w);
    let config = config(&w);
    set_shown(&config, false).unwrap();
    assert!(home_packs(&w.deps, &config).await.unwrap().is_empty());
    assert!(w.server.seen().is_empty(), "the server is not even asked");
}

#[tokio::test]
async fn home_packs_fail_when_the_server_is_away() {
    let w = World::start().await;
    w.server.reply("/api/mods", 500, "{}");
    // Home says the server is away rather than showing nothing to install.
    assert!(home_packs(&w.deps, &config(&w)).await.is_err());
}

#[tokio::test]
async fn a_home_install_takes_a_free_name() {
    let w = World::start().await;
    two(&w);
    let mut own = Build::new("Aero");
    own.client = Some("Fabric".into());
    w.deps.versions.create(&mut own).unwrap();
    let mut second = Build::new("aero 2");
    w.deps.versions.create(&mut second).unwrap();
    let installed = install_from_home(&w.deps, "aero").await.unwrap();
    assert_eq!(installed.name, "Aero 3");
    assert_eq!(installed.remote_pack_id, Some(json!("aero")));
}

#[test]
fn settings_switch_the_home_cards() {
    let dir = tempfile::tempdir().unwrap();
    let config = ConfigStore::open(dir.path().join("config.json"));
    assert!(shown(&config), "on by default");
    set_shown(&config, false).unwrap();
    assert!(!shown(&config));
    assert_eq!(config.get_str("show_server_builds").as_deref(), Some("no"));
    set_shown(&config, true).unwrap();
    assert!(shown(&config));
}

#[tokio::test]
async fn a_failed_home_install_raises_a_reportable_alert() {
    let w = World::start().await;
    w.catalog(json!({"id": "aero", "name": "Aero", "minecraft_version": "26.3", "loader_id": "fabric",
                     "loader_version": "0.19.5", "files_endpoint": w.server.url("/broken")}));
    w.server.reply("/broken", 500, "{}");
    assert!(install_from_home(&w.deps, "aero").await.is_err());
    let alerts = w.alerts.list();
    assert_eq!(alerts.len(), 1, "one alert says it failed");
    assert!(alerts[0].allow_report);
    let report = w.deps.feedback.report_context(alerts[0].id).expect("the failure can be reported");
    assert_eq!((report.screen.as_str(), report.action.as_str()), ("Home", "tensacraft_install"));
    assert_eq!(report.metadata["pack_id"], "aero");
    assert_eq!(report.metadata["version_name"], "Aero", "named as the catalog names it");
}
