mod support;

use launcher_shared::ContentKind;
use launcher_shared::provider::{InstallArgs, NewerVersion, OverviewArgs, UpdatesStatus};
use serde_json::json;
use support::*;

/// NeoForge: no implicit Fabric API joins the installs (that rule has its own tests).
const NEOFORGE: &str = "neoforge-21.1.209";

fn args(key: &str, kind: ContentKind) -> OverviewArgs {
    OverviewArgs { key: key.into(), kind, check_updates: true }
}

/// Installs version `id` of `project` (published with `newer` after it).
async fn with_mod(w: &World, key: &str, project: &str, id: &str, newer: Option<&str>) {
    let mut versions = Vec::new();
    if let Some(next) = newer {
        versions.push(release(&w.server, project, next, &["neoforge"], 9, json!([])));
    }
    versions.push(release(&w.server, project, id, &["neoforge"], 1, json!([])));
    w.server.publish(project_json(project, project), versions);
    let args = InstallArgs {
        version_id: Some(id.into()),
        ..InstallArgs::new(key, ContentKind::Mods, project, "", project)
    };
    install(w, args).await.unwrap();
}

fn version_of(w: &World, id: &str) -> serde_json::Value {
    w.server.data.lock().unwrap().version_ids[id].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_enabled_mod_with_a_newer_compatible_version_offers_it() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM", "sod-1", Some("sod-2")).await;
    w.server.offer(b"sod-1", &version_of(&w, "sod-2"));
    let overview = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert_eq!(overview.notes.len(), 1);
    let note = &overview.notes[0];
    assert_eq!(
        (note.file.as_str(), note.project_id.as_str(), note.version_number.as_str()),
        ("sod-1.jar", "SODIUM", "sod-1")
    );
    assert_eq!(
        note.update,
        Some(NewerVersion { version_id: "sod-2".into(), version_number: "sod-2".into() })
    );
    let summary = overview.updates.unwrap();
    assert_eq!((summary.status, summary.available, summary.unchecked), (UpdatesStatus::Available, 1, 0));
    let body = w.server.data.lock().unwrap().update_bodies[0].clone();
    assert_eq!(
        (body["loaders"].clone(), body["game_versions"].clone()),
        (json!(["neoforge"]), json!(["1.21.1"]))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_newer_fitting_file_counts_and_silence_is_unchecked() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SAME", "same-1", None).await;
    with_mod(&w, &key, "OLDER", "old-5", None).await;
    with_mod(&w, &key, "QUILTONLY", "q-1", None).await;
    with_mod(&w, &key, "QUIET", "quiet-1", None).await;
    with_mod(&w, &key, "OFF", "off-1", Some("off-2")).await;
    std::fs::rename(dir.join("mods/off-1.jar"), dir.join("mods/off-1.jar.disabled")).unwrap();
    w.server.offer(b"same-1", &version_of(&w, "same-1"));
    w.server.offer(b"old-5", &release(&w.server, "OLDER", "old-4", &["neoforge"], 1, json!([])));
    w.server.offer(b"q-1", &release(&w.server, "QUILTONLY", "q-2", &["quilt"], 20, json!([])));
    w.server.offer(b"off-1", &version_of(&w, "off-2"));
    let overview = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert!(overview.notes.iter().all(|n| n.update.is_none()), "{:?}", overview.notes);
    assert_eq!(overview.notes.len(), 5, "a disabled mod still has its page");
    let summary = overview.updates.unwrap();
    assert_eq!((summary.status, summary.available, summary.unchecked), (UpdatesStatus::Unchecked, 0, 1));
    let asked = w.server.data.lock().unwrap().update_bodies[0]["hashes"].as_array().unwrap().len();
    assert_eq!(asked, 4, "the disabled mod is not asked about");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_to_check_and_a_failed_check_say_so() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    std::fs::create_dir_all(dir.join("mods")).unwrap();
    std::fs::write(dir.join("mods/handmade.jar"), b"mine").unwrap();
    let quiet = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert_eq!(quiet.updates.unwrap().status, UpdatesStatus::NoEnabled);
    assert!(w.server.data.lock().unwrap().update_bodies.is_empty(), "nothing eligible, nothing asked");
    with_mod(&w, &key, "SODIUM", "sod-1", None).await;
    w.server.data.lock().unwrap().updates_status = Some(500);
    let failed = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert_eq!(failed.updates.unwrap().status, UpdatesStatus::Failed);
    assert_eq!(failed.notes.len(), 1, "the page stays known");
    w.server.data.lock().unwrap().hashes_status = Some(503);
    let offline = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert_eq!(offline.notes.len(), 1, "the provenance alone names it");
    assert_eq!(offline.updates.unwrap().status, UpdatesStatus::Failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_project_in_the_answer_fails_the_check() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM", "sod-1", None).await;
    w.server.offer(b"sod-1", &release(&w.server, "LITHIUM", "lit-9", &["neoforge"], 20, json!([])));
    let overview = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert_eq!(overview.updates.unwrap().status, UpdatesStatus::Failed);
    assert!(overview.notes.iter().all(|n| n.update.is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn packs_get_their_pages_and_an_update_check() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    let url = w.server.file("pk-1.zip", b"pk-1");
    w.server.publish(
        json!({"id": "PACK", "slug": "faithful", "title": "Faithful", "project_type": "resourcepack"}),
        vec![json!({"id": "pk-1", "project_id": "PACK", "version_number": "1", "game_versions": ["1.21.1"],
                    "loaders": ["minecraft"], "date_published": "2026-09-01T00:00:00Z",
                    "files": [file_entry(&url, "pk-1.zip", b"pk-1", true)]})],
    );
    install(&w, InstallArgs::new(&key, ContentKind::ResourcePacks, "PACK", "faithful", "Faithful"))
        .await
        .unwrap();
    let overview = w.service.overview(&args(&key, ContentKind::ResourcePacks)).await.unwrap();
    assert!(overview.updates.is_some(), "packs are checked too");
    assert_eq!(overview.notes[0].url.as_deref(), Some("https://modrinth.com/resourcepack/faithful"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_replaces_the_file_keeps_a_backup_and_says_updated() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_mod(&w, &key, "SODIUM", "sod-1", Some("sod-2")).await;
    w.server.offer(b"sod-1", &version_of(&w, "sod-2"));
    let overview = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    let update = overview.notes[0].update_args(&key, ContentKind::Mods, "Sodium").unwrap();
    install(&w, update).await.unwrap();
    assert!(dir.join("mods/sod-2.jar").is_file() && !dir.join("mods/sod-1.jar").exists());
    assert!(dir.join("mods/.backups/sod-1.jar.backup").is_file(), "the old file is backed up");
    assert_eq!(w.said.keys().last().map(String::as_str), Some("mod_updated"));
    let after = w.service.overview(&args(&key, ContentKind::Mods)).await.unwrap();
    assert!(after.notes.iter().all(|n| n.update.is_none()), "{:?}", after.notes);
}

/// Installs pack version `id` of `project` (`kind`), with `newer` published after it and offered
/// as its update.
async fn with_pack(
    w: &World,
    key: &str,
    kind: ContentKind,
    project: &str,
    id: &str,
    newer: &str,
    loaders: &[&str],
) {
    let next = pack_release(&w.server, project, newer, loaders, 9);
    w.server.publish(
        json!({"id": project, "slug": project, "title": project, "project_type": "resourcepack"}),
        vec![next.clone(), pack_release(&w.server, project, id, loaders, 1)],
    );
    let args =
        InstallArgs { version_id: Some(id.into()), ..InstallArgs::new(key, kind, project, "", project) };
    install(w, args).await.unwrap();
    w.server.offer(id.as_bytes(), &next);
}

async fn update(w: &World, key: &str, kind: ContentKind, project: &str, to: &str) {
    let args =
        InstallArgs { version_id: Some(to.into()), ..InstallArgs::new(key, kind, project, "", project) };
    install(w, args).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn packs_and_shaders_are_checked_for_updates() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_pack(&w, &key, ContentKind::ResourcePacks, "FAITH", "faith-1", "faith-2", &["minecraft"]).await;
    let overview = w.service.overview(&args(&key, ContentKind::ResourcePacks)).await.unwrap();
    assert_eq!(overview.notes[0].update.as_ref().map(|u| u.version_id.as_str()), Some("faith-2"));
    assert_eq!(overview.updates.map(|s| s.status), Some(UpdatesStatus::Available));
    let body = w.server.data.lock().unwrap().update_bodies[0].clone();
    assert_eq!(body["loaders"], json!(["minecraft"]), "resource packs are Minecraft's");
}

#[tokio::test(flavor = "multi_thread")]
async fn shaders_are_asked_for_iris_versions() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_pack(&w, &key, ContentKind::ShaderPacks, "BSL", "bsl-1", "bsl-2", &["iris", "optifine"]).await;
    let overview = w.service.overview(&args(&key, ContentKind::ShaderPacks)).await.unwrap();
    assert_eq!(overview.notes[0].update.as_ref().map(|u| u.version_id.as_str()), Some("bsl-2"));
    let body = w.server.data.lock().unwrap().update_bodies[0].clone();
    assert_eq!(body["loaders"], json!(["iris"]), "the launcher's shaders run on Iris");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_updated_resource_pack_stays_active() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_pack(&w, &key, ContentKind::ResourcePacks, "FAITH", "faith-1", "faith-2", &["minecraft"]).await;
    std::fs::write(
        dir.join("options.txt"),
        "lang:uk_ua
resourcePacks:[\"vanilla\",\"file/faith-1.zip\"]
",
    )
    .unwrap();
    update(&w, &key, ContentKind::ResourcePacks, "FAITH", "faith-2").await;
    let options = std::fs::read_to_string(dir.join("options.txt")).unwrap();
    assert!(options.contains(r#"resourcePacks:["vanilla","file/faith-2.zip"]"#), "{options}");
    assert!(options.contains("lang:uk_ua"), "other lines stay");
    assert!(
        !dir.join("resourcepacks/faith-1.zip").exists() && dir.join("resourcepacks/faith-2.zip").exists()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_updated_shader_stays_selected() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_pack(&w, &key, ContentKind::ShaderPacks, "BSL", "bsl-1", "bsl-2", &["iris"]).await;
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(
        dir.join("config/iris.properties"),
        "enableShaders=true
shaderPack=bsl-1.zip
",
    )
    .unwrap();
    update(&w, &key, ContentKind::ShaderPacks, "BSL", "bsl-2").await;
    let properties = std::fs::read_to_string(dir.join("config/iris.properties")).unwrap();
    assert!(
        properties.contains("shaderPack=bsl-2.zip") && properties.contains("enableShaders=true"),
        "{properties}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_inactive_pack_stays_inactive_after_an_update() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "NeoForge", NEOFORGE);
    with_pack(&w, &key, ContentKind::ResourcePacks, "FAITH", "faith-1", "faith-2", &["minecraft"]).await;
    let listed = "resourcePacks:[\"vanilla\",\"file/other.zip\"]
";
    std::fs::write(dir.join("options.txt"), listed).unwrap();
    update(&w, &key, ContentKind::ResourcePacks, "FAITH", "faith-2").await;
    assert_eq!(std::fs::read_to_string(dir.join("options.txt")).unwrap(), listed, "nothing switched on");
}
