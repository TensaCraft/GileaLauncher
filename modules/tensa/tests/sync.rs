mod support;

use launcher_core::feedback::{OperationHandle, OperationSpec};
use launcher_core::launch::options::game_dir;
use launcher_core::storage::journal::SyncJournal;
use launcher_core::storage::versions::Build;
use launcher_shared::{ErrorCode, Text};
use module_tensa::backend::identity::{CLIENT, JOURNAL};
use module_tensa::backend::sync::{SyncDeps, Synced, sync};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::World;

fn sha256(body: &str) -> String {
    hex(Sha256::digest(body.as_bytes()).as_slice())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A server build of the catalog, `client` fields added over the defaults.
fn catalog(w: &World, extra: Value) {
    let mut client = json!({
        "id": "aero", "name": "Aero", "minecraft_version": "26.3", "loader_id": "fabric", "loader_version": "0.19.5",
        "force_update_endpoint": w.server.url("/aero/force")
    });
    for (k, v) in extra.as_object().unwrap() {
        client[k] = v.clone();
    }
    w.catalog(client);
}

/// The force-update manifest lists `files` (name → body) in `mods`, the one managed folder.
fn manifest(w: &World, files: &[(&str, &str)]) {
    let entries: Vec<Value> = files
        .iter()
        .map(|(name, body)| {
            json!({
                "relative_path": format!("mods/{name}"), "download_url": w.file(name, body),
                "sha256": sha256(body), "size": body.len()
            })
        })
        .collect();
    w.server.json("/aero/force", json!({"files": entries, "directories": [{"path": "mods"}]}));
}

/// A server build already installed: Fabric 0.19.5 for 26.3.
fn installed(w: &World) -> Build {
    let mut build = Build::new("Aero");
    build.version = Some("26.3".into());
    build.loader = Some("fabric-loader-0.19.5-26.3".into());
    build.loader_version = Some("0.19.5".into());
    build.client = Some(CLIENT.into());
    build.remote_pack_id = Some(json!("aero"));
    build.options.insert("managedByApi".into(), json!(true));
    w.deps.versions.create(&mut build).unwrap();
    build
}

fn put(w: &World, build: &Build, relative: &str, body: &str) {
    let path = folder(w, build).join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn folder(w: &World, build: &Build) -> std::path::PathBuf {
    game_dir(build, w.deps.versions.minecraft_dir())
}

fn op(w: &World) -> OperationHandle {
    w.deps.feedback.begin(OperationSpec::new(Text::key("syncing_files_check"), "sync").hidden())
}

async fn run(w: &World, build: &Build, force: bool) -> Result<Synced, launcher_shared::AppError> {
    run_while(w, build, force, false).await
}

async fn run_while(
    w: &World,
    build: &Build,
    force: bool,
    running: bool,
) -> Result<Synced, launcher_shared::AppError> {
    let deps = SyncDeps {
        api: &w.deps.api,
        versions: &w.deps.versions,
        components: w.deps.components.as_ref(),
        downloader: &w.deps.downloader,
        running,
    };
    sync(&deps, build, force, &op(w)).await
}

fn downloads(w: &World) -> usize {
    w.server.seen().iter().filter(|s| s.starts_with("/files/")).count()
}

#[tokio::test]
async fn a_sync_downloads_new_files_deletes_stale_ones_and_saves_the_build() {
    let w = World::start().await;
    catalog(&w, json!({"jvm_arguments": ["-XX:+UseG1GC"], "force_update_profile_fields": ["jvm"]}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    put(&w, &build, "mods/old.jar", "old");
    put(&w, &build, "config/mine.json", "mine");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    let dir = folder(&w, &build);
    assert_eq!(std::fs::read_to_string(dir.join("mods/a.jar")).unwrap(), "jar-a");
    assert!(!dir.join("mods/old.jar").exists());
    assert!(dir.join("config/mine.json").is_file(), "outside the managed folders nothing changes");
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(saved.options.get("jvmArguments"), Some(&json!(["-XX:+UseG1GC"])));
    assert_eq!(saved.options.get("tensacraftPackId"), Some(&json!("aero")));
    assert!(saved.force_update);
    assert_eq!(SyncJournal::new(&dir, JOURNAL).status().as_deref(), Some("complete"));
}

#[tokio::test]
async fn an_unchanged_build_downloads_nothing() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    put(&w, &build, "mods/a.jar", "jar-a");
    assert!(run(&w, &build, false).await.is_ok());
    let again = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(run(&w, &again, false).await.unwrap(), Synced::Unchanged);
    assert_eq!(downloads(&w), 0);
}

#[tokio::test]
async fn a_new_loader_is_installed_and_named_in_the_build() {
    let w = World::start().await;
    catalog(&w, json!({"loader_version": "0.19.6"}));
    manifest(&w, &[]);
    let build = installed(&w);
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(w.components.asked.lock().unwrap().as_slice(), ["fabric-loader-0.19.6-26.3"]);
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(
        (saved.loader.as_deref(), saved.loader_version.as_deref(), saved.client.as_deref()),
        (Some("fabric-loader-0.19.6-26.3"), Some("0.19.6"), Some(CLIENT))
    );
    assert_eq!(saved.options.get("executablePath"), Some(&json!("java-for-26.3")));
}

#[tokio::test]
async fn a_failed_download_leaves_files_and_build_as_they_were() {
    let w = World::start().await;
    catalog(&w, json!({"jvm_arguments": ["-XX:+UseG1GC"], "force_update_profile_fields": ["jvm"]}));
    let good = json!({"relative_path": "mods/a.jar", "download_url": w.file("a.jar", "jar-a"), "sha256": sha256("jar-a")});
    w.server.reply("/files/b.jar", 500, "{}");
    let bad = json!({"relative_path": "mods/b.jar", "download_url": w.server.url("/files/b.jar"), "sha256": sha256("jar-b")});
    w.server.json("/aero/force", json!({"files": [good, bad], "directories": [{"path": "mods"}]}));
    let build = installed(&w);
    put(&w, &build, "mods/a.jar", "old-a");
    put(&w, &build, "mods/old.jar", "old");
    assert!(run(&w, &build, false).await.is_err());
    let dir = folder(&w, &build);
    assert_eq!(std::fs::read_to_string(dir.join("mods/a.jar")).unwrap(), "old-a");
    assert!(dir.join("mods/old.jar").is_file());
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert!(!saved.options.contains_key("jvmArguments"), "the build is kept as it was");
    assert_eq!(SyncJournal::new(&dir, JOURNAL).status().as_deref(), Some("rolled_back"));
}

#[tokio::test]
async fn an_unreachable_server_is_skipped_unless_forced() {
    let w = World::start().await;
    w.server.reply("/api/mods", 500, "{}");
    let build = installed(&w);
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Skipped);
    let error = run(&w, &build, true).await.unwrap_err();
    assert_eq!(
        (error.code, error.params.get("pack").map(String::as_str)),
        (ErrorCode::Network, Some("aero"))
    );

    let named = World::start().await;
    catalog(&named, json!({}));
    named.server.reply("/aero/force", 500, "{}");
    let build = installed(&named);
    assert_eq!(run(&named, &build, false).await.unwrap(), Synced::Skipped, "its named manifest is gone");
    assert_eq!(run(&named, &build, true).await.unwrap_err().code, ErrorCode::Network);

    let gone = World::start().await;
    gone.server.json("/api/mods", json!([]));
    let build = installed(&gone);
    assert_eq!(run(&gone, &build, false).await.unwrap(), Synced::Skipped, "the catalog no longer lists it");
    assert_eq!(run(&gone, &build, true).await.unwrap_err().code, ErrorCode::Network);
}

#[tokio::test]
async fn forced_sync_downloads_unchanged_files_again() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    put(&w, &build, "mods/a.jar", "jar-a");
    assert_eq!(run(&w, &build, true).await.unwrap(), Synced::Updated);
    assert_eq!(downloads(&w), 1);
}

#[tokio::test]
async fn empty_folders_left_in_managed_folders_go() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    put(&w, &build, "mods/sub/deep/old.jar", "old");
    run(&w, &build, false).await.unwrap();
    let dir = folder(&w, &build);
    assert!(!dir.join("mods/sub").exists());
    assert!(dir.join("mods").is_dir(), "the managed folder itself stays");
}

#[tokio::test]
async fn a_running_game_blocks_changes_but_not_a_quiet_sync() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    let error = run_while(&w, &build, false, true).await.unwrap_err();
    assert_eq!(
        (error.code, error.params.get("version").map(String::as_str)),
        (ErrorCode::GameRunning, Some("Aero"))
    );
    assert!(!folder(&w, &build).join("mods/a.jar").exists(), "nothing changes under a running game");
    put(&w, &build, "mods/a.jar", "jar-a");
    assert_eq!(run_while(&w, &build, false, true).await.unwrap(), Synced::Unchanged);
}

#[tokio::test]
async fn an_interrupted_commit_does_not_block_later_syncs() {
    let w = World::start().await;
    catalog(&w, json!({"loader_version": "0.19.6"}));
    manifest(&w, &[("a.jar", "jar-a")]);
    let build = installed(&w);
    let dir = folder(&w, &build);
    std::fs::create_dir_all(&dir).unwrap();
    // A sync that crashed while saving the build: its files were swapped in, its commit was not
    // done — and the server has changed since.
    let journal = json!({
        "schema_version": 2, "status": "committing", "operation": "tensacraft_sync",
        "commit_key": "tensacraft-build:an-older-target", "transaction_id": "0123456789abcdef0123456789abcdef",
        "entries": []
    });
    std::fs::write(dir.join(JOURNAL), journal.to_string()).unwrap();
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(std::fs::read_to_string(dir.join("mods/a.jar")).unwrap(), "jar-a");
    assert_eq!(SyncJournal::new(&dir, JOURNAL).status().as_deref(), Some("complete"));
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(saved.loader_version.as_deref(), Some("0.19.6"));
}
