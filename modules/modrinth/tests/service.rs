mod support;

use std::fs;

use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use support::*;

use launcher_core::providers::ContentProvider;
use launcher_shared::provider::{Action, InstallAnswer, InstallArgs, Pick, SearchArgs};
use launcher_shared::provider::{PACKS_LIMIT, PacksArgs};
use launcher_shared::{ContentKind, ErrorCode};
use module_modrinth::backend::resolver::FABRIC_API;
use module_modrinth::backend::service::JOURNAL;
use std::sync::Arc;

const PROVENANCE: &str = ".launcher/modrinth-content.json";
const FABRIC: &str = "fabric-loader-0.16.9-1.21.1";
/// Quilt builds keep the implicit Fabric API dependency out of the single-file tests.
const QUILT: &str = "quilt-loader-0.26.0-1.21.1";
const BODY: &[u8] = b"sodium jar bytes";

/// Sodium on the fake server: a Forge-only newest version, the Fabric one to pick (a sources jar
/// first, the primary jar second) and an older Fabric one.
fn serve_sodium(w: &World, body: &[u8]) {
    let good = w.server.file("sodium-0.6.jar", body);
    let sources = w.server.file("sodium-sources.jar", b"sources");
    let versions = json!([
        version(
            "v3",
            "AANobbMI",
            "0.7.0",
            &["1.21.1"],
            &["forge"],
            vec![file_entry(&good, "sodium-forge.jar", BODY, true)]
        ),
        version(
            "v2",
            "AANobbMI",
            "0.6.0",
            &["1.21.1"],
            &["fabric", "quilt"],
            vec![
                file_entry(&sources, "sodium-sources.jar", b"sources", false),
                file_entry(&good, "sodium-0.6.jar", BODY, true)
            ]
        ),
        version(
            "v1",
            "AANobbMI",
            "0.5.0",
            &["1.21.1"],
            &["fabric"],
            vec![file_entry(&good, "sodium-0.5.jar", BODY, true)]
        ),
    ]);
    w.server.publish(project_json("AANobbMI", "Sodium"), versions.as_array().unwrap().clone());
}

fn sodium(key: &str) -> InstallArgs {
    InstallArgs::new(key, ContentKind::Mods, "AANobbMI", "sodium", "Sodium")
}

fn provenance(dir: &std::path::Path) -> Value {
    serde_json::from_str(&fs::read_to_string(dir.join(PROVENANCE)).unwrap()).unwrap()
}

fn journal_status(dir: &std::path::Path) -> Value {
    serde_json::from_str::<Value>(&fs::read_to_string(dir.join(JOURNAL)).unwrap()).unwrap()["status"].clone()
}

fn downloads(w: &World) -> Vec<String> {
    w.server.seen().into_iter().map(|(path, _)| path).filter(|p| p.starts_with("/files/")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn searching_asks_for_this_builds_content() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    w.server.data.lock().unwrap().search = Some(json!({"total_hits": 40, "hits": [
        {"project_id": "AANobbMI", "slug": "sodium", "title": "Sodium", "author": "jellysquid3", "downloads": 100}]}));
    let args = SearchArgs { key: key.clone(), kind: ContentKind::Mods, query: "  sod ".into(), offset: 16 };
    let page = w.service.search(&args).await.unwrap();
    assert_eq!((page.hits.len(), page.total, page.offset, page.limit), (1, 40, 16, 16));
    let seen = pairs(&w.server.seen()[0].0);
    let get = |k: &str| seen.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str());
    assert_eq!(get("query"), Some("sod"));
    assert_eq!(get("facets"), Some(r#"[["project_type:mod"],["categories:fabric"],["versions:1.21.1"]]"#));
    assert_eq!((get("offset"), get("limit"), get("index")), (Some("16"), Some("16"), Some("relevance")));
    let packs = SearchArgs { kind: ContentKind::ResourcePacks, offset: 0, ..args };
    w.service.search(&packs).await.unwrap();
    let seen = pairs(&w.server.seen()[1].0);
    assert!(
        seen.contains(&("facets".into(), r#"[["project_type:resourcepack"],["versions:1.21.1"]]"#.into())),
        "{seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn vanilla_builds_search_only_resource_packs() {
    let w = world().await;
    let (key, _) = build(&w, "Vanilla", "Minecraft", "1.21.1");
    w.server.data.lock().unwrap().search = Some(json!({"hits": [], "total_hits": 0}));
    for kind in [ContentKind::Mods, ContentKind::ShaderPacks] {
        let args = SearchArgs { key: key.clone(), kind, query: String::new(), offset: 0 };
        assert_eq!(w.service.search(&args).await.unwrap_err().code, ErrorCode::Unsupported, "{kind:?}");
    }
    let packs =
        SearchArgs { key: key.clone(), kind: ContentKind::ResourcePacks, query: String::new(), offset: 0 };
    assert!(w.service.search(&packs).await.is_ok());
    let missing = SearchArgs { key: "ghost".into(), ..packs };
    assert_eq!(w.service.search(&missing).await.unwrap_err().code, ErrorCode::VersionNotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn installing_puts_the_newest_compatible_primary_file_in_place() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, BODY);
    let done = install(&w, sodium(&key)).await.unwrap();
    assert_eq!(
        (done.project_id.as_str(), done.filename.as_str(), done.version_number.as_str()),
        ("AANobbMI", "sodium-0.6.jar", "0.6.0")
    );
    assert_eq!(fs::read(dir.join("mods/sodium-0.6.jar")).unwrap(), BODY);
    assert_eq!(
        provenance(&dir)["files"]["mods/sodium-0.6.jar"],
        json!({"content_key": "mods", "filename": "sodium-0.6.jar", "project_id": "AANobbMI", "project_slug": "sodium",
               "project_title": "Sodium", "version_id": "v2", "version_number": "0.6.0",
               "hash_algorithm": "sha512", "file_hash": sha512(BODY)})
    );
    assert_eq!(journal_status(&dir), "complete");
    assert_eq!(downloads(&w), ["/files/sodium-0.6.jar"], "only the primary file");
    let asked = pairs(&w.server.seen()[0].0);
    assert_eq!(
        asked,
        [("loaders", r#"["quilt","fabric"]"#), ("game_versions", r#"["1.21.1"]"#)]
            .map(|(k, v)| (k.to_string(), v.to_string())),
        "a Quilt build runs Fabric's mods too"
    );
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, ["AANobbMI"]);
    assert!(installed(&w, &key, ContentKind::ResourcePacks).await.is_empty());
    assert!(w.instances.try_acquire(&dir, "check").is_ok(), "the lease is given back");
}

#[tokio::test(flavor = "multi_thread")]
async fn packs_install_without_a_loader_filter() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let url = w.server.file("faithful.zip", b"pack");
    w.server.data.lock().unwrap().versions.insert(
        "faith".into(),
        json!([version(
            "f1",
            "faith",
            "1.0",
            &["1.21.1"],
            &["minecraft"],
            vec![file_entry(&url, "faithful.zip", b"pack", true)]
        )]),
    );
    let args = InstallArgs::new(key.clone(), ContentKind::ResourcePacks, "faith", "", "Faithful");
    install(&w, args).await.unwrap();
    assert_eq!(fs::read(dir.join("resourcepacks/faithful.zip")).unwrap(), b"pack");
    let record = &provenance(&dir)["files"]["resourcepacks/faithful.zip"];
    assert_eq!(
        (record["content_key"].as_str(), record["project_slug"].as_str()),
        (Some("resourcepacks"), Some("faith"))
    );
    let asked = pairs(&w.server.seen()[0].0);
    assert_eq!(asked, [("game_versions".to_string(), r#"["1.21.1"]"#.to_string())], "no loader for packs");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_already_there_is_not_replaced() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, BODY);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-0.6.jar.disabled"), b"mine").unwrap();
    let err = install(&w, sodium(&key)).await.unwrap_err();
    assert_eq!(
        (err.code, err.params.get("name").map(String::as_str)),
        (ErrorCode::ContentConflict, Some("sodium-0.6.jar"))
    );
    assert!(downloads(&w).is_empty());
    assert!(!dir.join(PROVENANCE).exists() && !dir.join(JOURNAL).exists());
    assert_eq!(fs::read(dir.join("mods/sodium-0.6.jar.disabled")).unwrap(), b"mine");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_download_leaves_the_build_as_it_was() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, b"not what the hash says");
    fs::create_dir_all(dir.join(".launcher")).unwrap();
    let before = r#"{"schema_version": 2, "files": {"mods/other.jar": {"project_id": "x"}}}"#;
    fs::write(dir.join(PROVENANCE), before).unwrap();
    let err = install(&w, sodium(&key)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed);
    assert!(!dir.join("mods/sodium-0.6.jar").exists());
    assert_eq!(fs::read_to_string(dir.join(PROVENANCE)).unwrap(), before);
    assert_eq!(journal_status(&dir), "rolled_back");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provenance_keeps_what_others_wrote() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, BODY);
    fs::create_dir_all(dir.join(".launcher")).unwrap();
    fs::write(
        dir.join(PROVENANCE),
        r#"{"schema_version": 2, "note": "from TensaLauncher",
        "files": {"mods/other.jar": {"content_key": "mods", "project_id": "x", "project_slug": null}}}"#,
    )
    .unwrap();
    install(&w, sodium(&key)).await.unwrap();
    let doc = provenance(&dir);
    assert_eq!(doc["note"], "from TensaLauncher");
    assert_eq!(doc["files"]["mods/other.jar"]["project_slug"], json!(null));
    assert!(doc["files"]["mods/sodium-0.6.jar"].is_object());
    // A damaged file gives way to a valid one.
    let (key2, dir2) = build(&w, "Beta", "Quilt", QUILT);
    fs::create_dir_all(dir2.join(".launcher")).unwrap();
    fs::write(dir2.join(PROVENANCE), "{broken").unwrap();
    assert!(installed(&w, &key2, ContentKind::Mods).await.is_empty());
    install(&w, sodium(&key2)).await.unwrap();
    assert_eq!(provenance(&dir2)["files"].as_object().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_compatible_or_no_safe_file() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Quilt", QUILT);
    let url = w.server.file("old.jar", BODY);
    w.server.data.lock().unwrap().versions.insert(
        "AANobbMI".into(),
        json!([version(
            "v0",
            "AANobbMI",
            "0.4",
            &["1.20.1"],
            &["fabric"],
            vec![file_entry(&url, "old.jar", BODY, true)]
        )]),
    );
    let err = install(&w, sodium(&key)).await.unwrap_err();
    assert_eq!(
        (err.code, err.params.get("name").map(String::as_str)),
        (ErrorCode::NoCompatibleVersion, Some("Sodium"))
    );
    w.server.data.lock().unwrap().versions.insert(
        "AANobbMI".into(),
        json!([version(
            "v2",
            "AANobbMI",
            "0.6",
            &["1.21.1"],
            &["fabric", "quilt"],
            vec![file_entry("http://example.com/a.jar", "a.jar", BODY, true)]
        )]),
    );
    assert_eq!(install(&w, sodium(&key)).await.unwrap_err().code, ErrorCode::NoFileFound);
    assert!(downloads(&w).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsafe_file_name_downloads_nothing() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    let url = w.server.file("evil.jar", BODY);
    w.server.data.lock().unwrap().versions.insert(
        "AANobbMI".into(),
        json!([version(
            "v2",
            "AANobbMI",
            "0.6",
            &["1.21.1"],
            &["fabric", "quilt"],
            vec![file_entry(&url, "../evil.jar", BODY, true)]
        )]),
    );
    assert_eq!(install(&w, sodium(&key)).await.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(downloads(&w).is_empty());
    assert!(!dir.join("evil.jar").exists() && !dir.join(JOURNAL).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_build_is_refused() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, BODY);
    let _held = w.instances.try_acquire(&dir, "copy").unwrap();
    let err = install(&w, sodium(&key)).await.unwrap_err();
    assert_eq!(
        (err.code, err.params.get("version").map(String::as_str)),
        (ErrorCode::InstanceBusy, Some("Aero"))
    );
    assert!(downloads(&w).is_empty(), "nothing is downloaded while the build is busy");
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_follows_the_files_themselves() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    serve_sodium(&w, BODY);
    install(&w, sodium(&key)).await.unwrap();
    fs::rename(dir.join("mods/sodium-0.6.jar"), dir.join("mods/sodium-0.6.jar.disabled")).unwrap();
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, ["AANobbMI"], "switched off is still installed");
    fs::write(dir.join("mods/sodium-0.6.jar.disabled"), b"edited by hand").unwrap();
    assert!(
        installed(&w, &key, ContentKind::Mods).await.is_empty(),
        "a changed file is no longer Modrinth's"
    );
    // Records that name another folder, a bad hash or an unknown algorithm count for nothing.
    fs::write(dir.join("config.jar"), BODY).unwrap();
    let doc = json!({"schema_version": 2, "files": {
        "config.jar": {"content_key": "mods", "project_id": "a", "hash_algorithm": "sha512", "file_hash": sha512(BODY)},
        "mods/../config.jar": {"content_key": "mods", "project_id": "b", "hash_algorithm": "sha512", "file_hash": sha512(BODY)},
        "mods/x.jar": {"content_key": "mods", "project_id": "c", "hash_algorithm": "md5", "file_hash": "00"}}});
    fs::write(dir.join(PROVENANCE), doc.to_string()).unwrap();
    fs::write(dir.join("mods/x.jar"), BODY).unwrap();
    assert!(installed(&w, &key, ContentKind::Mods).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_names_hand_installed_files_too() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Quilt", QUILT);
    let s = &w.server;
    let v2 = release(s, "AANobbMI", "sodium-2", &["quilt"], 4, json!([]));
    s.publish(project_json("AANobbMI", "Sodium"), vec![v2.clone()]);
    s.know(b"sodium-2", &v2);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/my-sodium-copy.jar"), b"sodium-2").unwrap();
    fs::write(dir.join("mods/unknown.jar"), b"who knows").unwrap();
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, ["AANobbMI"]);
    // Modrinth out of reach: the provenance alone answers, and there is none.
    s.data.lock().unwrap().hashes_status = Some(503);
    assert!(installed(&w, &key, ContentKind::Mods).await.is_empty());
}

fn publish_extra(w: &World) {
    let s = &w.server;
    s.publish(
        project_json(FABRIC_API, "Fabric API"),
        vec![release(s, FABRIC_API, "fapi-1", &["fabric"], 2, json!([]))],
    );
    s.publish(
        project_json("extra", "Sodium Extra"),
        vec![release(
            s,
            "extra",
            "extra-2",
            &["fabric"],
            5,
            json!([{"project_id": "sodium", "version_id": "sodium-2", "dependency_type": "required"}]),
        )],
    );
    s.publish(
        project_json("sodium", "Sodium"),
        vec![
            release(s, "sodium", "sodium-2", &["fabric"], 4, json!([])),
            release(s, "sodium", "sodium-1", &["fabric"], 1, json!([])),
        ],
    );
}

fn extra(key: &str) -> InstallArgs {
    InstallArgs::new(key, ContentKind::Mods, "extra", "extra", "Sodium Extra")
}

fn mods(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join("mods"))
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

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_is_installed_in_one_go_with_its_provenance() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    let done = install(&w, extra(&key)).await.unwrap();
    assert_eq!((done.project_id.as_str(), done.filename.as_str()), ("extra", "extra-2.jar"));
    assert_eq!(mods(&dir), ["extra-2.jar", "fapi-1.jar", "sodium-2.jar"]);
    let files = provenance(&dir)["files"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(files, ["mods/extra-2.jar", "mods/fapi-1.jar", "mods/sodium-2.jar"]);
    assert_eq!(journal_status(&dir), "complete");
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, [FABRIC_API, "extra", "sodium"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_plan_comes_back_to_the_user() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    let plan = w.service.plan(&extra(&key)).await.unwrap();
    let main_only = plan.changes(&[]).into_iter().filter(|c| c.project_id == "extra").collect();
    let answer = w
        .service
        .install(&InstallArgs { version_id: Some("extra-2".into()), approved: main_only, ..extra(&key) })
        .await
        .unwrap();
    let InstallAnswer::Replanned(again) = answer else { panic!("installed without approval") };
    assert_eq!(*again, plan);
    assert!(downloads(&w).is_empty() && mods(&dir).is_empty() && !dir.join(JOURNAL).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_installed_dependency_is_not_installed_twice() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    let fapi = w.server.data.lock().unwrap().version_ids["fapi-1"].clone();
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/fabric-api-by-hand.jar"), b"fapi-1").unwrap();
    w.server.know(b"fapi-1", &fapi);
    let plan = w.service.plan(&extra(&key)).await.unwrap();
    assert!(plan.satisfied.iter().any(|i| i.project_id == FABRIC_API));
    install(&w, extra(&key)).await.unwrap();
    assert_eq!(mods(&dir), ["extra-2.jar", "fabric-api-by-hand.jar", "sodium-2.jar"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_dependency_is_backed_up_and_removed() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    let old = w.server.data.lock().unwrap().version_ids["sodium-1"].clone();
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-1.jar"), b"sodium-1").unwrap();
    w.server.know(b"sodium-1", &old);
    let plan = w.service.plan(&extra(&key)).await.unwrap();
    assert_eq!(
        plan.replace.iter().map(|i| (i.project_id.as_str(), i.current.as_deref())).collect::<Vec<_>>(),
        [("sodium", Some("sodium-1"))]
    );
    install(&w, extra(&key)).await.unwrap();
    assert_eq!(mods(&dir), ["extra-2.jar", "fapi-1.jar", "sodium-2.jar"]);
    assert_eq!(fs::read(dir.join("mods/.backups/sodium-1.jar.backup")).unwrap(), b"sodium-1");
    assert!(provenance(&dir)["files"].get("mods/sodium-1.jar").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_game_takes_replacements_too() {
    let w = world().await;
    publish_extra(&w);
    w.running.store(true, Ordering::SeqCst);
    let (fresh, fresh_dir) = build(&w, "Fresh", "Fabric", FABRIC);
    install(&w, extra(&fresh)).await.unwrap();
    assert_eq!(mods(&fresh_dir).len(), 3, "new files may join a running game");
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let old = w.server.data.lock().unwrap().version_ids["sodium-1"].clone();
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-1.jar"), b"sodium-1").unwrap();
    w.server.know(b"sodium-1", &old);
    install(&w, extra(&key)).await.unwrap();
    assert_eq!(mods(&dir), ["extra-2.jar", "fapi-1.jar", "sodium-2.jar"], "sodium 1 is replaced");
    assert_eq!(fs::read(dir.join("mods/.backups/sodium-1.jar.backup")).unwrap(), b"sodium-1");
}

/// The game holds its loaded jar open and Windows refuses to move it: the whole plan is undone and
/// the error says the file is in use.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_jar_the_game_holds_undoes_the_plan() {
    use std::os::windows::fs::OpenOptionsExt;
    let w = world().await;
    publish_extra(&w);
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let old = w.server.data.lock().unwrap().version_ids["sodium-1"].clone();
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-1.jar"), b"sodium-1").unwrap();
    w.server.know(b"sodium-1", &old);
    // FILE_SHARE_READ | FILE_SHARE_WRITE, as the JVM opens a jar.
    let held =
        fs::OpenOptions::new().read(true).share_mode(1 | 2).open(dir.join("mods/sodium-1.jar")).unwrap();
    let err = install(&w, extra(&key)).await.unwrap_err();
    drop(held);
    assert_eq!(err.code, ErrorCode::FileInUse);
    assert_eq!(mods(&dir), ["sodium-1.jar"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_changing_while_identified_stops_the_plan() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/busy.jar"), b"copying...").unwrap();
    w.server.data.lock().unwrap().touch_on_identify = Some(dir.join("mods/busy.jar"));
    let err = w.service.plan(&extra(&key)).await.unwrap_err();
    assert_eq!((err.code, err.params.get("name").map(String::as_str)), (ErrorCode::Io, Some("busy.jar")));
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_picks_are_installed_when_approved() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    publish_extra(&w);
    let s = &w.server;
    s.publish(
        project_json("iris", "Iris"),
        vec![release(
            s,
            "iris",
            "iris-1",
            &["fabric"],
            3,
            json!([{"project_id": "sodium", "dependency_type": "optional"}]),
        )],
    );
    let iris = InstallArgs::new(&key, ContentKind::Mods, "iris", "iris", "Iris");
    let plan = w.service.plan(&iris).await.unwrap();
    let picks = plan.optional.clone();
    let args = InstallArgs {
        version_id: Some("iris-1".into()),
        optional: picks
            .iter()
            .map(|i| Pick { project_id: i.project_id.clone(), version_id: i.version_id.clone() })
            .collect(),
        approved: plan.changes(&picks),
        ..iris
    };
    assert!(matches!(w.service.install(&args).await.unwrap(), InstallAnswer::Installed(_)));
    assert_eq!(mods(&dir), ["fapi-1.jar", "iris-1.jar", "sodium-2.jar"]);
    assert!(plan.changes(&picks).iter().all(|c| c.action == Action::Install));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_modrinth_named_once_is_not_asked_about_again() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", "fabric-loader-0.16.9-1.21.1");
    w.server.publish(project_json("AANobbMI", "Sodium"), vec![]);
    let v = release(&w.server, "AANobbMI", "sod-1", &["fabric"], 1, json!([]));
    w.server.know(b"sod-1", &v);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium.jar"), b"sod-1").unwrap();
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, ["AANobbMI"]);
    assert_eq!(installed(&w, &key, ContentKind::Mods).await, ["AANobbMI"]);
    let asked = w.server.seen().iter().filter(|(p, _)| p.starts_with("/v2/version_files")).count();
    assert_eq!(asked, 1, "the second list knew the file already");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_answers_as_a_content_provider() {
    let w = world().await;
    w.server.data.lock().unwrap().search = Some(json!({"hits": [{"project_id": "AANobbMI", "slug": "sodium",
        "title": "Sodium", "project_type": "mod"}], "total_hits": 1}));
    let (key, _) = build(&w, "Aero", "Fabric", "fabric-loader-0.16.9-1.21.1");
    let provider: Arc<dyn ContentProvider> = w.service.clone();
    let args = SearchArgs { key, kind: ContentKind::Mods, query: String::new(), offset: 0 };
    let page = provider.search(args).await.unwrap();
    assert_eq!(page.hits[0].url.as_deref(), Some("https://modrinth.com/mod/sodium"));
    let packs = provider.modpacks(PacksArgs { query: String::new(), offset: 0 }).await.unwrap();
    assert_eq!(packs.limit, PACKS_LIMIT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_installed_alone_leaves_every_dependency_out() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    // One dependency it can have, one no one can: with them, the install cannot go ahead.
    let deps = json!([
        {"project_id": "sodium", "dependency_type": "required"},
        {"project_id": "gone", "dependency_type": "required"},
        {"project_id": "optifine", "dependency_type": "incompatible"}
    ]);
    s.publish(project_json("main", "Main"), vec![release(s, "main", "main-1", &["fabric"], 5, deps)]);
    s.publish(
        project_json("sodium", "Sodium"),
        vec![release(s, "sodium", "sodium-1", &["fabric"], 4, json!([]))],
    );
    let with_them = InstallArgs::new(&key, ContentKind::Mods, "main", "main", "Main");
    assert!(!w.service.plan(&with_them).await.unwrap().can_install());

    let alone = InstallArgs { alone: true, ..with_them };
    let plan = w.service.plan(&alone).await.unwrap();
    assert_eq!(plan.main.as_ref().map(|m| m.version_id.as_str()), Some("main-1"));
    assert!(plan.install.is_empty() && plan.replace.is_empty() && plan.satisfied.is_empty());
    assert!(plan.optional.is_empty() && plan.blocking.is_empty() && plan.embedded.is_empty());
    assert!(plan.can_install() && !plan.requires_confirmation());
    install(&w, alone).await.unwrap();
    assert_eq!(mods(&dir), ["main-1.jar"], "the mod alone, no Fabric API either");
}
