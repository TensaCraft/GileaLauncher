mod support;

use std::io::{Cursor, Write};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use launcher_core::feedback::OperationSpec;
use launcher_core::launch::options::game_dir;
use launcher_shared::provider::{PackArgs, PackInstallArgs, PacksArgs};
use launcher_shared::{ErrorCode, Text};
use module_modrinth::backend::pack::INDEX;
use serde_json::{Value, json};
use support::*;
use zip::write::SimpleFileOptions;

fn mrpack(files: Vec<Value>, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let index = json!({"formatVersion": 1, "game": "minecraft", "versionId": "1.0", "name": "Speedy",
        "summary": "Fast and light", "files": files,
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.9"}});
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(INDEX, SimpleFileOptions::default()).unwrap();
    zip.write_all(index.to_string().as_bytes()).unwrap();
    for (name, body) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn mod_file(server: &FakeModrinth, name: &str, body: &[u8]) -> Value {
    json!({"path": format!("mods/{name}"), "downloads": [server.file(name, body)], "fileSize": body.len(),
           "hashes": {"sha512": sha512(body)}, "env": {"client": "required", "server": "required"}})
}

fn args(name: &str) -> PackInstallArgs {
    PackInstallArgs {
        project_id: "SPEEDY".into(),
        version_id: "sp-1".into(),
        name: name.into(),
        icon_url: None,
    }
}

/// The build folders (the launcher's own `.launcher-*` entries left out).
fn folders(w: &World) -> usize {
    std::fs::read_dir(w.versions.games_dir())
        .map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_modpack_becomes_a_ready_build() {
    let w = world().await;
    let pack = mrpack(
        vec![mod_file(&w.server, "sodium.jar", b"sodium")],
        &[
            ("overrides/config/a.txt", b"common"),
            ("client-overrides/config/a.txt", b"client"),
            ("overrides/options.txt", b"o"),
        ],
    );
    publish_pack(&w.server, "SPEEDY", "sp-1", &pack);
    let icon = Some("https://cdn.modrinth.com/i.png".to_string());
    let done =
        w.service.install_pack(&PackInstallArgs { icon_url: icon.clone(), ..args("Швидка") }).await.unwrap();
    let build = w.versions.get(&done.key).unwrap();
    assert_eq!(done.name, "Швидка");
    assert_eq!(
        (
            build.version.as_deref(),
            build.loader.as_deref(),
            build.client.as_deref(),
            build.loader_version.as_deref()
        ),
        (Some("1.21.1"), Some("fabric-loader-0.16.9-1.21.1"), Some("Fabric"), Some("0.16.9"))
    );
    assert_eq!(build.image, icon);
    assert_eq!(build.description, "Fast and light");
    assert_eq!(build.options["modrinthProjectId"], json!("SPEEDY"));
    assert_eq!(build.options["modrinthVersionId"], json!("sp-1"));
    assert_eq!(*w.components.asked.lock().unwrap(), ["fabric-loader-0.16.9-1.21.1"]);
    let game = game_dir(&build, w.versions.minecraft_dir());
    assert_eq!(std::fs::read(game.join("mods/sodium.jar")).unwrap(), b"sodium");
    assert_eq!(std::fs::read(game.join("config/a.txt")).unwrap(), b"client", "client overrides win");
    assert_eq!(std::fs::read(game.join("options.txt")).unwrap(), b"o");
    let record: Value =
        serde_json::from_slice(&std::fs::read(game.join(".launcher/modrinth-pack.json")).unwrap()).unwrap();
    assert_eq!(
        (record["project_id"].clone(), record["version_id"].clone()),
        (json!("SPEEDY"), json!("sp-1"))
    );
    assert!(record["managed_files"].as_array().unwrap().contains(&json!("mods/sodium.jar")));
    assert!(!game.join(".launcher-pack.mrpack").exists(), "the downloaded pack goes");
    assert_eq!(w.said.keys().last().map(String::as_str), Some("version_install_success"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_pack_leaves_no_build_and_no_folder() {
    let w = world().await;
    let mut broken = mod_file(&w.server, "sodium.jar", b"sodium");
    broken["hashes"] = json!({"sha512": sha512(b"something else")});
    publish_pack(&w.server, "SPEEDY", "sp-1", &mrpack(vec![broken], &[]));
    let before = folders(&w);
    assert!(w.service.install_pack(&args("Зламана")).await.is_err());
    assert!(w.versions.get_by_name("Зламана").is_none());
    assert_eq!(folders(&w), before, "the claimed folder is gone");
    w.components.fail.store(true, Ordering::SeqCst);
    publish_pack(&w.server, "SPEEDY", "sp-1", &mrpack(vec![], &[]));
    let loader = w.service.install_pack(&args("Без лоадера")).await.unwrap_err();
    assert_eq!(loader.code, ErrorCode::LoaderInstallFailed);
    assert!(w.versions.get_by_name("Без лоадера").is_none());
    assert_eq!(folders(&w), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_cannot_bring_the_build_s_own_record() {
    let w = world().await;
    publish_pack(
        &w.server,
        "SPEEDY",
        "sp-1",
        &mrpack(vec![], &[("overrides/version.json", b"{\"name\":\"x\"}")]),
    );
    let before = folders(&w);
    let err = w.service.install_pack(&args("Запис")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(w.versions.get_by_name("Запис").is_none());
    assert_eq!(folders(&w), before);
    assert!(w.components.asked.lock().unwrap().is_empty(), "refused before the loader");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_pack_is_placed_in_one_pass() {
    let w = world().await;
    let names: Vec<String> = (0..1500).map(|i| format!("overrides/config/many/{i}.txt")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    publish_pack(
        &w.server,
        "SPEEDY",
        "sp-1",
        &mrpack(vec![mod_file(&w.server, "sodium.jar", b"sodium")], &entries),
    );
    let started = Instant::now();
    let done = w.service.install_pack(&args("Велика")).await.unwrap();
    let took = started.elapsed();
    let game = game_dir(&w.versions.get(&done.key).unwrap(), w.versions.minecraft_dir());
    assert_eq!(std::fs::read_dir(game.join("config/many")).unwrap().count(), 1500);
    assert_eq!(std::fs::read(game.join("mods/sodium.jar")).unwrap(), b"sodium");
    assert!(game.join(".launcher/modrinth-pack.json").is_file());
    // A pass per file takes minutes here; a slow disk, seconds.
    assert!(took < Duration::from_secs(60), "placing 1500 files took {took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_taken_name_or_a_busy_launcher_is_refused_before_any_download() {
    let w = world().await;
    publish_pack(&w.server, "SPEEDY", "sp-1", &mrpack(vec![], &[]));
    build(&w, "Aero", "Fabric", "fabric-loader-0.16.9-1.21.1");
    let taken = w.service.install_pack(&args("aero")).await.unwrap_err();
    assert_eq!(taken.code, ErrorCode::VersionExists);
    let empty = w.service.install_pack(&args("   ")).await.unwrap_err();
    assert_eq!(empty.code, ErrorCode::VersionNameEmpty);
    let op = w.feedback.begin(OperationSpec::new(Text::raw("other"), "install"));
    let busy = w.service.install_pack(&args("Друга")).await.unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy);
    drop(op);
    assert!(!w.server.seen().iter().any(|(p, _)| p.starts_with("/files/")), "nothing was downloaded");
}

#[tokio::test(flavor = "multi_thread")]
async fn modpacks_are_searched_twenty_a_page_and_their_versions_listed() {
    let w = world().await;
    w.server.data.lock().unwrap().search = Some(json!({"hits": [], "total_hits": 0}));
    let page = w.service.packs(&PacksArgs { query: " fast ".into(), offset: 20 }).await.unwrap();
    assert_eq!((page.offset, page.limit), (20, 20));
    let (path, _) = w.server.seen().into_iter().find(|(p, _)| p.starts_with("/v2/search")).unwrap();
    let query = pairs(&path);
    assert!(query.contains(&("facets".into(), r#"[["project_type:modpack"]]"#.into())));
    assert!(
        query.contains(&("query".into(), "fast".into())) && query.contains(&("limit".into(), "20".into()))
    );
    publish_pack(&w.server, "SPEEDY", "sp-1", &mrpack(vec![], &[]));
    let versions = w.service.pack_versions(&PackArgs { project_id: "SPEEDY".into() }).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].label(), "sp-1 (1.21.1)");
}
