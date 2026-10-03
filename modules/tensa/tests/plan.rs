mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use launcher_core::net::downloader::{HashKind, hash_file};
use launcher_shared::ErrorCode;
use module_tensa::backend::api::TensaApi;
use module_tensa::backend::manifest::{PreserveKind, PreserveRule};
use module_tensa::backend::pack::Pack;
use module_tensa::backend::plan::{Source, SyncPlan, plan, prepare};
use serde_json::{Value, json};
use support::Server;

fn put(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn sha256(root: &Path, relative: &str) -> String {
    hash_file(&root.join(relative), HashKind::Sha256).unwrap()
}

fn file(relative: &str) -> Value {
    json!({"relative_path": relative, "download_url": format!("http://files/{relative}"), "sha1": "00"})
}

fn downloads(plan: &SyncPlan) -> Vec<&str> {
    plan.downloads.iter().map(|f| f.relative.as_str()).collect()
}

fn stale(plan: &SyncPlan) -> Vec<&str> {
    plan.stale.iter().map(String::as_str).collect()
}

#[test]
fn a_manifest_manages_its_folders_and_files() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/old.jar", "old");
    put(dir.path(), "saves/w/level.dat", "world");
    let manifest = json!({
        "files": [file("mods/a.jar"), file("config/b.json")],
        "directories": [{"path": "mods"}, {"path": "saves", "sync_scope": "files"}]
    });
    let plan = plan(dir.path(), &Source::Manifest(manifest), &[], false);
    assert_eq!(downloads(&plan), ["mods/a.jar", "config/b.json"]);
    assert_eq!(stale(&plan), ["mods/old.jar"]);
    assert_eq!(plan.managed_dirs, ["mods"], "a folder with another scope is not managed");
    assert!(plan.has_changes());
    let first = &plan.downloads[0];
    assert_eq!(
        (first.url.as_str(), first.hash.as_ref().map(|h| h.kind)),
        ("http://files/mods/a.jar", Some(HashKind::Sha1))
    );
}

#[test]
fn a_file_checked_once_is_not_hashed_again_while_its_size_and_time_stay() {
    // Every launch planned the sync by hashing each managed file (gigabytes for a big server
    // build): what was checked is remembered in the build, by size and time.
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/a.jar", "aaa");
    let jar = dir.path().join("mods/a.jar");
    let manifest = json!({
        "files": [{
            "relative_path": "mods/a.jar",
            "download_url": "http://files/mods/a.jar",
            "sha256": sha256(dir.path(), "mods/a.jar"),
            "size": 3
        }],
        "directories": [{"path": "mods"}]
    });
    let source = Source::Manifest(manifest);
    assert!(downloads(&plan(dir.path(), &source, &[], false)).is_empty());
    // The same size and time, other bytes: only a new hash would notice.
    let modified = fs::metadata(&jar).unwrap().modified().unwrap();
    fs::write(&jar, "bbb").unwrap();
    fs::File::options().write(true).open(&jar).unwrap().set_modified(modified).unwrap();
    assert!(downloads(&plan(dir.path(), &source, &[], false)).is_empty(), "known, not hashed again");
    assert!(stale(&plan(dir.path(), &source, &[], false)).is_empty(), "the launcher's note is its own");
    // A new time is hashed again, and the damage is found.
    let later = modified + Duration::from_secs(5);
    fs::File::options().write(true).open(&jar).unwrap().set_modified(later).unwrap();
    assert_eq!(downloads(&plan(dir.path(), &source, &[], false)), ["mods/a.jar"]);
    assert_eq!(downloads(&plan(dir.path(), &source, &[], true)), ["mods/a.jar"]);
}

#[test]
fn the_launchers_backups_are_never_stale() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/old.jar", "old");
    put(dir.path(), "mods/.backups/sodium-1.jar.backup", "the player's old sodium");
    put(dir.path(), "mods/.backups/x.restore.part", "a restore underway");
    let manifest = json!({"files": [file("mods/a.jar")], "directories": [{"path": "mods"}]});
    let plan = plan(dir.path(), &Source::Manifest(manifest), &[], false);
    assert_eq!(stale(&plan), ["mods/old.jar"], "the launcher's own backups stay");
}

#[test]
fn a_server_cannot_manage_the_launchers_folders() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), ".launcher/modrinth-pack.json", "{}");
    put(dir.path(), "mods/.backups/a.jar.backup", "a");
    let manifest = json!({
        "files": [file(".launcher/evil.json"), file("version.json")],
        "directories": [{"path": ".launcher"}, {"path": "mods/.backups"}]
    });
    let plan = plan(dir.path(), &Source::Manifest(manifest), &[], false);
    assert!(plan.downloads.is_empty() && plan.stale.is_empty() && plan.managed_dirs.is_empty(), "{plan:?}");
}

#[test]
fn unchanged_files_are_kept_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/a.jar", "same");
    let same = json!({"relative_path": "mods/a.jar", "download_url": "u", "sha256": sha256(dir.path(), "mods/a.jar"), "size": 4});
    let source = Source::Manifest(json!({"files": [same], "directories": [{"path": "mods"}]}));
    let kept = plan(dir.path(), &source, &[], false);
    assert!(kept.downloads.is_empty() && kept.stale.is_empty() && !kept.has_changes());
    let forced = plan(dir.path(), &source, &[], true);
    assert_eq!(downloads(&forced), ["mods/a.jar"]);
    assert!(forced.force);
    let resized = json!({"relative_path": "mods/a.jar", "download_url": "u", "sha256": sha256(dir.path(), "mods/a.jar"), "size": 5});
    let other =
        plan(dir.path(), &Source::Manifest(json!({"files": [resized], "directories": []})), &[], false);
    assert_eq!(downloads(&other), ["mods/a.jar"], "a size that differs is a change");
    let rehashed = json!({"relative_path": "mods/a.jar", "download_url": "u", "sha256": "00"});
    let changed =
        plan(dir.path(), &Source::Manifest(json!({"files": [rehashed], "directories": []})), &[], false);
    assert_eq!(downloads(&changed), ["mods/a.jar"]);
}

#[test]
fn preserved_files_stay_and_the_rest_are_stale() {
    let dir = tempfile::tempdir().unwrap();
    for f in ["config/a.local", "config/keep.txt", "config/x.json", "config/sub/y.json", "config/server.json"]
    {
        put(dir.path(), f, "x");
    }
    let rules = [
        PreserveRule { kind: PreserveKind::Glob, path: "config/*.local".into() },
        PreserveRule { kind: PreserveKind::File, path: "config/keep.txt".into() },
    ];
    let source =
        Source::Manifest(json!({"files": [file("config/server.json")], "directories": [{"path": "config"}]}));
    let plan = plan(dir.path(), &source, &rules, false);
    assert_eq!(stale(&plan), ["config/sub/y.json", "config/x.json"]);
}

#[test]
fn a_legacy_list_without_metadata_manages_mods_only() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/extra.jar", "x");
    let plan = plan(
        dir.path(),
        &Source::Files(vec![file("mods/a.jar"), file("MODS/b.jar"), file("config/c.json")]),
        &[],
        false,
    );
    assert_eq!(downloads(&plan), ["mods/a.jar", "MODS/b.jar"]);
    assert!(plan.stale.is_empty(), "nothing is managed as a folder");
    assert!(plan.managed_dirs.is_empty());
}

#[test]
fn locked_forced_folders_are_inferred_from_a_legacy_list() {
    let dir = tempfile::tempdir().unwrap();
    for f in ["resourcepacks/old.zip", "shaderpacks/old.zip", "mods/old.jar", "config/old.json"] {
        put(dir.path(), f, "x");
    }
    let forced = |relative: &str, locked: bool| {
        let mut f = file(relative);
        f["force_update"] = json!(true);
        if locked {
            f["force_update_locked"] = json!("yes");
        }
        f
    };
    let files = vec![
        forced("resourcepacks/a.zip", true),
        forced("resourcepacks/b.zip", true),
        forced("shaderpacks/a.zip", false),
        forced("mods/a.jar", false),
        file("config/c.json"),
    ];
    let plan = plan(dir.path(), &Source::Files(files), &[], false);
    assert_eq!(plan.managed_dirs, ["mods", "resourcepacks"]);
    assert_eq!(
        downloads(&plan),
        ["resourcepacks/a.zip", "resourcepacks/b.zip", "shaderpacks/a.zip", "mods/a.jar"]
    );
    assert_eq!(stale(&plan), ["mods/old.jar", "resourcepacks/old.zip"]);
}

#[test]
fn explicit_scopes_manage_folders() {
    let dir = tempfile::tempdir().unwrap();
    let mut scoped = file("config/x/a.json");
    scoped["force_update_scope"] = json!("config/x");
    let mut by_scope = file("defaultconfigs/b.toml");
    by_scope["sync_scope"] = json!("directory");
    let folder = json!({"relative_path": "kubejs/", "force_update": true});
    let typed = json!({"relative_path": "scripts", "type": "folder"});
    let bare = json!({"relative_path": "emotes", "forceUpdate": "1"});
    let mut marked = file("resources/pack/c.png");
    marked["force_update_folder"] = json!(true);
    let plan =
        plan(dir.path(), &Source::Files(vec![scoped, by_scope, folder, typed, bare, marked]), &[], false);
    assert_eq!(
        plan.managed_dirs,
        ["config/x", "defaultconfigs", "emotes", "kubejs", "resources/pack", "scripts"]
    );
}

#[test]
fn a_file_outside_the_build_is_never_planned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("build");
    put(&root, "mods/a.jar", "x");
    put(dir.path(), "outside/keep.jar", "x");
    let manifest = json!({
        "files": [file("../evil.jar"), file("C:/evil.jar"), file("mods/ok.jar")],
        "directories": [{"path": ".."}, {"path": "."}, {"path": "../outside"}, {"path": "mods"}]
    });
    let plan = plan(&root, &Source::Manifest(manifest), &[], false);
    assert_eq!(downloads(&plan), ["mods/ok.jar"]);
    assert_eq!(stale(&plan), ["mods/a.jar"]);
    assert_eq!(plan.managed_dirs, ["mods"]);
    assert!(dir.path().join("outside/keep.jar").is_file());
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_file_whose_name_differs_only_in_case_is_the_listed_one() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/Sodium.jar", "same");
    let listed = json!({"relative_path": "mods/sodium.jar", "download_url": "u", "sha256": sha256(dir.path(), "mods/Sodium.jar")});
    let plan = plan(
        dir.path(),
        &Source::Manifest(json!({"files": [listed], "directories": [{"path": "mods"}]})),
        &[],
        false,
    );
    assert!(plan.stale.is_empty(), "{:?}", plan.stale);
    assert!(plan.downloads.is_empty());
}

fn pack(entry: Value) -> Pack {
    Pack::from_value(&entry).unwrap()
}

fn api(server: &Server) -> TensaApi {
    TensaApi::new(&server.url("/api/mods")).unwrap().with_retry_delay(Duration::from_millis(1))
}

#[tokio::test]
async fn an_invalid_manifest_falls_back_unless_its_endpoint_was_named() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start().await;
    server.json(
        "/api/mods/aero/force-update",
        json!({"files": [{"relative_path": "mods/x.jar"}], "directories": []}),
    );
    server.json("/api/mods/aero", json!({"files": [file("mods/a.jar")]}));
    let plain = pack(json!({"client": {"id": "aero"}}));
    let fell_back = prepare(&api(&server), &plain, dir.path(), false).await.unwrap();
    assert_eq!(downloads(&fell_back), ["mods/a.jar"]);

    server.json("/named", json!({"files": [{"relative_path": "mods/x.jar"}], "directories": []}));
    let named = pack(json!({"client": {"id": "aero", "force_update_endpoint": server.url("/named")}}));
    let error = prepare(&api(&server), &named, dir.path(), false).await.unwrap_err();
    assert_eq!(
        (error.code, error.params.get("pack").map(String::as_str)),
        (ErrorCode::Network, Some("aero"))
    );

    let gone = Server::start().await;
    let error = prepare(&api(&gone), &plain, dir.path(), false).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Network);
}

#[tokio::test]
async fn a_valid_manifest_wins_and_brings_its_preserve_rules() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "config/mine.cfg", "x");
    put(dir.path(), "config/old.cfg", "x");
    let server = Server::start().await;
    server.json(
        "/api/mods/aero/force-update",
        json!({
            "files": [file("config/server.cfg")],
            "directories": [{"path": "config"}],
            "preserve_rules": [{"type": "file", "path": "config/mine.cfg"}]
        }),
    );
    let plan =
        prepare(&api(&server), &pack(json!({"client": {"id": "aero"}})), dir.path(), false).await.unwrap();
    assert_eq!(stale(&plan), ["config/old.cfg"]);
    let own = pack(
        json!({"client": {"id": "aero", "preserve_rules": [{"type": "file", "path": "config/old.cfg"}]}}),
    );
    let plan = prepare(&api(&server), &own, dir.path(), false).await.unwrap();
    assert_eq!(stale(&plan), ["config/mine.cfg"], "the build's own rules win over the manifest's");
    assert!(
        !server.seen().contains(&"/api/mods/aero".to_string()),
        "no legacy list when the manifest is whole"
    );
}

/// A catalog entry and a force-update manifest as the real API gave them (2026-09-29; the server's
/// address made neutral, the manifest cut to two files, the description shortened).
#[test]
fn a_real_server_build_and_its_manifest_are_read() {
    let entry: Value = serde_json::from_str(include_str!("fixtures/catalog-entry.json")).unwrap();
    let pack = pack(entry);
    assert_eq!((pack.id.as_str(), pack.name.as_str()), ("tensa-lite", "Tensa"));
    assert_eq!(
        (pack.minecraft.as_deref(), pack.loader.as_deref(), pack.loader_version.as_deref()),
        (Some("26.3"), Some("fabric"), Some("0.19.5"))
    );
    assert!(pack.force_update, "it names a force-update endpoint");
    assert_eq!(pack.force_endpoint.as_deref(), Some("https://gigabait.uk/api/mods/tensa-lite/force-update"));
    assert_eq!(pack.server, Some(("play.example".into(), 25565)));
    assert_eq!(pack.jvm_arguments.as_ref().map(Vec::len), Some(4));
    assert_eq!(pack.forced_fields.len(), 7);

    let manifest: Value = serde_json::from_str(include_str!("fixtures/force-update.json")).unwrap();
    module_tensa::backend::manifest::validate(&manifest).unwrap();
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "mods/removed-from-server.jar", "x");
    put(dir.path(), "config/mine.json", "x");
    let plan = plan(dir.path(), &Source::Manifest(manifest), &[], false);
    assert_eq!(plan.managed_dirs, ["mods"]);
    assert_eq!(downloads(&plan), ["mods/badoptimizations.jar", "mods/bettergrassify.jar"]);
    assert_eq!(stale(&plan), ["mods/removed-from-server.jar"]);
    assert!(
        plan.downloads
            .iter()
            .all(|f| f.hash.as_ref().is_some_and(|h| h.kind == HashKind::Sha256) && f.size.is_some())
    );
}

/// Makes `link` a link to the folder `target` (a junction on Windows); false when this system
/// will not.
fn link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

#[test]
fn a_linked_folder_is_never_planned_through_its_link() {
    let dir = tempfile::tempdir().unwrap();
    let (root, shared) = (dir.path().join("build"), dir.path().join("shared"));
    put(&shared, "players-pack.zip", "x");
    put(&root, "mods/old.jar", "x");
    if !link_dir(&shared, &root.join("resourcepacks")) {
        eprintln!("this system makes no folder links; skipped");
        return;
    }
    let manifest = json!({
        "files": [file("resourcepacks/server.zip"), file("mods/a.jar")],
        "directories": [{"path": "resourcepacks"}, {"path": "mods"}]
    });
    let plan = plan(&root, &Source::Manifest(manifest), &[], false);
    assert_eq!(plan.managed_dirs, ["mods"], "the linked folder is not the server's to manage");
    assert_eq!(downloads(&plan), ["mods/a.jar"], "nothing is downloaded through a link");
    assert_eq!(stale(&plan), ["mods/old.jar"]);
    assert!(shared.join("players-pack.zip").is_file());
}

#[test]
fn a_name_the_file_transaction_refuses_is_never_planned() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec![file("mods/x.jar."), file("config/a:b.json"), file("mods/ok.jar")];
    let plan = plan(dir.path(), &Source::Manifest(json!({"files": files, "directories": []})), &[], false);
    assert_eq!(downloads(&plan), ["mods/ok.jar"]);
}

#[cfg(unix)]
#[test]
fn a_players_file_with_a_name_the_transaction_refuses_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "resourcepacks/Pack 1:2.zip", "x");
    put(dir.path(), "resourcepacks/old.zip", "x");
    let source = Source::Manifest(json!({"files": [], "directories": [{"path": "resourcepacks"}]}));
    let plan = plan(dir.path(), &source, &[], false);
    assert_eq!(stale(&plan), ["resourcepacks/old.zip"]);
}
