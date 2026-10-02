mod support;

use std::fs;

use launcher_shared::provider::{Action, InstallArgs, Pick, PlanItem};
use launcher_shared::{ContentKind, ErrorCode};
use module_modrinth::backend::resolver::FABRIC_API;
use serde_json::json;
use support::*;

const FABRIC: &str = "fabric-loader-0.16.9-1.21.1";

fn args(key: &str, id: &str, title: &str) -> InstallArgs {
    InstallArgs::new(key, ContentKind::Mods, id, id, title)
}

fn ids(items: &[PlanItem]) -> Vec<&str> {
    items.iter().map(|i| i.project_id.as_str()).collect()
}

fn fabric_api(w: &World) {
    let s = &w.server;
    s.publish(
        project_json(FABRIC_API, "Fabric API"),
        vec![release(s, FABRIC_API, "fapi-1", &["fabric"], 2, json!([]))],
    );
}

fn required(project: &str) -> serde_json::Value {
    json!({"project_id": project, "dependency_type": "required"})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fabric_mod_brings_its_dependencies_and_fabric_api() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
    s.publish(
        project_json("extra", "Sodium Extra"),
        vec![release(s, "extra", "extra-2", &["fabric"], 5, json!([required("sodium")]))],
    );
    // Listed out of date order: the newest by date wins.
    s.publish(
        project_json("sodium", "Sodium"),
        vec![
            release(s, "sodium", "sodium-1", &["fabric"], 1, json!([])),
            release(s, "sodium", "sodium-3", &["fabric"], 6, json!([])),
            release(s, "sodium", "sodium-2", &["fabric"], 4, json!([])),
        ],
    );
    let plan = w.service.plan(&args(&key, "extra", "Sodium Extra")).await.unwrap();
    assert_eq!(plan.main.as_ref().map(|m| m.version_id.as_str()), Some("extra-2"));
    assert_eq!(ids(&plan.install), ["sodium", FABRIC_API]);
    assert_eq!(plan.install[0].version_id, "sodium-3");
    assert_eq!(plan.install[1].url.as_deref(), Some("https://modrinth.com/mod/p7dr8msh"));
    assert!(plan.requires_confirmation() && plan.can_install());
    assert!(plan.blocking.is_empty() && plan.optional.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_embedded_and_incompatible_dependencies() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
    let deps = json!([
        {"project_id": "sodium", "dependency_type": "optional"},
        {"project_id": "indium", "dependency_type": "embedded"},
        {"project_id": "optifine", "dependency_type": "incompatible"},
        {"project_id": "other", "dependency_type": "incompatible"},
        {"project_id": "weird", "dependency_type": "suggested"}
    ]);
    s.publish(project_json("iris", "Iris"), vec![release(s, "iris", "iris-1", &["fabric"], 3, deps)]);
    s.publish(
        project_json("sodium", "Sodium"),
        vec![release(s, "sodium", "sodium-2", &["fabric"], 4, json!([]))],
    );
    s.publish(project_json("indium", "Indium"), vec![]);
    let optifine = release(s, "optifine", "of-1", &["fabric"], 1, json!([]));
    s.publish(project_json("optifine", "OptiFine"), vec![optifine.clone()]);
    // OptiFine is in the build (Modrinth knows its file); "other" is neither there nor planned.
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/optifine.jar"), b"of-1").unwrap();
    s.know(b"of-1", &optifine);
    let plan = w.service.plan(&args(&key, "iris", "Iris")).await.unwrap();
    assert_eq!(ids(&plan.optional), ["sodium"]);
    assert_eq!(
        plan.embedded.iter().map(|i| (i.name.as_deref(), i.blocking)).collect::<Vec<_>>(),
        [(Some("Indium"), false)]
    );
    assert_eq!(
        plan.blocking.iter().map(|i| (i.code.as_str(), i.name.as_deref())).collect::<Vec<_>>(),
        [("incompatible_installed", Some("OptiFine"))]
    );
    assert_eq!(
        plan.optional_issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
        ["unsupported_dependency_type"]
    );
    assert!(!plan.can_install());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_optional_pick_joins_the_install() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
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
    s.publish(
        project_json("sodium", "Sodium"),
        vec![release(s, "sodium", "sodium-2", &["fabric"], 4, json!([]))],
    );
    let offered = w.service.plan(&args(&key, "iris", "Iris")).await.unwrap();
    assert_eq!((ids(&offered.optional), ids(&offered.install)), (vec!["sodium"], vec![FABRIC_API]));
    let picked = InstallArgs {
        version_id: Some("iris-1".into()),
        optional: vec![Pick { project_id: "sodium".into(), version_id: "sodium-2".into() }],
        ..args(&key, "iris", "Iris")
    };
    let plan = w.service.plan(&picked).await.unwrap();
    assert_eq!(ids(&plan.install), ["sodium", FABRIC_API]);
    assert!(plan.optional.is_empty());
    let sorted = |mut changes: Vec<launcher_shared::provider::Change>| {
        changes.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        changes
    };
    assert_eq!(
        sorted(plan.changes(&[])),
        sorted(offered.changes(&offered.optional)),
        "what the user saw is what is installed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_versions_that_disagree_block_the_install() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
    let main_deps =
        json!([{"project_id": "lib", "version_id": "lib-1", "dependency_type": "required"}, required("b")]);
    s.publish(project_json("main", "Main"), vec![release(s, "main", "main-1", &["fabric"], 5, main_deps)]);
    let b_deps = json!([{"project_id": "lib", "version_id": "lib-2", "dependency_type": "required"}]);
    s.publish(project_json("b", "B"), vec![release(s, "b", "b-1", &["fabric"], 4, b_deps)]);
    s.publish(
        project_json("lib", "Lib"),
        vec![
            release(s, "lib", "lib-2", &["fabric"], 3, json!([])),
            release(s, "lib", "lib-1", &["fabric"], 1, json!([])),
        ],
    );
    let plan = w.service.plan(&args(&key, "main", "Main")).await.unwrap();
    assert_eq!(
        plan.blocking.iter().map(|i| (i.code.as_str(), i.name.as_deref())).collect::<Vec<_>>(),
        [("dependency_version_conflict", Some("Lib"))]
    );
    assert_eq!(
        plan.install.iter().find(|i| i.project_id == "lib").map(|i| i.version_id.as_str()),
        Some("lib-1")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dependencies_that_cannot_be_resolved_block_with_a_reason() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
    let deps = json!([
        required("gone"),
        {"file_name": "legacy.jar", "dependency_type": "required"},
        required("nofile"),
        required("old"),
        {"version_id": "missing-version", "dependency_type": "required"}
    ]);
    s.publish(project_json("main", "Main"), vec![release(s, "main", "main-1", &["fabric"], 5, deps)]);
    let unsafe_file = json!({"id": "nofile-1", "project_id": "nofile", "version_number": "1", "game_versions": ["1.21.1"],
        "loaders": ["fabric"], "date_published": "2026-09-01T00:00:00Z",
        "files": [file_entry("http://example.com/x.jar", "x.jar", b"x", true)]});
    s.publish(project_json("nofile", "No File"), vec![unsafe_file]);
    let old = json!({"id": "old-1", "project_id": "old", "version_number": "1", "game_versions": ["1.20.1"],
        "loaders": ["fabric"], "date_published": "2026-09-01T00:00:00Z", "files": []});
    s.publish(project_json("old", "Old"), vec![old]);
    let plan = w.service.plan(&args(&key, "main", "Main")).await.unwrap();
    let blocking: Vec<(&str, Option<&str>, Option<&str>)> =
        plan.blocking.iter().map(|i| (i.code.as_str(), i.name.as_deref(), i.file_name.as_deref())).collect();
    assert_eq!(
        blocking,
        [
            ("dependency_resolution_failed", Some("gone"), None),
            ("required_file_only", Some("legacy.jar"), Some("legacy.jar")),
            ("dependency_no_file", Some("No File"), None),
            ("dependency_resolution_failed", Some("Old"), None),
            ("dependency_resolution_failed", Some("missing-version"), None),
        ]
    );
    assert!(plan.blocking.iter().all(|i| i.blocking) && !plan.can_install());
    assert_eq!(plan.blocking[0].url, None, "a project Modrinth does not know has no page");
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_dependencies_are_satisfied_replaced_or_installed() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    let fapi = release(s, FABRIC_API, "fapi-1", &["fabric"], 2, json!([]));
    s.publish(project_json(FABRIC_API, "Fabric API"), vec![fapi.clone()]);
    let deps = json!([required("a"), required("b"), required("c"), required("d"),
        {"project_id": "e", "version_id": "e-2", "dependency_type": "required"}]);
    s.publish(project_json("main", "Main"), vec![release(s, "main", "main-1", &["fabric"], 9, deps)]);
    let (a2, b1, c2) = (
        release(s, "a", "a-2", &["fabric"], 4, json!([])),
        release(s, "b", "b-1", &["fabric"], 1, json!([])),
        release(s, "c", "c-2", &["fabric"], 4, json!([])),
    );
    s.publish(project_json("a", "A"), vec![a2.clone(), release(s, "a", "a-1", &["fabric"], 1, json!([]))]);
    s.publish(project_json("b", "B"), vec![release(s, "b", "b-2", &["fabric"], 5, json!([])), b1.clone()]);
    s.publish(project_json("c", "C"), vec![c2.clone()]);
    s.publish(project_json("d", "D"), vec![release(s, "d", "d-1", &["fabric"], 4, json!([]))]);
    let e1 = release(s, "e", "e-1", &["fabric"], 1, json!([]));
    s.publish(project_json("e", "E"), vec![release(s, "e", "e-2", &["fabric"], 5, json!([])), e1.clone()]);
    fs::create_dir_all(dir.join("mods")).unwrap();
    for (file, body, version) in [
        ("mods/a-2.jar", "a-2", &a2),
        ("mods/b-1.jar", "b-1", &b1),
        ("mods/c-2.jar.disabled", "c-2", &c2),
        ("mods/fapi.jar", "fapi-1", &fapi),
        ("mods/e-1.jar", "e-1", &e1),
    ] {
        fs::write(dir.join(file), body).unwrap();
        s.know(body.as_bytes(), version);
    }
    fs::write(dir.join("mods/d.jar"), b"made by hand").unwrap();
    let plan = w.service.plan(&args(&key, "main", "Main")).await.unwrap();
    // An installed compatible version satisfies a dependency (b stays at b-1); a switched-off file
    // or a pin to another version is replaced.
    assert_eq!(ids(&plan.satisfied), ["a", "b", FABRIC_API]);
    assert_eq!(ids(&plan.replace), ["c", "e"]);
    assert_eq!(plan.replace[1].current.as_deref(), Some("e-1"));
    assert_eq!(plan.replace[0].action, Action::Replace);
    assert_eq!(ids(&plan.install), ["d"], "a file Modrinth does not know is not the project's");
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_packs_plan_only_themselves() {
    let w = world().await;
    let (key, _) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    let pack = json!({"id": "fa-1", "project_id": "faith", "version_number": "1", "game_versions": ["1.21.1"],
        "loaders": ["minecraft"], "date_published": "2026-09-01T00:00:00Z",
        "dependencies": [{"project_id": "other", "dependency_type": "required"}],
        "files": [file_entry(&s.file("faith.zip", b"pack"), "faith.zip", b"pack", true)]});
    s.publish(project_json("faith", "Faithful"), vec![pack]);
    let plan = w
        .service
        .plan(&InstallArgs::new(&key, ContentKind::ResourcePacks, "faith", "faith", "Faithful"))
        .await
        .unwrap();
    assert!(plan.install.is_empty() && plan.blocking.is_empty() && !plan.requires_confirmation());
    assert_eq!(plan.changes(&[]).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_owned_copies_of_the_project_block_the_plan() {
    let w = world().await;
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let s = &w.server;
    fabric_api(&w);
    let (one, two) = (
        release(s, "sodium", "sodium-1", &["fabric"], 1, json!([])),
        release(s, "sodium", "sodium-2", &["fabric"], 2, json!([])),
    );
    s.publish(project_json("sodium", "Sodium"), vec![two.clone(), one.clone()]);
    fs::create_dir_all(dir.join("mods")).unwrap();
    fs::write(dir.join("mods/s1.jar"), b"sodium-1").unwrap();
    fs::write(dir.join("mods/s2.jar"), b"sodium-2").unwrap();
    s.know(b"sodium-1", &one);
    s.know(b"sodium-2", &two);
    let plan = w.service.plan(&args(&key, "sodium", "Sodium")).await.unwrap();
    assert!(plan.main.is_none() && !plan.can_install());
    assert_eq!(
        plan.blocking.iter().map(|i| (i.code.as_str(), i.name.as_deref())).collect::<Vec<_>>(),
        [("dependency_duplicate_copies", Some("Sodium"))],
        "the copies are named, so the user knows which to remove"
    );
    assert_eq!(plan.blocking[0].file_name.as_deref(), Some("s1.jar, s2.jar"));
    s.publish(project_json("ghost", "Ghost"), vec![]);
    let none = w.service.plan(&args(&key, "ghost", "Ghost")).await.unwrap_err();
    assert_eq!(
        (none.code, none.params.get("name").map(String::as_str)),
        (ErrorCode::NoCompatibleVersion, Some("Ghost"))
    );
}
