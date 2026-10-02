//! Installed content shows its project's icon: one `GET /projects` per unknown set, kept for the
//! session; a failed ask leaves the notes as they are.

mod support;

use launcher_shared::ContentKind;
use launcher_shared::provider::{InstallArgs, OverviewArgs};
use serde_json::json;
use support::*;

const NEOFORGE: &str = "neoforge-21.1.209";

fn overview_args(key: &str) -> OverviewArgs {
    OverviewArgs { key: key.into(), kind: ContentKind::Mods, check_updates: false }
}

/// Installs version `<project>-1` of `project`, whose page has an icon.
async fn with_mod(w: &World, key: &str, project: &str) {
    let id = format!("{project}-1");
    w.server.publish(
        json!({"id": project, "slug": project, "title": project, "project_type": "mod",
               "icon_url": format!("https://cdn.example/{project}.png")}),
        vec![release(&w.server, project, &id, &["neoforge"], 1, json!([]))],
    );
    let args = InstallArgs {
        version_id: Some(id),
        ..InstallArgs::new(key, ContentKind::Mods, project, "", project)
    };
    install(w, args).await.unwrap();
}

fn icon_asks(w: &World) -> usize {
    w.server.seen().iter().filter(|(path, _)| path.starts_with("/v2/projects")).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn overview_notes_carry_project_icons() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM").await;
    with_mod(&w, &key, "LITHIUM").await;
    let overview = w.service.overview(&overview_args(&key)).await.unwrap();
    let mut icons: Vec<(String, Option<String>)> =
        overview.notes.iter().map(|n| (n.project_id.clone(), n.icon_url.clone())).collect();
    icons.sort();
    assert_eq!(
        icons,
        [
            ("LITHIUM".to_string(), Some("https://cdn.example/LITHIUM.png".to_string())),
            ("SODIUM".to_string(), Some("https://cdn.example/SODIUM.png".to_string())),
        ]
    );
    assert_eq!(icon_asks(&w), 1, "one ask for all of them");
}

#[tokio::test(flavor = "multi_thread")]
async fn icons_are_asked_once_per_session() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM").await;
    w.service.overview(&overview_args(&key)).await.unwrap();
    let again = w.service.overview(&overview_args(&key)).await.unwrap();
    assert!(again.notes[0].icon_url.is_some());
    assert_eq!(icon_asks(&w), 1, "the second overview knows them");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_known_projects_no_icon_request() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    std::fs::create_dir_all(dir.join("mods")).unwrap();
    std::fs::write(dir.join("mods/own.jar"), b"made by hand").unwrap();
    let overview = w.service.overview(&overview_args(&key)).await.unwrap();
    assert!(overview.notes.is_empty());
    assert_eq!(icon_asks(&w), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn icons_failing_leave_the_notes() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM").await;
    w.server.data.lock().unwrap().projects_status = Some(500);
    let overview = w.service.overview(&overview_args(&key)).await.unwrap();
    assert_eq!(overview.notes.len(), 1, "the notes stay");
    assert_eq!(overview.notes[0].icon_url, None);
    // A failure is not remembered: the next overview asks again.
    w.server.data.lock().unwrap().projects_status = None;
    let later = w.service.overview(&overview_args(&key)).await.unwrap();
    assert_eq!(later.notes[0].icon_url.as_deref(), Some("https://cdn.example/SODIUM.png"));
}

/// Publishes `project` with version `<project>-1` (its jar holds the bytes of that id), which
/// Modrinth names by its hash.
fn publish(w: &World, project: &str) {
    let id = format!("{project}-1");
    let version = release(&w.server, project, &id, &["neoforge", "fabric"], 1, json!([]));
    w.server.know(id.as_bytes(), &version);
    w.server.publish(
        json!({"id": project, "slug": project, "title": project, "project_type": "mod"}),
        vec![version],
    );
}

fn marks(overview: &launcher_shared::provider::Overview) -> Vec<(String, bool)> {
    let mut marks: Vec<(String, bool)> =
        overview.notes.iter().map(|n| (n.file.clone(), n.installed)).collect();
    marks.sort();
    marks
}

#[tokio::test(flavor = "multi_thread")]
async fn files_modrinth_installed_are_its_own_and_ones_named_by_hash_are_not() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM").await;
    // Lithium's jar, put there by hand: Modrinth names it by its hash; the launcher did not install it.
    publish(&w, "LITHIUM");
    std::fs::write(dir.join("mods/lithium-copy.jar"), b"LITHIUM-1").unwrap();
    let overview = w.service.overview(&overview_args(&key)).await.unwrap();
    assert_eq!(
        marks(&overview),
        [("SODIUM-1.jar".to_string(), true), ("lithium-copy.jar".to_string(), false)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_modrinth_pack_s_files_are_modrinth_s_own() {
    let w = world().await;
    publish(&w, "LITHIUM");
    let pack = mrpack_of(vec![pack_mod(&w.server, "lithium.jar", b"LITHIUM-1")], &[], "1.0");
    publish_pack(&w.server, "SPEEDY", "sp-1", &pack);
    let done = w
        .service
        .install_pack(&launcher_shared::provider::PackInstallArgs {
            project_id: "SPEEDY".into(),
            version_id: "sp-1".into(),
            name: "Швидка".into(),
            icon_url: None,
        })
        .await
        .unwrap();
    let overview = w.service.overview(&overview_args(&done.key)).await.unwrap();
    assert_eq!(marks(&overview), [("lithium.jar".to_string(), true)], "the pack put it there");
}
