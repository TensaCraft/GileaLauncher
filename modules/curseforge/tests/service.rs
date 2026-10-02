mod support;

use std::fs;

use launcher_core::providers::ContentProvider;
use launcher_shared::provider::{InstallArgs, NewerVersion, OverviewArgs, SearchArgs, UpdatesStatus};
use launcher_shared::{ContentKind, ErrorCode};
use module_curseforge::backend::provenance::PROVENANCE;
use module_curseforge::backend::service::JOURNAL;
use serde_json::{Value, json};
use support::*;

const FABRIC_LOADER: &str = "fabric-loader-0.16.9-1.21.1";
const FABRIC: &[&str] = &["1.21.1", "Fabric"];

fn args(key: &str, kind: ContentKind, id: u64, name: &str) -> InstallArgs {
    InstallArgs::new(key, kind, id.to_string(), name.to_lowercase(), name)
}

/// Create (mod 1, file 10) needs Flywheel (mod 2, file 20); both are jars of their own mod ids.
fn create_and_flywheel(server: &FakeCurseForge) {
    let required = json!([{"modId": 2, "relationType": 3}]);
    let create = file_json(server, 10, 1, "create.jar", &mod_jar("create"), FABRIC, required, false);
    server.publish(mod_json(1, "Create", MODS, 9), vec![create]);
    let flywheel = file_json(server, 20, 2, "flywheel.jar", &mod_jar("flywheel"), FABRIC, json!([]), false);
    server.publish(mod_json(2, "Flywheel", MODS, 5), vec![flywheel]);
}

fn files(dir: &std::path::Path, folder: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(folder))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn provenance(dir: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join(PROVENANCE)).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_and_its_dependencies_come_in_one_go_with_the_key() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    let done = install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!((done.project_id.as_str(), done.filename.as_str()), ("1", "create.jar"));
    assert_eq!(files(&dir, "mods"), ["create.jar", "flywheel.jar"]);
    assert_eq!(fs::read(dir.join("mods/create.jar")).unwrap(), mod_jar("create"));
    let record = &provenance(&dir)["files"]["mods/create.jar"];
    assert_eq!((record["project_id"].clone(), record["file_id"].clone()), (json!(1), json!(10)));
    assert_eq!(record["title"], "Create");
    assert!(provenance(&dir)["files"]["mods/flywheel.jar"].is_object());
    let downloads: Vec<_> = w.server.seen().into_iter().filter(|s| s.path.starts_with("/files/")).collect();
    assert_eq!(downloads.len(), 2);
    assert!(downloads.iter().all(|s| s.key.as_deref() == Some(TEST_KEY)), "the CDN gets the key");
    let journal: Value = serde_json::from_slice(&fs::read(dir.join(JOURNAL)).unwrap()).unwrap();
    assert_eq!(journal["status"], "complete");
    assert!(w.said.keys().contains(&"mod_installed".to_string()), "{:?}", w.said.keys());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_download_leaves_the_build_as_it_was() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    w.server.data.lock().unwrap().status.insert("/files/flywheel.jar".into(), 404);
    let err = install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed);
    assert!(files(&dir, "mods").is_empty());
    assert!(!dir.join(PROVENANCE).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_already_there_by_another_name_is_not_added_again() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    fs::create_dir_all(dir.join("mods")).unwrap();
    // Another build of the mod: CurseForge does not know its fingerprint, and the project's slug
    // is not its mod id; only the downloaded jar shows the clash.
    let mut other_build = mod_jar("create");
    other_build.extend_from_slice(b"another build");
    fs::write(dir.join("mods/create-by-hand.jar"), other_build).unwrap();
    let create = file_json(&w.server, 10, 1, "create.jar", &mod_jar("create"), FABRIC, json!([]), false);
    w.server.publish(mod_json(1, "Create Fork", MODS, 9), vec![create]);
    let err = install(&w, args(&key, ContentKind::Mods, 1, "Create Fork")).await.unwrap_err();
    assert_eq!((err.code, err.params["name"].as_str()), (ErrorCode::ContentConflict, "create-by-hand.jar"));
    assert_eq!(files(&dir, "mods"), ["create-by-hand.jar"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_file_replaces_the_installed_one_with_a_backup() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let old = file_json(&w.server, 9, 1, "create-0.9.jar", &mod_jar("create"), FABRIC, json!([]), false);
    w.server.publish(mod_json(1, "Create", MODS, 9), vec![old]);
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let mut newer_body = mod_jar("create");
    newer_body.extend_from_slice(b"newer");
    let newer = file_json(&w.server, 10, 1, "create-1.0.jar", &newer_body, FABRIC, json!([]), false);
    let old = file_json(&w.server, 9, 1, "create-0.9.jar", &mod_jar("create"), FABRIC, json!([]), false);
    w.server.publish(mod_json(1, "Create", MODS, 9), vec![newer, old]);
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(plan.main.as_ref().unwrap().current.as_deref(), Some("0.9.jar"));
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(files(&dir, "mods"), ["create-1.0.jar"]);
    assert!(dir.join("mods/.backups/create-0.9.jar.backup").is_file());
    let records = provenance(&dir)["files"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(records, ["mods/create-1.0.jar"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn packs_land_in_their_folders_and_a_busy_build_waits() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let pack = file_json(&w.server, 50, 5, "faithful.zip", b"pack", &["1.21.1"], json!([]), false);
    w.server.publish(mod_json(5, "Faithful", RESOURCE_PACKS, 1), vec![pack]);
    let lease = w.instances.try_acquire(&dir, "copy").unwrap();
    let busy = install(&w, args(&key, ContentKind::ResourcePacks, 5, "Faithful")).await.unwrap_err();
    assert_eq!(busy.code, ErrorCode::InstanceBusy);
    drop(lease);
    install(&w, args(&key, ContentKind::ResourcePacks, 5, "Faithful")).await.unwrap();
    assert_eq!(files(&dir, "resourcepacks"), ["faithful.zip"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_the_user_did_not_approve_comes_back_as_a_plan() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let main_only = plan.changes(&[]).into_iter().filter(|c| c.project_id == "1").collect();
    let answer = w
        .service
        .install(&InstallArgs {
            version_id: Some("10".into()),
            approved: main_only,
            ..args(&key, ContentKind::Mods, 1, "Create")
        })
        .await
        .unwrap();
    assert!(matches!(answer, launcher_shared::provider::InstallAnswer::Replanned(_)));
    assert!(files(&dir, "mods").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn files_installed_by_hand_are_known_by_their_fingerprints() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    // Flywheel's very file, put there by hand (or by another provider) under another name.
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/flywheel-by-hand.jar"), mod_jar("flywheel")).unwrap();
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(plan.satisfied.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["Flywheel"]);
    assert!(plan.install.is_empty(), "the dependency is not fetched again");
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(files(&dir, "mods"), ["create.jar", "flywheel-by-hand.jar"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_file_put_by_hand_is_replaced_by_the_newer_one() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let mut newer_body = mod_jar("create");
    newer_body.extend_from_slice(b"newer");
    let newer = file_json(&w.server, 10, 1, "create-1.0.jar", &newer_body, FABRIC, json!([]), false);
    let older = file_json(&w.server, 9, 1, "create-0.9.jar", &mod_jar("create"), FABRIC, json!([]), false);
    w.server.publish(mod_json(1, "Create", MODS, 9), vec![newer, older]);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/old-create.jar.disabled"), mod_jar("create")).unwrap();
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let main = plan.main.as_ref().unwrap();
    assert_eq!(
        (main.action, main.current.as_deref()),
        (launcher_shared::provider::Action::Replace, Some("0.9.jar"))
    );
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(files(&dir, "mods"), ["create-1.0.jar"]);
    assert!(dir.join("mods/.backups/old-create.jar.backup").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provider_searches_plans_installs_and_names_its_files() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    let provider: &dyn ContentProvider = w.service.as_ref();
    let page = provider
        .search(SearchArgs { key: key.clone(), kind: ContentKind::Mods, query: "cre".into(), offset: 0 })
        .await
        .unwrap();
    assert_eq!(page.hits.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(), ["Create"]);
    let search = w.server.seen().into_iter().find(|s| s.path == "/v1/mods/search").unwrap();
    assert!(
        search.query.contains(&("modLoaderType".into(), "4".into())),
        "a Fabric build asks for Fabric mods"
    );
    let plan = provider.plan(args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    assert_eq!(plan.install.len(), 1);
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let overview = provider
        .overview(OverviewArgs { key: key.clone(), kind: ContentKind::Mods, check_updates: false })
        .await
        .unwrap();
    let mut notes: Vec<(String, String, Option<String>)> =
        overview.notes.into_iter().map(|n| (n.file, n.title, n.url)).collect();
    notes.sort();
    assert_eq!(
        notes[0],
        (
            "create.jar".into(),
            "Create".into(),
            Some("https://www.curseforge.com/minecraft/mc-mods/create".into())
        )
    );
    assert!(overview.updates.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_overview_names_files_curseforge_knows_by_their_fingerprints() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/flywheel-by-hand.jar.disabled"), mod_jar("flywheel")).unwrap();
    fs::write(dir.join("mods/unknown.jar"), mod_jar("unknown")).unwrap();
    let overview = w
        .service
        .overview(&OverviewArgs { key, kind: ContentKind::Mods, check_updates: false })
        .await
        .unwrap();
    let notes: Vec<(String, String, String, Option<String>)> =
        overview.notes.into_iter().map(|n| (n.file, n.project_id, n.title, n.url)).collect();
    assert_eq!(
        notes,
        [(
            "flywheel-by-hand.jar.disabled".to_string(),
            "2".to_string(),
            "Flywheel".to_string(),
            Some("https://www.curseforge.com/minecraft/mc-mods/flywheel".to_string())
        )],
        "a file switched off is still the project's; one CurseForge does not know is not"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_rows_get_their_icons_once_a_session() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let look = OverviewArgs { key: key.clone(), kind: ContentKind::Mods, check_updates: false };
    let first = w.service.overview(&look).await.unwrap();
    let icons: Vec<(String, Option<String>)> =
        first.notes.into_iter().map(|n| (n.file, n.icon_url)).collect();
    assert_eq!(
        icons,
        [
            ("create.jar".to_string(), Some("https://media.forgecdn.net/avatars/1.png".to_string())),
            ("flywheel.jar".to_string(), Some("https://media.forgecdn.net/avatars/2.png".to_string())),
        ]
    );
    let asked = || w.server.seen().iter().filter(|s| s.path == "/v1/mods").count();
    let before = asked();
    w.service.overview(&look).await.unwrap();
    assert_eq!(asked(), before, "the projects are asked for once a session");
}

/// Create's older file 9 and newer file 10 (a release unless `beta`), the build on file 9.
async fn create_installed_with_a_newer_file(w: &World, key: &str, beta: bool) {
    let old = file_json(&w.server, 9, 1, "create-0.9.jar", &mod_jar("create"), FABRIC, json!([]), false);
    w.server.publish(mod_json(1, "Create", MODS, 9), vec![old.clone()]);
    install(w, args(key, ContentKind::Mods, 1, "Create")).await.unwrap();
    let mut body = mod_jar("create");
    body.extend_from_slice(b"newer");
    let mut newer = file_json(&w.server, 10, 1, "create-1.0.jar", &body, FABRIC, json!([]), false);
    newer["displayName"] = json!("Create 1.0 for mc1.21.1");
    if beta {
        newer["releaseType"] = json!(2);
    }
    w.server.publish(mod_json(1, "Create", MODS, 9), vec![newer, old]);
}

fn checking(key: &str) -> OverviewArgs {
    OverviewArgs { key: key.into(), kind: ContentKind::Mods, check_updates: true }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_file_is_offered_and_installed_as_an_update() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_installed_with_a_newer_file(&w, &key, false).await;
    let overview = w.service.overview(&checking(&key)).await.unwrap();
    let summary = overview.updates.unwrap();
    assert_eq!((summary.status, summary.available), (UpdatesStatus::Available, 1));
    let note = overview.notes.iter().find(|n| n.file == "create-0.9.jar").unwrap();
    assert_eq!(
        note.update,
        Some(NewerVersion { version_id: "10".into(), version_number: "1.0 for mc1.21.1".into() })
    );
    let update = note.update_args(&key, ContentKind::Mods, "Create").unwrap();
    install(&w, update).await.unwrap();
    assert_eq!(files(&dir, "mods"), ["create-1.0.jar"]);
    assert!(dir.join("mods/.backups/create-0.9.jar.backup").is_file());
    let after = w.service.overview(&checking(&key)).await.unwrap().updates.unwrap();
    assert_eq!(after.status, UpdatesStatus::Current);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_is_not_offered_a_beta() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_installed_with_a_newer_file(&w, &key, true).await;
    let summary = w.service.overview(&checking(&key)).await.unwrap().updates.unwrap();
    assert_eq!((summary.status, summary.available), (UpdatesStatus::Current, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn switched_off_files_are_not_checked_and_a_failed_check_says_so() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_installed_with_a_newer_file(&w, &key, false).await;
    w.server.data.lock().unwrap().status.insert("/v1/mods".into(), 500);
    let failed = w.service.overview(&checking(&key)).await.unwrap();
    assert_eq!(failed.updates.unwrap().status, UpdatesStatus::Failed);
    assert_eq!(failed.notes.len(), 1, "the files are still named");
    w.server.data.lock().unwrap().status.clear();
    fs::rename(dir.join("mods/create-0.9.jar"), dir.join("mods/create-0.9.jar.disabled")).unwrap();
    let summary = w.service.overview(&checking(&key)).await.unwrap().updates.unwrap();
    assert_eq!(summary.status, UpdatesStatus::NoEnabled);
}

/// Skin Layers (mod 7, file 70) needs Held Lib (mod 8, file 80), whose file CurseForge keeps from
/// other apps; Held Lib's jar.
fn needs_a_held_lib(server: &FakeCurseForge) -> Vec<u8> {
    let required = json!([{"modId": 8, "relationType": 3}]);
    let layers = file_json(server, 70, 7, "layers.jar", &mod_jar("layers"), FABRIC, required, false);
    server.publish(mod_json(7, "Skin Layers", MODS, 9), vec![layers]);
    let jar = mod_jar("heldlib");
    let held = file_json(server, 80, 8, "held-lib.jar", &jar, FABRIC, json!([]), true);
    server.publish(mod_json(8, "Held Lib", MODS, 5), vec![held]);
    jar
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_found_nowhere_names_the_file_to_download_by_hand() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let jar = needs_a_held_lib(&w.server);
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 7, "Skin Layers")).await.unwrap();
    assert!(!plan.can_install());
    let issue = &plan.blocking[0];
    assert_eq!((issue.code.as_str(), issue.name.as_deref()), ("file_blocked", Some("Held Lib")));
    let held = issue.held.clone().unwrap();
    assert_eq!(
        (held.file_name.as_str(), held.size, held.sha1.clone()),
        ("held-lib.jar", jar.len() as u64, sha1_hex(&jar))
    );
    assert_eq!(held.url.as_deref(), Some("https://www.curseforge.com/minecraft/mc-mods/held-lib/files/80"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_the_user_downloaded_is_installed_from_their_downloads() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let jar = needs_a_held_lib(&w.server);
    fs::write(w.downloads.join("held-lib.jar"), &jar).unwrap();
    install(&w, args(&key, ContentKind::Mods, 7, "Skin Layers")).await.unwrap();
    assert_eq!(files(&dir, "mods"), ["held-lib.jar", "layers.jar"]);
    assert_eq!(fs::read(dir.join("mods/held-lib.jar")).unwrap(), jar);
    let record = &provenance(&dir)["files"]["mods/held-lib.jar"];
    assert_eq!((record["project_id"].clone(), record["file_id"].clone()), (json!(8), json!(80)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_file_modrinth_has_is_installed_from_modrinth() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let jar = needs_a_held_lib(&w.server);
    w.server.on_modrinth("held-lib.jar", &jar);
    install(&w, args(&key, ContentKind::Mods, 7, "Skin Layers")).await.unwrap();
    assert_eq!(fs::read(dir.join("mods/held-lib.jar")).unwrap(), jar);
    let from_modrinth = w.server.seen().into_iter().find(|s| s.path == "/files/mr-held-lib.jar").unwrap();
    assert_eq!(from_modrinth.key, None, "the key goes to CurseForge only");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_changed_after_the_plan_is_refused() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    let jar = needs_a_held_lib(&w.server);
    let copy = w.downloads.join("held-lib.jar");
    fs::write(&copy, &jar).unwrap();
    let plan = w.service.plan(&args(&key, ContentKind::Mods, 7, "Skin Layers")).await.unwrap();
    assert!(plan.can_install());
    // Swapped for other bytes of its size between the plan and the install: found nowhere now.
    let mut other = jar.clone();
    let last = other.len() - 1;
    other[last] ^= 1;
    fs::write(&copy, &other).unwrap();
    let approved = InstallArgs {
        version_id: plan.main.as_ref().map(|m| m.version_id.clone()),
        approved: plan.changes(&[]),
        ..args(&key, ContentKind::Mods, 7, "Skin Layers")
    };
    match w.service.install(&approved).await.unwrap() {
        launcher_shared::provider::InstallAnswer::Replanned(plan) => assert!(!plan.can_install()),
        other => panic!("installed: {other:?}"),
    }
    assert!(files(&dir, "mods").is_empty(), "nothing goes in");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_curseforge_installed_are_its_own_and_ones_found_by_fingerprint_are_not() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC_LOADER);
    create_and_flywheel(&w.server);
    install(&w, args(&key, ContentKind::Mods, 1, "Create")).await.unwrap();
    // Flywheel's jar again, put there by hand: CurseForge knows it, it did not install it.
    fs::write(dir.join("mods/flywheel-copy.jar.disabled"), mod_jar("flywheel")).unwrap();
    fs::remove_file(dir.join("mods/flywheel.jar")).unwrap();
    let overview = w
        .service
        .overview(&OverviewArgs { key, kind: ContentKind::Mods, check_updates: false })
        .await
        .unwrap();
    let mut marks: Vec<(String, bool)> = overview.notes.into_iter().map(|n| (n.file, n.installed)).collect();
    marks.sort();
    assert_eq!(marks, [("create.jar".to_string(), true), ("flywheel-copy.jar.disabled".to_string(), false)]);
}
