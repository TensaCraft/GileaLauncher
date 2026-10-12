//! The «Необов'язкові» source: a server build's whitelist searched, installed, and overviewed.

mod support;

use std::io::Write;

use launcher_core::launch::options::game_dir;
use launcher_core::providers::ContentProvider;
use launcher_core::storage::versions::Build;
use launcher_shared::provider::{
    Action, InstallAnswer, InstallArgs, OverviewArgs, SearchArgs, UpdatesStatus,
};
use launcher_shared::{ContentKind, ErrorCode};
use module_tensa::backend::identity::CLIENT;
use module_tensa::backend::optional::read_note;
use module_tensa::backend::provider::ServerBuilds;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::World;

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A jar that declares Fabric mod `id`, `tag` making each version's bytes its own.
fn jar(id: &str, tag: &str) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("fabric.mod.json", options).unwrap();
        let meta = json!({"schemaVersion": 1, "id": id, "version": tag});
        zip.write_all(meta.to_string().as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    bytes.into_inner()
}

/// The server build of the catalog and of the launcher: Fabric for 26.3.
fn server_build(w: &World) -> Build {
    server_build_with(w, json!({}))
}

/// The server build, its catalog entry with `extra` client fields (the test server answers a
/// path from a queue, so a path is given its answer once).
fn server_build_with(w: &World, extra: Value) -> Build {
    let mut client =
        json!({"id": "aero", "name": "Aero", "minecraft_version": "26.3", "loader_id": "fabric"});
    for (k, v) in extra.as_object().unwrap() {
        client[k] = v.clone();
    }
    w.catalog(client);
    let mut build = Build::new("Aero");
    build.version = Some("26.3".into());
    build.client = Some(CLIENT.into());
    build.remote_pack_id = Some(json!("aero"));
    build.options.insert("managedByApi".into(), json!(true));
    w.deps.versions.create(&mut build).unwrap();
    build
}

fn folder(w: &World, build: &Build) -> std::path::PathBuf {
    game_dir(build, w.deps.versions.minecraft_dir())
}

fn put(w: &World, build: &Build, relative: &str, bytes: &[u8]) {
    let path = folder(w, build).join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A whitelist entry for `bytes`, served at `/files/<name>`.
fn entry(w: &World, id: &str, title: &str, name: &str, bytes: &[u8]) -> Value {
    w.server.reply(&format!("/files/{name}"), 200, bytes.to_vec());
    json!({
        "id": id, "title": title, "type": "mod", "name": name, "relative_path": format!("mods/{name}"),
        "download_url": w.server.url(&format!("/files/{name}")), "size": bytes.len(), "sha256": sha256(bytes),
        "catalog": {"provider": "modrinth", "project_id": format!("p-{id}")}
    })
}

fn whitelist(w: &World, items: Vec<Value>) {
    w.server.json("/api/mods/aero/whitelist", json!({"modpack": {"slug": "aero"}, "items": items}));
}

fn search(key: &str, query: &str) -> SearchArgs {
    SearchArgs { key: key.into(), kind: ContentKind::Mods, query: query.into(), offset: 0 }
}

fn mods(w: &World, build: &Build) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder(w, build).join("mods"))
        .map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Installs whitelist item `id` as the player approves its plan.
async fn install(
    provider: &ServerBuilds,
    build: &Build,
    id: &str,
) -> Result<InstallAnswer, launcher_shared::AppError> {
    let args = InstallArgs::new(&build.key, ContentKind::Mods, id, id, id);
    let plan = provider.plan(args.clone()).await?;
    provider.install(InstallArgs { approved: plan.changes(&[]), ..args }).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_source_lists_the_builds_whitelist_and_filters_it() {
    let w = World::start().await;
    let build = server_build(&w);
    let goggles = entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"));
    let barrel = entry(&w, "b", "Do a Barrel Roll", "barrel-1.jar", &jar("barrel", "1"));
    whitelist(&w, vec![goggles, barrel]);
    let provider = ServerBuilds::new(w.deps.clone());
    let all = provider.search(search(&build.key, "")).await.unwrap();
    let titles: Vec<&str> = all.hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!((titles, all.total), (vec!["Cyber Goggles", "Do a Barrel Roll"], 2));
    assert_eq!((all.hits[0].project_id.as_str(), all.hits[0].author.as_str()), ("g", "goggles-2.jar"));
    assert_eq!(all.hits[0].url.as_deref(), Some("https://modrinth.com/mod/p-g"));
    let barrel = provider.search(search(&build.key, "BARREL")).await.unwrap();
    assert_eq!(barrel.hits.len(), 1, "by title, ignoring case");
    let mut own = Build::new("Own");
    own.client = Some("fabric".into());
    w.deps.versions.create(&mut own).unwrap();
    assert!(provider.search(search(&own.key, "")).await.unwrap().hits.is_empty(), "not a server build");
}

#[tokio::test(flavor = "multi_thread")]
async fn installing_puts_the_mod_in_and_notes_it() {
    let w = World::start().await;
    let build = server_build(&w);
    whitelist(&w, vec![entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"))]);
    let provider = ServerBuilds::new(w.deps.clone());
    let plan = provider.plan(InstallArgs::new(&build.key, ContentKind::Mods, "g", "g", "g")).await.unwrap();
    assert_eq!(plan.main.as_ref().map(|m| m.action), Some(Action::Install));
    let answer = install(&provider, &build, "g").await.unwrap();
    assert!(
        matches!(answer, InstallAnswer::Installed(ref done) if done.filename == "goggles-2.jar"),
        "{answer:?}"
    );
    assert_eq!(mods(&w, &build), ["goggles-2.jar"]);
    let note = read_note(&folder(&w, &build));
    assert_eq!(note["g"].file, "goggles-2.jar");
    let again = provider.plan(InstallArgs::new(&build.key, ContentKind::Mods, "g", "g", "g")).await.unwrap();
    assert_eq!(again.main.as_ref().map(|m| m.action), Some(Action::Satisfied));
}

#[tokio::test(flavor = "multi_thread")]
async fn installing_over_an_older_version_replaces_it() {
    let w = World::start().await;
    let rules = json!([{"type": "glob", "path": "mods/goggles-*.jar", "enabled": true}]);
    let build = server_build_with(&w, json!({"preserve_rules": rules}));
    put(&w, &build, "mods/goggles-1.jar", &jar("goggles", "1"));
    whitelist(&w, vec![entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"))]);
    let provider = ServerBuilds::new(w.deps.clone());
    let plan = provider.plan(InstallArgs::new(&build.key, ContentKind::Mods, "g", "g", "g")).await.unwrap();
    let main = plan.main.clone().unwrap();
    assert_eq!(
        (main.action, main.current.as_deref(), main.unrecognized),
        (Action::Replace, Some("goggles-1.jar"), true)
    );
    install(&provider, &build, "g").await.unwrap();
    assert_eq!(mods(&w, &build), ["goggles-2.jar"], "one copy of the mod");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_already_there_under_another_name_is_refused() {
    let w = World::start().await;
    let build = server_build(&w);
    put(&w, &build, "mods/renamed-goggles.jar", &jar("goggles", "1"));
    whitelist(&w, vec![entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"))]);
    let provider = ServerBuilds::new(w.deps.clone());
    let refused = install(&provider, &build, "g").await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::ContentConflict);
    assert_eq!(refused.params.get("name").map(String::as_str), Some("renamed-goggles.jar"));
    assert_eq!(mods(&w, &build), ["renamed-goggles.jar"], "nothing changed");
    assert!(read_note(&folder(&w, &build)).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_gone_from_the_whitelist_is_not_installed() {
    let w = World::start().await;
    let build = server_build(&w);
    // The plan sees the mod; by the click the server has taken it off.
    whitelist(&w, vec![entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"))]);
    whitelist(&w, Vec::new());
    let provider = ServerBuilds::new(w.deps.clone());
    let args = InstallArgs::new(&build.key, ContentKind::Mods, "g", "g", "g");
    let plan = provider.plan(args.clone()).await.unwrap();
    let gone = provider.install(InstallArgs { approved: plan.changes(&[]), ..args }).await.unwrap_err();
    assert_eq!(gone.code, ErrorCode::NotFound);
    assert!(mods(&w, &build).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_overview_names_the_mods_and_their_updates() {
    let w = World::start().await;
    let build = server_build(&w);
    put(&w, &build, "mods/barrel-1.jar", &jar("barrel", "1"));
    put(&w, &build, "mods/goggles-old.jar", &jar("goggles", "1"));
    let barrel = entry(&w, "b", "Do a Barrel Roll", "barrel-1.jar", &jar("barrel", "1"));
    let goggles = entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"));
    whitelist(&w, vec![barrel, goggles]);
    let mut note = module_tensa::backend::optional::Note::new();
    note.insert(
        "g".into(),
        module_tensa::backend::optional::Noted { file: "goggles-old.jar".into(), ..Default::default() },
    );
    module_tensa::backend::optional::write_note(&folder(&w, &build), &note).unwrap();
    let provider = ServerBuilds::new(w.deps.clone());
    let args = OverviewArgs { key: build.key.clone(), kind: ContentKind::Mods, check_updates: true };
    let overview = provider.overview(args).await.unwrap();
    let notes: Vec<(&str, &str, bool)> =
        overview.notes.iter().map(|n| (n.file.as_str(), n.project_id.as_str(), n.update.is_some())).collect();
    assert_eq!(notes, [("barrel-1.jar", "b", false), ("goggles-old.jar", "g", true)]);
    let installed: Vec<(&str, bool)> =
        overview.notes.iter().map(|n| (n.file.as_str(), n.installed)).collect();
    assert_eq!(
        installed,
        [("barrel-1.jar", false), ("goggles-old.jar", true)],
        "only what the launcher put in is its own: Modrinth keeps what it installed"
    );
    let summary = overview.updates.unwrap();
    assert_eq!((summary.status, summary.available), (UpdatesStatus::Available, 1));
    assert_eq!(overview.notes[1].update.as_ref().unwrap().version_number, "goggles-2.jar");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_that_was_not_approved_is_answered_with_the_plan() {
    let w = World::start().await;
    let build = server_build(&w);
    whitelist(&w, vec![entry(&w, "g", "Cyber Goggles", "goggles-2.jar", &jar("goggles", "2"))]);
    let provider = ServerBuilds::new(w.deps.clone());
    let answer =
        provider.install(InstallArgs::new(&build.key, ContentKind::Mods, "g", "g", "g")).await.unwrap();
    assert!(matches!(answer, InstallAnswer::Replanned(_)), "{answer:?}");
    assert!(mods(&w, &build).is_empty());
}
