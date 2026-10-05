mod support;

use std::fs;

use launcher_core::launch::options::game_dir;
use launcher_shared::ErrorCode;
use launcher_shared::provider::{
    HeldAlternative, HeldFile, PackArgs, PackInstallArgs, PackUpdateArgs, PacksArgs, held_files,
};
use serde_json::{Value, json};
use support::*;

const PACK: u64 = 300;

/// Publishes mods Create (100) and Faithful (200, a resource pack) with their files.
fn publish_content(w: &World) {
    let create = file_json(
        &w.server,
        1001,
        100,
        "create-1.jar",
        &mod_jar("create"),
        &["1.20.1", "Forge"],
        json!([]),
        false,
    );
    let create_2 = file_json(
        &w.server,
        1002,
        100,
        "create-2.jar",
        &mod_jar("create"),
        &["1.20.1", "Forge"],
        json!([]),
        false,
    );
    w.server.publish(mod_json(100, "Create", MODS, 10), vec![create_2, create]);
    let faithful =
        file_json(&w.server, 2001, 200, "faithful.zip", b"faithful", &["1.20.1"], json!([]), false);
    w.server.publish(mod_json(200, "Faithful", RESOURCE_PACKS, 5), vec![faithful]);
}

fn args(version: u64) -> PackInstallArgs {
    PackInstallArgs {
        project_id: PACK.to_string(),
        version_id: version.to_string(),
        name: "Велика".into(),
        icon_url: None,
        replace_held: Vec::new(),
        skip_held: false,
    }
}

/// The build folders (the launcher's own `.launcher-*` entries left out).
fn folders(w: &World) -> usize {
    fs::read_dir(w.versions.games_dir())
        .map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
        .unwrap_or(0)
}

/// Version 3001 of the pack (Create 1, Faithful, a config) installed as a build.
async fn installed(w: &World) -> (String, std::path::PathBuf) {
    publish_content(w);
    let zip = pack_zip(&[(100, 1001), (200, 2001)], &[("overrides/config/a.toml", b"one")], "1.0");
    publish_pack(&w.server, PACK, vec![pack_file(&w.server, 3001, PACK, &zip, "1.20.1", "Forge")]);
    let done = w.service.install_pack(&args(3001)).await.unwrap();
    let build = w.versions.get(&done.key).unwrap();
    (done.key, game_dir(&build, w.versions.minecraft_dir()))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_curseforge_modpack_becomes_a_ready_build() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    assert_eq!(fs::read(game.join("mods/create-1.jar")).unwrap(), mod_jar("create"));
    assert_eq!(
        fs::read(game.join("resourcepacks/faithful.zip")).unwrap(),
        b"faithful",
        "by its project's class"
    );
    assert_eq!(fs::read(game.join("config/a.toml")).unwrap(), b"one");
    assert!(!game.join(".launcher-curseforge-pack.zip").exists(), "the pack's zip goes");
    let build = w.versions.get(&key).unwrap();
    assert_eq!(build.name, "Велика");
    assert_eq!(
        (build.version.as_deref(), build.client.as_deref(), build.loader_version.as_deref()),
        (Some("1.20.1"), Some("Forge"), Some("47.2.0"))
    );
    assert_eq!(build.options["curseforgeProjectId"], json!("300"));
    assert_eq!(build.options["curseforgeVersionId"], json!("3001"));
    assert_eq!(build.description, "Big Pack does things");
    assert_eq!(w.components.asked.lock().unwrap().clone(), [build.loader.clone().unwrap()]);
    let record: Value =
        serde_json::from_slice(&fs::read(game.join(".launcher/curseforge-pack.json")).unwrap()).unwrap();
    assert_eq!(
        (record["project_id"].clone(), record["version_id"].clone(), record["version_number"].clone()),
        (json!("300"), json!("3001"), json!("3001"))
    );
    assert_eq!(
        record["managed_files"],
        json!(["mods/create-1.jar", "resourcepacks/faithful.zip", "config/a.toml"])
    );
    let downloads: Vec<Seen> =
        w.server.seen().into_iter().filter(|s| s.path.starts_with("/files/")).collect();
    assert_eq!(downloads.len(), 3, "the pack's zip and its two files");
    assert!(downloads.iter().all(|s| s.key.as_deref() == Some(TEST_KEY)), "CurseForge's files take the key");
    assert!(w.said.keys().contains(&"version_install_success".to_string()));
}

/// Publishes Held Mod (400), whose file 4001 CurseForge keeps from other apps, and version 3001
/// of the pack taking it with Create; the held jar's bytes.
fn pack_with_a_held_file(w: &World) -> Vec<u8> {
    publish_content(w);
    let jar = mod_jar("held");
    let held = file_json(&w.server, 4001, 400, "held.jar", &jar, &["1.20.1", "Forge"], json!([]), true);
    w.server.publish(mod_json(400, "Held Mod", MODS, 1), vec![held]);
    let zip = pack_zip(&[(100, 1001), (400, 4001)], &[], "1.0");
    publish_pack(&w.server, PACK, vec![pack_file(&w.server, 3001, PACK, &zip, "1.20.1", "Forge")]);
    jar
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_found_nowhere_is_named_to_download_by_hand_and_leaves_no_build() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    let e = w.service.install_pack(&args(3001)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ProviderFilesHeld);
    assert_eq!(
        held_files(&e),
        [HeldFile {
            title: "Held Mod".into(),
            file_name: "held.jar".into(),
            url: Some("https://www.curseforge.com/minecraft/mc-mods/held-mod/files/4001".into()),
            size: jar.len() as u64,
            sha1: sha1_hex(&jar),
            alternative: None,
            folder: "mods".into(),
        }]
    );
    assert_eq!(folders(&w), 0, "the claimed folder goes");
    assert!(w.versions.list().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn going_on_after_a_held_file_takes_the_pack_downloaded_before() {
    // The pack's zip (hundreds of megabytes) came down before the held file was found missing:
    // once the user has it, the next try reads the zip kept from the first.
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    let e = w.service.install_pack(&args(3001)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ProviderFilesHeld);
    fs::write(w.downloads.join("held.jar"), &jar).unwrap();
    w.service.install_pack(&args(3001)).await.unwrap();
    let zips = w.server.seen().into_iter().filter(|s| s.path.ends_with("/big-pack-3001.zip")).count();
    assert_eq!(zips, 1, "the pack's zip came down once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_the_user_downloaded_is_taken_from_their_downloads() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    fs::write(w.downloads.join("held (1).jar"), &jar).unwrap();
    let done = w.service.install_pack(&args(3001)).await.unwrap();
    let build = w.versions.get(&done.key).unwrap();
    let game = game_dir(&build, w.versions.minecraft_dir());
    assert_eq!(fs::read(game.join("mods/held.jar")).unwrap(), jar);
    assert!(w.downloads.join("held (1).jar").exists(), "the user's copy stays theirs");
    assert!(!w.server.seen().iter().any(|s| s.path == "/v2/version_files"), "no need to ask Modrinth");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_modrinth_has_comes_from_modrinth_without_the_key() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    w.server.on_modrinth("held.jar", &jar);
    let done = w.service.install_pack(&args(3001)).await.unwrap();
    let build = w.versions.get(&done.key).unwrap();
    assert_eq!(fs::read(game_dir(&build, w.versions.minecraft_dir()).join("mods/held.jar")).unwrap(), jar);
    let from_modrinth = w.server.seen().into_iter().find(|s| s.path == "/files/mr-held.jar").unwrap();
    assert_eq!(from_modrinth.key, None, "the key goes to CurseForge only");
    let asked = w.server.seen().into_iter().find(|s| s.path == "/v2/version_files").unwrap();
    assert_eq!(asked.body.unwrap(), json!({"hashes": [sha1_hex(&jar)], "algorithm": "sha1"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_names_the_same_mod_modrinth_has_for_the_pack_s_game() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    w.server.modrinth_project("held-mod", "Held Mod", "2.0", "held-mr-2.jar", b"held from modrinth");
    let e = w.service.install_pack(&args(3001)).await.unwrap_err();
    let held = held_files(&e);
    assert_eq!(held[0].sha1, sha1_hex(&jar));
    assert_eq!(
        held[0].alternative,
        Some(HeldAlternative {
            provider: "Modrinth".into(),
            title: "Held Mod".into(),
            version: "2.0".into(),
            file_name: "held-mr-2.jar".into(),
            url: Some("https://modrinth.com/mod/held-mod".into()),
        })
    );
    let asked = w.server.seen().into_iter().find(|s| s.path == "/v2/search").unwrap();
    let facets = asked.query.iter().find(|(k, _)| k == "facets").map(|(_, v)| v.clone()).unwrap();
    assert!(
        facets.contains("versions:1.20.1") && facets.contains("categories:forge"),
        "for the pack's game and loader: {facets}"
    );
    let versions = w.server.seen().into_iter().find(|s| s.path == "/v2/project/MR-held-mod/version").unwrap();
    assert!(versions.query.iter().any(|(k, v)| k == "game_versions" && v.contains("1.20.1")), "{versions:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_alike_name_is_no_alternative() {
    let w = world().await;
    pack_with_a_held_file(&w);
    w.server.modrinth_project("held-mod-extras", "Held Mod Extras", "1.0", "extras.jar", b"other");
    let e = w.service.install_pack(&args(3001)).await.unwrap_err();
    assert_eq!(held_files(&e)[0].alternative, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_is_taken_from_its_alternative_when_asked() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    w.server.modrinth_project("held-mod", "Held Mod", "2.0", "held-mr-2.jar", b"held from modrinth");
    let mut with = args(3001);
    with.replace_held = vec![sha1_hex(&jar)];
    let done = w.service.install_pack(&with).await.unwrap();
    assert!(done.skipped.is_empty());
    let build = w.versions.get(&done.key).unwrap();
    let game = game_dir(&build, w.versions.minecraft_dir());
    assert_eq!(fs::read(game.join("mods/held-mr-2.jar")).unwrap(), b"held from modrinth");
    assert!(!game.join("mods/held.jar").exists());
    let from_modrinth = w.server.seen().into_iter().find(|s| s.path == "/files/mr-held-mr-2.jar").unwrap();
    assert_eq!(from_modrinth.key, None, "the key goes to CurseForge only");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skipped_held_file_leaves_the_build_without_it_and_is_named() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    let mut without = args(3001);
    without.skip_held = true;
    let done = w.service.install_pack(&without).await.unwrap();
    assert_eq!(done.skipped.len(), 1);
    assert_eq!(
        (done.skipped[0].file_name.as_str(), done.skipped[0].sha1.clone()),
        ("held.jar", sha1_hex(&jar))
    );
    let build = w.versions.get(&done.key).unwrap();
    let game = game_dir(&build, w.versions.minecraft_dir());
    assert!(game.join("mods/create-1.jar").exists(), "the rest is installed");
    assert!(!game.join("mods/held.jar").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_of_another_project_is_refused() {
    let w = world().await;
    publish_content(&w);
    let zip = pack_zip(&[(100, 1001)], &[], "1.0");
    publish_pack(&w.server, PACK, vec![pack_file(&w.server, 3001, PACK, &zip, "1.20.1", "Forge")]);
    let e =
        w.service.install_pack(&PackInstallArgs { version_id: "1001".into(), ..args(0) }).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput, "Create's jar is no file of the pack");
    assert_eq!(folders(&w), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn modpacks_are_searched_twenty_a_page_and_their_versions_listed_newest_first() {
    let w = world().await;
    publish_content(&w);
    let zip = pack_zip(&[], &[], "1.0");
    let mut server_pack = pack_file(&w.server, 3003, PACK, &zip, "1.20.1", "Forge");
    server_pack["isServerPack"] = json!(true);
    let mut newer = pack_file(&w.server, 3002, PACK, &zip, "1.20.1", "Forge");
    newer["fileDate"] = json!("2026-09-30T10:00:00Z");
    let mut older = pack_file(&w.server, 3001, PACK, &zip, "1.20.1", "Forge");
    older["fileDate"] = json!("2026-09-01T10:00:00Z");
    publish_pack(&w.server, PACK, vec![older, server_pack, newer]);
    let page = w.service.packs(&PacksArgs { query: "big".into(), offset: 0 }).await.unwrap();
    assert_eq!(page.hits.iter().map(|h| h.project_id.as_str()).collect::<Vec<_>>(), ["300"]);
    let search = w.server.seen().into_iter().find(|s| s.path == "/v1/mods/search").unwrap();
    for pair in [("classId", "4471"), ("pageSize", "20"), ("searchFilter", "big")] {
        assert!(search.query.contains(&(pair.0.to_string(), pair.1.to_string())), "{pair:?}");
    }
    assert!(!search.query.iter().any(|(k, _)| k == "gameVersion" || k == "modLoaderType"));
    let versions = w.service.pack_versions(&PackArgs { project_id: PACK.to_string() }).await.unwrap();
    let listed: Vec<(&str, &str)> =
        versions.iter().map(|v| (v.id.as_str(), v.version_number.as_str())).collect();
    assert_eq!(listed, [("3002", "3002"), ("3001", "3001")], "no server pack; the pack's name left out");
    assert_eq!(
        (versions[0].game_versions.clone(), versions[0].loaders.clone()),
        (vec!["1.20.1".to_string()], vec!["forge".to_string()])
    );
    let e = w.service.pack_versions(&PackArgs { project_id: "100".into() }).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput, "a mod is no pack");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_swaps_the_pack_s_files_and_keeps_the_player_s() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("mods/mine.jar"), b"mine").unwrap();
    let zip = pack_zip(&[(100, 1002)], &[("overrides/config/a.toml", b"two")], "2.0");
    let mut v2 = pack_file(&w.server, 3002, PACK, &zip, "1.20.1", "Forge");
    v2["fileDate"] = json!("2026-09-30T10:00:00Z");
    let v1 = w.server.data.lock().unwrap().files[&PACK][0].clone();
    publish_pack(&w.server, PACK, vec![v2, v1]);
    let listed = w.service.modpack_builds().await.unwrap();
    let mine = listed.iter().find(|b| b.key == key).unwrap();
    assert_eq!(
        (mine.version_id.as_str(), mine.newest.as_ref().map(|v| v.id.as_str())),
        ("3001", Some("3002"))
    );
    let updated = w
        .service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "3002".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(updated.version_number, "3002");
    assert_eq!(fs::read(game.join("mods/create-2.jar")).unwrap(), mod_jar("create"));
    assert!(!game.join("mods/create-1.jar").exists() && !game.join("resourcepacks/faithful.zip").exists());
    assert_eq!(fs::read(game.join("config/a.toml")).unwrap(), b"two");
    assert_eq!(fs::read(game.join("mods/mine.jar")).unwrap(), b"mine", "the player's mod stays");
    assert_eq!(w.versions.get(&key).unwrap().options["curseforgeVersionId"], json!("3002"));
    assert!(
        w.service.modpack_builds().await.unwrap().iter().find(|b| b.key == key).unwrap().newest.is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_mod_the_player_has_from_curseforge_is_not_added_again() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // The player's own copy of a mod CurseForge knows (by its fingerprint), under another name.
    let jei_1 = mod_jar("jei");
    let jei_file =
        file_json(&w.server, 5001, 500, "jei-1.jar", &jei_1, &["1.20.1", "Forge"], json!([]), false);
    let jei_2 =
        file_json(&w.server, 5002, 500, "jei-2.jar", b"jei two", &["1.20.1", "Forge"], json!([]), false);
    w.server.publish(mod_json(500, "JEI", MODS, 9), vec![jei_2, jei_file]);
    fs::write(game.join("mods/my-jei.jar"), &jei_1).unwrap();
    let zip = pack_zip(&[(100, 1001), (200, 2001), (500, 5002)], &[], "2.0");
    let v1 = w.server.data.lock().unwrap().files[&PACK][0].clone();
    publish_pack(&w.server, PACK, vec![pack_file(&w.server, 3002, PACK, &zip, "1.20.1", "Forge"), v1]);
    w.service
        .update_pack(&PackUpdateArgs { key, version_id: "3002".into(), ..Default::default() })
        .await
        .unwrap();
    assert!(!game.join("mods/jei-2.jar").exists(), "the player has JEI already");
    assert_eq!(fs::read(game.join("mods/my-jei.jar")).unwrap(), jei_1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_of_another_provider_s_pack_is_not_curseforge_s() {
    let w = world().await;
    let (key, game) = build(&w, "Чужа", "Forge", "1.20.1-forge-47.2.0");
    fs::create_dir_all(game.join(".launcher")).unwrap();
    fs::write(game.join(".launcher/modrinth-pack.json"), b"{}").unwrap();
    assert!(w.service.modpack_builds().await.unwrap().is_empty());
    let e = w
        .service
        .update_pack(&PackUpdateArgs { key, version_id: "3001".into(), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_keeps_a_held_file_the_build_has_without_asking_for_it_again() {
    let w = world().await;
    let jar = pack_with_a_held_file(&w);
    fs::write(w.downloads.join("held.jar"), &jar).unwrap();
    let done = w.service.install_pack(&args(3001)).await.unwrap();
    // The user cleaned up their Downloads since.
    fs::remove_file(w.downloads.join("held.jar")).unwrap();
    let zip = pack_zip(&[(100, 1002), (400, 4001)], &[], "2.0");
    let v1 = w.server.data.lock().unwrap().files[&PACK][0].clone();
    publish_pack(&w.server, PACK, vec![pack_file(&w.server, 3002, PACK, &zip, "1.20.1", "Forge"), v1]);
    w.service
        .update_pack(&PackUpdateArgs {
            key: done.key.clone(),
            version_id: "3002".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let game = game_dir(&w.versions.get(&done.key).unwrap(), w.versions.minecraft_dir());
    assert_eq!(fs::read(game.join("mods/held.jar")).unwrap(), jar);
    assert!(game.join("mods/create-2.jar").exists() && !game.join("mods/create-1.jar").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_curseforge_pack_s_files_are_curseforge_s_own() {
    let w = world().await;
    let (key, _) = installed(&w).await;
    let overview = w
        .service
        .overview(&launcher_shared::provider::OverviewArgs {
            key,
            kind: launcher_shared::ContentKind::Mods,
            check_updates: false,
        })
        .await
        .unwrap();
    let marks: Vec<(String, bool)> = overview.notes.into_iter().map(|n| (n.file, n.installed)).collect();
    assert_eq!(marks, [("create-1.jar".to_string(), true)], "the pack put it there");
}
