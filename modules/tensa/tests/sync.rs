mod support;

use launcher_core::feedback::{OperationHandle, OperationSpec};
use launcher_core::launch::options::game_dir;
use launcher_core::storage::journal::JOURNAL_DIR;
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
        ask_within: None,
    };
    sync(&deps, build, force, &op(w)).await
}

fn downloads(w: &World) -> usize {
    w.server.seen().iter().filter(|s| s.starts_with("/files/")).count()
}

// The launcher's runtime: the file work is handed off the async workers there.
#[tokio::test(flavor = "multi_thread")]
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

#[tokio::test(flavor = "multi_thread")]
async fn a_forced_name_is_taken_unless_another_build_has_it() {
    let w = World::start().await;
    catalog(
        &w,
        json!({"name": "Aeronautics", "force_update_profile_fields": ["name", "description"],
                       "description": "The server's own"}),
    );
    manifest(&w, &[]);
    let build = installed(&w);
    run(&w, &build, false).await.unwrap();
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert_eq!((saved.name.as_str(), saved.description.as_str()), ("Aeronautics", "The server's own"));

    let other = World::start().await;
    catalog(&other, json!({"name": "Aeronautics", "force_update_profile_fields": ["name"]}));
    manifest(&other, &[]);
    let mut mine = Build::new("aeronautics");
    other.deps.versions.create(&mut mine).unwrap();
    let build = installed(&other);
    run(&other, &build, false).await.unwrap();
    assert_eq!(other.deps.versions.get(&build.key).unwrap().name, "Aero", "the name is another build's");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_s_icon_carries_its_version_so_a_new_picture_shows() {
    let w = World::start().await;
    let icon = w.server.url("/icons/aero.png");
    w.server.reply("/icons/aero.png", 200, "png");
    w.server.header("/icons/aero.png", "etag", "\"aaa-1\"");
    catalog(&w, json!({"image": icon, "force_update_profile_fields": ["image"]}));
    manifest(&w, &[]);
    let build = installed(&w);
    run(&w, &build, false).await.unwrap();
    let first = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(first.image, Some(format!("{icon}?iv=aaa-1")));
    assert_eq!(run(&w, &first, false).await.unwrap(), Synced::Unchanged, "the same picture changes nothing");

    // The picture is replaced under the same address: the build's address changes with it.
    let fresh = World::start().await;
    let icon = fresh.server.url("/icons/aero.png");
    fresh.server.reply("/icons/aero.png", 200, "png");
    fresh.server.header("/icons/aero.png", "etag", "\"bbb-2\"");
    catalog(&fresh, json!({"image": icon, "force_update_profile_fields": ["image"]}));
    manifest(&fresh, &[]);
    let mut build = installed(&fresh);
    build.image = Some(format!("{icon}?iv=aaa-1"));
    fresh.deps.versions.save(&mut build).unwrap();
    run(&fresh, &build, false).await.unwrap();
    assert_eq!(fresh.deps.versions.get(&build.key).unwrap().image, Some(format!("{icon}?iv=bbb-2")));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_icon_of_the_player_s_own_is_kept_and_not_asked_about() {
    let w = World::start().await;
    let icon = w.server.url("/icons/aero.png");
    w.server.reply("/icons/aero.png", 200, "png");
    catalog(&w, json!({"image": icon}));
    manifest(&w, &[]);
    let mut build = installed(&w);
    build.image = Some("iVBORw0KGgoAAA".into());
    w.deps.versions.save(&mut build).unwrap();
    run(&w, &build, false).await.unwrap();
    assert_eq!(w.deps.versions.get(&build.key).unwrap().image.as_deref(), Some("iVBORw0KGgoAAA"));
    assert!(
        !w.server.seen().iter().any(|path| path.starts_with("/icons/")),
        "the server's picture is not asked"
    );
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

/// A server that takes connections and never answers.
async fn silent() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    (origin, task)
}

#[tokio::test]
async fn a_server_that_does_not_answer_is_waited_for_only_so_long_before_play() {
    let w = World::start().await;
    let build = installed(&w);
    let (origin, _server) = silent().await;
    let api = module_tensa::backend::api::TensaApi::new(&format!("{origin}/api/mods")).unwrap();
    let deps = SyncDeps {
        api: &api,
        versions: &w.deps.versions,
        components: w.deps.components.as_ref(),
        downloader: &w.deps.downloader,
        running: false,
        ask_within: Some(std::time::Duration::from_millis(300)),
    };
    let started = std::time::Instant::now();
    assert_eq!(sync(&deps, &build, false, &op(&w)).await.unwrap(), Synced::Skipped);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
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
    let fresh = dir.join(JOURNAL_DIR).join(JOURNAL.file);
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    std::fs::write(&fresh, journal.to_string()).unwrap();
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(std::fs::read_to_string(dir.join("mods/a.jar")).unwrap(), "jar-a");
    assert_eq!(SyncJournal::new(&dir, JOURNAL).status().as_deref(), Some("complete"));
    let saved = w.deps.versions.get(&build.key).unwrap();
    assert_eq!(saved.loader_version.as_deref(), Some("0.19.6"));
}

/// The server allows `items` (id, name, body) as optional mods of `aero`.
fn whitelist(w: &World, items: &[(&str, &str, &str)]) {
    let entries: Vec<Value> = items
        .iter()
        .map(|(id, name, body)| {
            json!({
                "id": id, "title": id, "type": "mod", "name": name, "relative_path": format!("mods/{name}"),
                "download_url": w.file(name, body), "sha256": sha256(body), "size": body.len()
            })
        })
        .collect();
    w.server.json("/api/mods/aero/whitelist", json!({"items": entries}));
}

fn mods(w: &World, build: &Build) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder(w, build).join("mods"))
        .map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn note_of(w: &World, build: &Build, id: &str, file: &str) {
    let mut note = module_tensa::backend::optional::Note::new();
    note.insert(
        id.into(),
        module_tensa::backend::optional::Noted { file: file.into(), ..Default::default() },
    );
    module_tensa::backend::optional::write_note(&folder(w, build), &note).unwrap();
}

fn goggles_glob() -> Value {
    json!({"preserve_rules": [{"type": "glob", "path": "mods/goggles-*.jar", "enabled": true}]})
}

#[tokio::test]
async fn an_outdated_optional_mod_is_replaced_by_the_whitelists() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1");
    note_of(&w, &build, "g", "goggles-1.jar");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
    let note = module_tensa::backend::optional::read_note(&folder(&w, &build));
    assert_eq!(note["g"].file, "goggles-2.jar");
}

#[tokio::test]
async fn a_current_optional_mod_stays_without_a_rule() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-2.jar", "goggles 2");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
}

#[tokio::test]
async fn an_optional_mod_gone_from_the_whitelist_is_removed() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-2.jar", "goggles 2");
    note_of(&w, &build, "g", "goggles-2.jar");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar"]);
    assert!(module_tensa::backend::optional::read_note(&folder(&w, &build)).is_empty(), "its note goes too");
}

#[tokio::test]
async fn noted_optional_mods_stay_when_the_whitelist_cannot_be_read() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("core.jar", "core")]);
    w.server.reply("/api/mods/aero/whitelist", 500, "down");
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-2.jar", "goggles 2");
    note_of(&w, &build, "g", "goggles-2.jar");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
}

#[tokio::test]
async fn a_disabled_optional_mod_is_updated_and_stays_disabled() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar.disabled", "goggles 1");
    note_of(&w, &build, "g", "goggles-1.jar");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar.disabled"]);
}

#[tokio::test]
async fn nothing_changes_when_the_optional_mods_are_current() {
    let w = World::start().await;
    catalog(&w, json!({}));
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-2.jar", "goggles 2");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Unchanged);
    assert_eq!(downloads(&w), 0);
}

#[tokio::test]
async fn a_whitelisted_file_the_manifest_also_lists_is_planned_once() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core"), ("goggles-2.jar", "goggles 2")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1");
    note_of(&w, &build, "g", "goggles-1.jar");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
    let fetched = w.server.seen().iter().filter(|s| s.as_str() == "/files/goggles-2.jar").count();
    assert_eq!(fetched, 1, "one download of it");
}

#[tokio::test]
async fn an_unrelated_mod_the_glob_matches_is_never_removed() {
    let w = World::start().await;
    catalog(&w, json!({"preserve_rules": [{"type": "glob", "path": "mods/sodium-*.jar", "enabled": true}]}));
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("s", "sodium-0.6.jar", "sodium 0.6")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/sodium-extra-0.6.jar", "sodium extra");
    run(&w, &build, false).await.unwrap();
    assert_eq!(
        mods(&w, &build),
        ["core.jar", "sodium-extra-0.6.jar"],
        "not Sodium, and Sodium is not put in"
    );
    put(&w, &build, "mods/sodium-0.6.jar", "sodium 0.6");
    run(&w, &build, false).await.unwrap();
    assert_eq!(
        mods(&w, &build),
        ["core.jar", "sodium-0.6.jar", "sodium-extra-0.6.jar"],
        "nor a second copy of it"
    );
}

#[tokio::test]
async fn a_hand_put_older_version_is_left_for_the_player_to_update() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Unchanged);
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-1.jar"]);
}

/// The launcher's note of `file` under `id`, as a sync leaves it once it put `body` there.
fn noted_as(w: &World, build: &Build, id: &str, file: &str, body: &str) {
    let mut note = module_tensa::backend::optional::Note::new();
    note.insert(
        id.into(),
        module_tensa::backend::optional::Noted {
            file: file.into(),
            sha256: sha256(body),
            size: body.len() as u64,
            modified_ms: 0,
        },
    );
    module_tensa::backend::optional::write_note(&folder(w, build), &note).unwrap();
}

#[tokio::test]
async fn an_optional_mod_keeps_updating_after_the_server_renumbers_it() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("new-id", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1");
    noted_as(&w, &build, "old-id", "goggles-1.jar", "goggles 1");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
    let note = module_tensa::backend::optional::read_note(&folder(&w, &build));
    assert_eq!(note["new-id"].file, "goggles-2.jar");
    assert!(!note.contains_key("old-id"), "the old number goes");
}

#[tokio::test]
async fn a_renumbered_mod_changed_since_it_was_noted_is_left_to_the_player() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("new-id", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1, patched by hand");
    noted_as(&w, &build, "old-id", "goggles-1.jar", "goggles 1");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Unchanged);
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-1.jar"]);
}

#[tokio::test]
async fn the_server_s_version_put_by_hand_is_updated_with_the_server() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core")]);
    whitelist(&w, &[("g", "goggles-1.jar", "goggles 1")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-1.jar", "goggles 1");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Unchanged);
    let note = module_tensa::backend::optional::read_note(&folder(&w, &build));
    assert_eq!(note["g"].file, "goggles-1.jar", "the server's very file is noted");
    assert_eq!(run(&w, &build, false).await.unwrap(), Synced::Updated);
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-2.jar"]);
}

#[tokio::test]
async fn a_mod_the_manifest_now_brings_is_left_to_the_manifest() {
    let w = World::start().await;
    catalog(&w, goggles_glob());
    manifest(&w, &[("core.jar", "core"), ("goggles-3.jar", "goggles 3")]);
    whitelist(&w, &[("g", "goggles-2.jar", "goggles 2")]);
    let build = installed(&w);
    put(&w, &build, "mods/core.jar", "core");
    put(&w, &build, "mods/goggles-2.jar", "goggles 2");
    note_of(&w, &build, "g", "goggles-2.jar");
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-3.jar"], "one copy: the server's own");
    std::fs::remove_file(folder(&w, &build).join("mods/goggles-3.jar")).unwrap();
    run(&w, &build, false).await.unwrap();
    assert_eq!(mods(&w, &build), ["core.jar", "goggles-3.jar"], "and the whitelist's never comes back");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_whitelist_does_not_hold_up_the_sync() {
    let w = World::start().await;
    // An address that takes the connection and never answers.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/whitelist", silent.local_addr().unwrap());
    catalog(&w, json!({"whitelist_endpoint": endpoint}));
    manifest(&w, &[("core.jar", "core")]);
    let build = installed(&w);
    let deps = SyncDeps {
        api: &w.deps.api,
        versions: &w.deps.versions,
        components: w.deps.components.as_ref(),
        downloader: &w.deps.downloader,
        running: false,
        ask_within: Some(std::time::Duration::from_secs(3)),
    };
    let synced = sync(&deps, &build, false, &op(&w)).await.unwrap();
    assert_eq!(synced, Synced::Updated, "the server's files came all the same");
    assert_eq!(mods(&w, &build), ["core.jar"]);
}
