//! One copy of a mod (owner's report, 2026-09-29): an older Sodium was in the build, a mod needing
//! Sodium brought a newer one, and the build ended up with two.

mod support;

use std::fs;
use std::path::Path;

use launcher_shared::provider::{InstallAnswer, InstallArgs, PlanItem};
use launcher_shared::{ContentKind, ErrorCode};
use module_modrinth::backend::resolver::FABRIC_API;
use serde_json::json;
use support::*;

const FABRIC: &str = "fabric-loader-0.16.9-1.21.1";
const SODIUM: &str = "AANobbMI";

fn args(key: &str, id: &str, title: &str) -> InstallArgs {
    InstallArgs::new(key, ContentKind::Mods, id, id, title)
}

fn ids(items: &[PlanItem]) -> Vec<&str> {
    items.iter().map(|i| i.project_id.as_str()).collect()
}

fn project(id: &str, slug: &str, title: &str) -> serde_json::Value {
    json!({"id": id, "slug": slug, "title": title, "project_type": "mod"})
}

/// Fabric API, Sodium (an old and a new version, both real jars of mod `sodium`), and Reese's
/// Sodium Options needing Sodium the way `sodium_dependency` says.
fn catalog(w: &World, sodium_dependency: serde_json::Value) -> (serde_json::Value, serde_json::Value) {
    let s = &w.server;
    s.publish(
        project_json(FABRIC_API, "Fabric API"),
        vec![release(s, FABRIC_API, "fapi-1", &["fabric"], 2, json!([]))],
    );
    let (old, new) = (
        release_jar(s, SODIUM, "sodium-0.5", "sodium", 1, json!([])),
        release_jar(s, SODIUM, "sodium-0.6", "sodium", 6, json!([])),
    );
    s.publish(project(SODIUM, "sodium", "Sodium"), vec![new.clone(), old.clone()]);
    s.publish(
        project("reeses", "reeses-sodium-options", "Reese's Sodium Options"),
        vec![release_jar(s, "reeses", "reeses-1", "reeses-sodium-options", 7, json!([sodium_dependency]))],
    );
    (old, new)
}

/// The jars in `mods/` whose `fabric.mod.json` says `id`.
fn copies(dir: &Path, id: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir.join("mods")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".jar") {
            continue;
        }
        if let Ok(meta) = launcher_core::content::jar::inspect_mod_jar(&entry.path(), None)
            && meta.mod_id == id
        {
            found.push(name);
        }
    }
    found.sort();
    found
}

async fn install(w: &World, args: InstallArgs) -> Result<InstallAnswer, launcher_shared::AppError> {
    let plan = w.service.plan(&args).await?;
    w.service.install(&args.approve(&plan, &[])).await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_older_copy_of_a_dependency_is_replaced_not_duplicated() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    catalog(&w, json!({"project_id": SODIUM, "dependency_type": "required"}));
    fs::create_dir_all(dir.join("mods")).unwrap();
    // An older Sodium Modrinth does not know (built or downloaded elsewhere).
    fs::write(dir.join("mods/sodium-fabric-0.5.8.jar"), mod_jar("sodium", "Sodium", "elsewhere")).unwrap();
    let plan = w.service.plan(&args(&key, "reeses", "Reese's Sodium Options")).await.unwrap();
    assert_eq!(ids(&plan.replace), [SODIUM], "the copy there is replaced, not joined");
    assert_eq!(plan.replace[0].current.as_deref(), Some("sodium-fabric-0.5.8.jar"));
    assert_eq!(ids(&plan.install), [FABRIC_API]);
    let answer = install(&w, args(&key, "reeses", "Reese's Sodium Options")).await.unwrap();
    assert!(matches!(answer, InstallAnswer::Installed(_)), "{answer:?}");
    assert_eq!(copies(&dir, "sodium"), ["sodium-0.6.jar"], "one Sodium");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_known_older_copy_is_replaced_by_the_required_version() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let (old, _) = catalog(&w, json!({"version_id": "sodium-0.6", "dependency_type": "required"}));
    fs::create_dir_all(dir.join("mods")).unwrap();
    let old_body = mod_jar("sodium", "sodium", "sodium-0.5");
    fs::write(dir.join("mods/sodium-0.5.jar"), &old_body).unwrap();
    w.server.know(&old_body, &old);
    install(&w, args(&key, "reeses", "Reese's Sodium Options")).await.unwrap();
    assert_eq!(copies(&dir, "sodium"), ["sodium-0.6.jar"], "one Sodium");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_similar_name_is_not_the_same_mod() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    catalog(&w, json!({"project_id": SODIUM, "dependency_type": "required"}));
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-extra-0.5.jar"), mod_jar("sodium-extra", "Sodium Extra", "x")).unwrap();
    let plan = w.service.plan(&args(&key, "reeses", "Reese's Sodium Options")).await.unwrap();
    assert_eq!(ids(&plan.install), [SODIUM, FABRIC_API], "Sodium Extra is not Sodium");
    assert!(plan.replace.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_copies_of_a_dependency_block_the_plan() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let (old, _) = catalog(&w, json!({"project_id": SODIUM, "dependency_type": "required"}));
    fs::create_dir_all(dir.join("mods")).unwrap();
    let known = mod_jar("sodium", "sodium", "sodium-0.5");
    fs::write(dir.join("mods/sodium-0.5.jar"), &known).unwrap();
    w.server.know(&known, &old);
    fs::write(dir.join("mods/sodium-custom.jar"), mod_jar("sodium", "Sodium", "mine")).unwrap();
    let plan = w.service.plan(&args(&key, "reeses", "Reese's Sodium Options")).await.unwrap();
    assert!(!plan.can_install(), "which copy to keep is the user's call");
    let issue = plan.blocking.iter().find(|i| i.code == "dependency_duplicate_copies").expect("says why");
    let files = issue.file_name.clone().unwrap_or_default();
    assert!(
        files.contains("sodium-0.5.jar") && files.contains("sodium-custom.jar"),
        "names the copies: {files}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_staged_jar_with_an_installed_mod_id_is_refused() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    s.publish(
        project_json(FABRIC_API, "Fabric API"),
        vec![release(s, FABRIC_API, "fapi-1", &["fabric"], 2, json!([]))],
    );
    // The project's slug and title do not name its jar's mod id: no hint finds the copy there.
    s.publish(
        project("cloth", "cloth-config", "Cloth Config API"),
        vec![release_jar(s, "cloth", "cloth-15", "cloth-config2", 3, json!([]))],
    );
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/cloth-config-14.jar"), mod_jar("cloth-config2", "Cloth Config v14", "old"))
        .unwrap();
    let e = install(&w, args(&key, "cloth", "Cloth Config API")).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ContentConflict);
    assert_eq!(e.params.get("name").map(String::as_str), Some("cloth-config-14.jar"));
    assert_eq!(copies(&dir, "cloth-config2"), ["cloth-config-14.jar"], "nothing was added");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hinted_copy_of_the_project_itself_asks_before_it_is_replaced() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    catalog(&w, json!({"project_id": SODIUM, "dependency_type": "required"}));
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/sodium-fabric-0.5.8.jar"), mod_jar("sodium", "Sodium", "elsewhere")).unwrap();
    // Installing Sodium itself from the search: the copy there is not silently swapped.
    let plan =
        w.service.plan(&InstallArgs::new(&key, ContentKind::Mods, SODIUM, "sodium", "Sodium")).await.unwrap();
    let main = plan.main.as_ref().unwrap();
    assert_eq!(main.action, launcher_shared::provider::Action::Replace);
    assert!(main.unrecognized, "the file it replaces was found by its name");
    assert_eq!(main.current.as_deref(), Some("sodium-fabric-0.5.8.jar"));
    assert!(plan.requires_confirmation(), "the user sees the replacement first");
}
