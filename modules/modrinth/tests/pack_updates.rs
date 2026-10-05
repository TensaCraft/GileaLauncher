mod support;

use std::fs;

use launcher_core::launch::options::game_dir;
use launcher_shared::ErrorCode;
use launcher_shared::provider::{PackInstallArgs, PackUpdateArgs};
use serde_json::{Value, json};
use support::*;

/// Installs version `v1` of pack SPEEDY (sodium 1 + a config) as build "Швидка".
async fn installed(w: &World) -> (String, std::path::PathBuf) {
    let v1 = mrpack_of(
        vec![pack_mod(&w.server, "sodium-1.jar", b"sodium 1"), pack_mod(&w.server, "old.jar", b"old")],
        &[("overrides/config/a.txt", b"one")],
        "1.0",
    );
    publish_pack(&w.server, "SPEEDY", "sp-1", &v1);
    let done = w
        .service
        .install_pack(&PackInstallArgs {
            project_id: "SPEEDY".into(),
            version_id: "sp-1".into(),
            name: "Швидка".into(),
            icon_url: None,
            ..Default::default()
        })
        .await
        .unwrap();
    let build = w.versions.get(&done.key).unwrap();
    (done.key, game_dir(&build, w.versions.minecraft_dir()))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_swaps_the_pack_s_files_and_keeps_the_user_s() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("mods/mine.jar"), b"mine").unwrap();
    fs::create_dir_all(game.join("saves/world")).unwrap();
    fs::write(game.join("saves/world/level.dat"), b"world").unwrap();
    let v2 = mrpack_on(
        vec![pack_mod(&w.server, "sodium-2.jar", b"sodium 2")],
        &[("overrides/config/a.txt", b"two")],
        "2.0",
        "1.21.1",
        "0.17.0",
    );
    publish_pack(&w.server, "SPEEDY", "sp-2", &v2);
    let listed = w.service.modpack_builds().await.unwrap();
    let mine = listed.iter().find(|b| b.key == key).unwrap();
    assert_eq!(
        (mine.version_id.as_str(), mine.newest.as_ref().map(|v| v.id.as_str())),
        ("sp-1", Some("sp-2"))
    );
    let updated = w
        .service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-2".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(updated.version_number, "sp-2");
    assert_eq!(fs::read(game.join("mods/sodium-2.jar")).unwrap(), b"sodium 2");
    assert!(
        !game.join("mods/sodium-1.jar").exists() && !game.join("mods/old.jar").exists(),
        "the pack's old files go"
    );
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"two");
    assert_eq!(fs::read(game.join("mods/mine.jar")).unwrap(), b"mine", "the user's mod stays");
    assert_eq!(fs::read(game.join("saves/world/level.dat")).unwrap(), b"world");
    let build = w.versions.get(&key).unwrap();
    assert_eq!(build.options["modrinthVersionId"], json!("sp-2"));
    assert_eq!(
        (build.loader.as_deref(), build.loader_version.as_deref()),
        (Some("fabric-loader-0.17.0-1.21.1"), Some("0.17.0")),
        "the build runs the new version's loader"
    );
    let record: Value =
        serde_json::from_slice(&fs::read(game.join(".launcher/modrinth-pack.json")).unwrap()).unwrap();
    assert_eq!((record["schema_version"].clone(), record["version_id"].clone()), (json!(3), json!("sp-2")));
    assert!(
        w.service.modpack_builds().await.unwrap().iter().find(|b| b.key == key).unwrap().newest.is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_update_offered_keeps_the_build_s_minecraft_and_loader() {
    let w = world().await;
    let (key, _) = installed(&w).await;
    let newest = |w: &World| {
        let w = w.service.clone();
        let key = key.clone();
        async move {
            let listed = w.modpack_builds().await.unwrap();
            listed.into_iter().find(|b| b.key == key).unwrap().newest.map(|v| v.id)
        }
    };
    // Newer, but for another Minecraft or another loader: not offered.
    publish_pack_for(&w.server, "SPEEDY", "sp-2", b"x", &["26.3"], &["fabric"]);
    publish_pack_for(&w.server, "SPEEDY", "sp-3", b"x", &["1.21.1"], &["quilt"]);
    assert_eq!(newest(&w).await, None);
    // The newest for 1.21.1 on Fabric is offered, though a version for 26.3 came after it.
    publish_pack_for(&w.server, "SPEEDY", "sp-4", b"x", &["1.21", "1.21.1"], &["fabric"]);
    publish_pack_for(&w.server, "SPEEDY", "sp-5", b"x", &["26.3"], &["fabric"]);
    assert_eq!(newest(&w).await.as_deref(), Some("sp-4"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_update_leaves_the_build_as_it_was() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    let mut broken = pack_mod(&w.server, "sodium-2.jar", b"sodium 2");
    broken["hashes"] = json!({"sha512": sha512(b"something else")});
    publish_pack(
        &w.server,
        "SPEEDY",
        "sp-2",
        &mrpack_on(vec![broken], &[("overrides/config/a.txt", b"two")], "2.0", "1.21.1", "0.17.0"),
    );
    assert!(
        w.service
            .update_pack(&PackUpdateArgs {
                key: key.clone(),
                version_id: "sp-2".into(),
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert_eq!(fs::read(game.join("mods/sodium-1.jar")).unwrap(), b"sodium 1");
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"one");
    assert_eq!(w.versions.get(&key).unwrap().options["modrinthVersionId"], json!("sp-1"));
    assert_eq!(w.versions.get(&key).unwrap().loader_version.as_deref(), Some("0.16.9"), "the loader stays");
    assert!(
        !game.join(".launcher-sync").exists()
            || fs::read_dir(game.join(".launcher-sync")).unwrap().next().is_none(),
        "no work left"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_game_takes_an_update_and_another_pack_is_refused() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    publish_pack(&w.server, "SPEEDY", "sp-2", &mrpack_of(vec![], &[], "2.0"));
    w.running.store(true, std::sync::atomic::Ordering::SeqCst);
    let updated = w
        .service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-2".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(updated.version_number, "sp-2");
    assert!(!game.join("mods/sodium-1.jar").exists(), "the old version's files go");
    w.running.store(false, std::sync::atomic::Ordering::SeqCst);
    publish_pack(&w.server, "OTHER", "ot-1", &mrpack_of(vec![], &[], "1.0"));
    let other = w
        .service
        .update_pack(&PackUpdateArgs { key, version_id: "ot-1".into(), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(other.code, ErrorCode::InvalidInput, "another project's version");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_installed_with_a_first_record_updates_too() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // As 4.7 wrote it: no loader fields.
    fs::write(
        game.join(".launcher/modrinth-pack.json"),
        json!({"schema_version": 1, "project_id": "SPEEDY", "version_id": "sp-1",
               "managed_files": ["mods/sodium-1.jar", "mods/old.jar", "config/a.txt"]})
        .to_string(),
    )
    .unwrap();
    publish_pack(
        &w.server,
        "SPEEDY",
        "sp-2",
        &mrpack_on(vec![pack_mod(&w.server, "sodium-2.jar", b"s2")], &[], "2.0", "1.21.1", "0.17.0"),
    );
    w.service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-2".into(), ..Default::default() })
        .await
        .unwrap();
    assert!(game.join("mods/sodium-2.jar").is_file() && !game.join("mods/old.jar").exists());
    assert_eq!(w.versions.get(&key).unwrap().loader_version.as_deref(), Some("0.17.0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_commit_is_finished_by_the_next_update() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // A crash after sp-2's files were swapped in, before the build took it: the record on disk
    // says sp-2, the journal stopped in its commit step, the build still says sp-1.
    let mut record: Value =
        serde_json::from_slice(&fs::read(game.join(".launcher/modrinth-pack.json")).unwrap()).unwrap();
    record["version_id"] = json!("sp-2");
    record["version_number"] = json!("sp-2");
    record["loader"] = json!("fabric-loader-0.17.0-1.21.1");
    record["loader_version"] = json!("0.17.0");
    fs::write(game.join(".launcher/modrinth-pack.json"), record.to_string()).unwrap();
    fs::write(
        game.join(".launcher-pack-sync.json"),
        json!({"schema_version": 2, "status": "committing", "operation": "modrinth-pack-update",
               "commit_key": format!("modrinth-pack:{key}"), "transaction_id": "0123456789abcdef0123456789abcdef",
               "entries": []})
        .to_string(),
    )
    .unwrap();
    // The next update resumes that commit first — and then fails on its own (a bad file), rolling
    // back only its own change.
    let mut broken = pack_mod(&w.server, "sodium-3.jar", b"sodium 3");
    broken["hashes"] = json!({"sha512": sha512(b"something else")});
    publish_pack(&w.server, "SPEEDY", "sp-3", &mrpack_of(vec![broken], &[], "3.0"));
    assert!(
        w.service
            .update_pack(&PackUpdateArgs {
                key: key.clone(),
                version_id: "sp-3".into(),
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert_eq!(
        w.versions.get(&key).unwrap().options["modrinthVersionId"],
        json!("sp-2"),
        "the commit finished"
    );
    assert_eq!(w.versions.get(&key).unwrap().loader_version.as_deref(), Some("0.17.0"));
}

/// The state a crash leaves when an update to sp-2 stopped in its commit step (`committing`) or
/// while its files were swapped in (`applying`): sp-2's record and sodium-2 in place, sp-1's
/// record in the transaction's backup.
fn cut_short(game: &std::path::Path, key: &str, status: &str) {
    let tid = "0123456789abcdef0123456789abcdef";
    let record = game.join(".launcher/modrinth-pack.json");
    let backup = game.join(format!(".launcher-sync/{tid}/backup/.launcher/modrinth-pack.json"));
    fs::create_dir_all(backup.parent().unwrap()).unwrap();
    fs::copy(&record, &backup).unwrap();
    fs::write(
        &record,
        json!({"schema_version": 2, "project_id": "SPEEDY", "version_id": "sp-2", "version_number": "2.0",
               "minecraft": "1.21.1", "loader": "fabric-loader-0.17.0-1.21.1", "loader_version": "0.17.0",
               "client": "Fabric", "managed_files": ["mods/sodium-2.jar"]})
        .to_string(),
    )
    .unwrap();
    fs::write(game.join("mods/sodium-2.jar"), b"sodium 2").unwrap();
    fs::write(
        game.join(".launcher-pack-sync.json"),
        json!({"schema_version": 2, "status": status, "operation": "modrinth-pack-update",
               "commit_key": format!("modrinth-pack:{key}"), "transaction_id": tid,
               "entries": [{"path": ".launcher/modrinth-pack.json", "state": "applied", "had_original": true},
                           {"path": "mods/sodium-2.jar", "state": "applied", "had_original": false}]})
        .to_string(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_cut_short_is_finished_when_the_builds_are_listed() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    publish_pack(&w.server, "SPEEDY", "sp-2", &mrpack_of(vec![], &[], "2.0"));
    cut_short(&game, &key, "committing");
    // sp-2 is the newest: no update is offered, so opening the Builds page must finish it.
    let listed = w.service.modpack_builds().await.unwrap();
    assert!(listed.iter().find(|b| b.key == key).unwrap().newest.is_none());
    let build = w.versions.get(&key).unwrap();
    assert_eq!(
        (build.options["modrinthVersionId"].clone(), build.loader_version.as_deref()),
        (json!("sp-2"), Some("0.17.0")),
        "the build took sp-2 and its loader"
    );
    assert!(!game.join(".launcher-sync").join("0123456789abcdef0123456789abcdef").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_swap_cut_short_is_undone_before_the_next_update_reads_the_record() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    cut_short(&game, &key, "applying");
    let v3 = mrpack_of(vec![pack_mod(&w.server, "sodium-3.jar", b"s3")], &[], "3.0");
    publish_pack(&w.server, "SPEEDY", "sp-3", &v3);
    w.service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-3".into(), ..Default::default() })
        .await
        .unwrap();
    assert!(game.join("mods/sodium-3.jar").is_file());
    for gone in ["mods/sodium-1.jar", "mods/old.jar", "mods/sodium-2.jar"] {
        assert!(!game.join(gone).exists(), "{gone}: sp-1's files go, as sp-2's swap was undone first");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_keeps_the_java_the_user_chose_while_minecraft_stays() {
    let w = world().await;
    let (key, _) = installed(&w).await;
    let mut build = w.versions.get(&key).unwrap();
    assert_eq!(build.options["executablePath"], json!("java-for-1.21.1"));
    build.options.insert("executablePath".into(), json!("C:/my/java.exe"));
    w.versions.save(&mut build).unwrap();
    publish_pack(&w.server, "SPEEDY", "sp-2", &mrpack_on(vec![], &[], "2.0", "1.21.1", "0.17.0"));
    w.service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-2".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(w.versions.get(&key).unwrap().options["executablePath"], json!("C:/my/java.exe"));
    // Another Minecraft may need another Java: the launcher's pick.
    publish_pack(&w.server, "SPEEDY", "sp-3", &mrpack_on(vec![], &[], "3.0", "1.21.4", "0.17.0"));
    w.service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-3".into(), ..Default::default() })
        .await
        .unwrap();
    let build = w.versions.get(&key).unwrap();
    assert_eq!(build.version.as_deref(), Some("1.21.4"));
    assert_eq!(build.options["executablePath"], json!("java-for-1.21.4"));
}

/// Publishes version `vid` of pack SPEEDY with `files` and `entries`, and updates build `key` to it.
async fn update_to(
    w: &World,
    key: &str,
    vid: &str,
    files: Vec<Value>,
    entries: &[(&str, &[u8])],
) -> launcher_shared::provider::PackUpdated {
    publish_pack(&w.server, "SPEEDY", vid, &mrpack_of(files, entries, vid));
    w.service
        .update_pack(&PackUpdateArgs { key: key.into(), version_id: vid.into(), ..Default::default() })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_update_keeps_the_players_changed_files() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("config/a.txt"), b"mine").unwrap();
    // The new version brings the file as the old one did: the player's change is all there is.
    let updated = update_to(&w, &key, "sp-2", vec![], &[("overrides/config/a.txt", b"one")]).await;
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"mine", "the player's change stays");
    assert!(updated.backups.is_empty());
    // A version that no longer ships it leaves it too.
    update_to(&w, &key, "sp-3", vec![], &[]).await;
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"mine");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_the_pack_changes_too_is_replaced_after_the_players_copy_is_saved() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("config/a.txt"), b"mine").unwrap();
    let updated = update_to(&w, &key, "sp-2", vec![], &[("overrides/config/a.txt", b"two")]).await;
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"two", "the pack's new settings apply");
    let [saved] = updated.backups.as_slice() else { panic!("one copy saved: {:?}", updated.backups) };
    assert!(saved.starts_with(".launcher/pack-backups/") && saved.ends_with("/config/a.txt"), "{saved}");
    assert_eq!(fs::read(game.join(saved)).unwrap(), b"mine", "the player's copy is kept");
    assert!(w.said.keys().iter().any(|k| k == "modpack_updated_backups"), "{:?}", w.said.keys());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_a_mod_wrote_gives_way_to_the_packs_after_it_is_saved() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // Written by a mod on its first run: the old version did not ship it.
    fs::write(game.join("config/b.toml"), b"defaults").unwrap();
    let updated = update_to(&w, &key, "sp-2", vec![], &[("overrides/config/b.toml", b"tuned")]).await;
    assert_eq!(fs::read(game.join("config/b.toml")).unwrap(), b"tuned");
    assert_eq!(updated.backups.len(), 1);
    assert_eq!(fs::read(game.join(&updated.backups[0])).unwrap(), b"defaults");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_already_as_the_new_version_has_it_is_the_packs_again() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("config/a.txt"), b"two").unwrap();
    let updated = update_to(&w, &key, "sp-2", vec![], &[("overrides/config/a.txt", b"two")]).await;
    assert!(updated.backups.is_empty(), "nothing to save: it is the new version's");
    // The pack owns it again: the next version replaces it without a copy.
    let updated = update_to(&w, &key, "sp-3", vec![], &[("overrides/config/a.txt", b"three")]).await;
    assert_eq!(fs::read(game.join("config/a.txt")).unwrap(), b"three");
    assert!(updated.backups.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_update_never_touches_worlds() {
    let w = world().await;
    let v1 = mrpack_of(vec![], &[("overrides/saves/Sky/level.dat", b"v1")], "1.0");
    publish_pack(&w.server, "SPEEDY", "sp-1", &v1);
    let done = w
        .service
        .install_pack(&PackInstallArgs {
            project_id: "SPEEDY".into(),
            version_id: "sp-1".into(),
            name: "Небо".into(),
            icon_url: None,
            ..Default::default()
        })
        .await
        .unwrap();
    let game = game_dir(&w.versions.get(&done.key).unwrap(), w.versions.minecraft_dir());
    assert_eq!(fs::read(game.join("saves/Sky/level.dat")).unwrap(), b"v1", "a new world comes with the pack");
    update_to(&w, &done.key, "sp-2", vec![], &[("overrides/saves/Sky/level.dat", b"v2")]).await;
    assert_eq!(fs::read(game.join("saves/Sky/level.dat")).unwrap(), b"v1", "a world there is the player's");
    update_to(&w, &done.key, "sp-3", vec![], &[]).await;
    assert!(game.join("saves/Sky/level.dat").is_file(), "a world is never deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_update_does_not_add_a_second_copy_of_a_mod() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // The player updated the pack's Sodium from the Content tab: its file is the player's now.
    fs::remove_file(game.join("mods/sodium-1.jar")).unwrap();
    fs::write(game.join("mods/sodium-new.jar"), b"sodium new").unwrap();
    fs::write(
        game.join(".launcher/modrinth-content.json"),
        json!({"files": {"mods/sodium-new.jar": {"kind": "mods", "filename": "sodium-new.jar", "project_id": "SOD",
            "version_id": "sod-9", "version_number": "9", "hash_algorithm": "sha512", "file_hash": sha512(b"sodium new")}}})
        .to_string(),
    )
    .unwrap();
    update_to(&w, &key, "sp-2", vec![pack_mod_of(&w.server, "SOD", "sodium-2.jar", b"sodium 2")], &[]).await;
    assert!(!game.join("mods/sodium-2.jar").exists(), "the player's Sodium stays the only one");
    assert!(game.join("mods/sodium-new.jar").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn unchanged_pack_files_are_not_downloaded_again() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    let asked = |w: &World| w.server.seen().iter().filter(|(p, _)| p.ends_with("/sodium-1.jar")).count();
    let before = asked(&w);
    update_to(&w, &key, "sp-2", vec![pack_mod(&w.server, "sodium-1.jar", b"sodium 1")], &[]).await;
    assert_eq!(asked(&w), before, "the same file is there already");
    assert_eq!(fs::read(game.join("mods/sodium-1.jar")).unwrap(), b"sodium 1");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_record_keeps_the_players_settings_files() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::write(game.join("options.txt"), b"keybinds of the player").unwrap();
    // A record from before 5.4: no hashes, and it says the pack put options.txt there.
    fs::write(
        game.join(".launcher/modrinth-pack.json"),
        json!({"schema_version": 2, "project_id": "SPEEDY", "version_id": "sp-1",
               "managed_files": ["mods/sodium-1.jar", "mods/old.jar", "config/a.txt", "options.txt"]})
        .to_string(),
    )
    .unwrap();
    update_to(&w, &key, "sp-2", vec![], &[("overrides/options.txt", b"pack defaults")]).await;
    assert_eq!(fs::read(game.join("options.txt")).unwrap(), b"keybinds of the player");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_the_player_turned_off_stays_off() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    fs::rename(game.join("mods/sodium-1.jar"), game.join("mods/sodium-1.jar.disabled")).unwrap();
    update_to(&w, &key, "sp-2", vec![pack_mod(&w.server, "sodium-1.jar", b"sodium 1")], &[]).await;
    assert!(!game.join("mods/sodium-1.jar").exists(), "the pack does not turn it back on");
    assert!(game.join("mods/sodium-1.jar.disabled").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_copy_found_by_its_mod_id_refuses_the_update() {
    let w = world().await;
    let (key, game) = installed(&w).await;
    // A jar the player put there by hand: no record says whose it is, its mod id does.
    fs::write(game.join("mods/my-sodium.jar"), mod_jar("sodium", "Sodium", "mine")).unwrap();
    let files = vec![pack_mod(&w.server, "sodium-2.jar", &mod_jar("sodium", "Sodium", "pack"))];
    publish_pack(&w.server, "SPEEDY", "sp-2", &mrpack_of(files, &[], "sp-2"));
    let err = w
        .service
        .update_pack(&PackUpdateArgs { key: key.clone(), version_id: "sp-2".into(), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ContentConflict);
    assert_eq!(err.params.get("name").map(String::as_str), Some("my-sodium.jar"));
    assert!(
        !game.join("mods/sodium-2.jar").exists() && game.join("mods/sodium-1.jar").is_file(),
        "nothing changed"
    );
}
