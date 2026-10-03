mod support;

use launcher_shared::provider::{Action, Pick, PlanDto};
use launcher_shared::{ContentKind, ErrorCode};
use module_curseforge::backend::api::CurseForgeApi;
use module_curseforge::backend::key::ApiKey;
use module_curseforge::backend::resolver::{self, Installed, InstalledFile, Target};
use serde_json::{Value, json};
use support::*;

const FABRIC: &[&str] = &["1.21.1", "Fabric"];

fn api(server: &FakeCurseForge) -> CurseForgeApi {
    CurseForgeApi::new(&server.base, ApiKey::new(TEST_KEY).unwrap()).unwrap()
}

fn mods_target(installed: &Installed) -> Target<'_> {
    Target {
        kind: ContentKind::Mods,
        game_version: Some("1.21.1"),
        loader: Some("fabric"),
        installed,
        finder: None,
    }
}

/// `relationType`: 1 embedded, 2 optional, 3 required, 5 incompatible.
fn deps(list: &[(u64, u64)]) -> Value {
    Value::Array(list.iter().map(|(id, rel)| json!({"modId": id, "relationType": rel})).collect())
}

/// Mod `id` named `name` with one Fabric file `id * 10` that depends on `dependencies`.
fn publish(server: &FakeCurseForge, id: u64, name: &str, dependencies: &[(u64, u64)]) {
    let file = file_json(
        server,
        id * 10,
        id,
        &format!("{name}.jar"),
        name.as_bytes(),
        FABRIC,
        deps(dependencies),
        false,
    );
    server.publish(mod_json(id, name, MODS, 100), vec![file]);
}

async fn plan(server: &FakeCurseForge, target: &Target<'_>, id: u64, picks: &[Pick]) -> PlanDto {
    resolver::plan(&api(server), target, id, None, picks).await.unwrap().to_dto()
}

fn titles(items: &[launcher_shared::provider::PlanItem]) -> Vec<&str> {
    items.iter().map(|i| i.title.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn required_dependencies_come_along_however_deep() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create", &[(2, 3)]);
    publish(&server, 2, "Flywheel", &[(3, 3)]);
    publish(&server, 3, "Architectury", &[]);
    let installed = Installed::default();
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    let main = dto.main.as_ref().unwrap();
    assert_eq!(
        (main.title.as_str(), main.version_id.as_str(), main.action),
        ("Create", "10", Action::Install)
    );
    assert_eq!(main.url.as_deref(), Some("https://www.curseforge.com/minecraft/mc-mods/create"));
    assert_eq!(titles(&dto.install), ["Flywheel", "Architectury"]);
    assert!(dto.can_install() && dto.blocking.is_empty());
    let order: Vec<String> = dto.changes(&[]).into_iter().map(|c| c.project_id).collect();
    assert_eq!(order, ["2", "3", "1"], "dependencies first, the project last");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dependencies_of_a_dependency_are_asked_about_together() {
    // One request for the projects a dependency's file needs, not one for each of them.
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create", &[(2, 3)]);
    publish(&server, 2, "Flywheel", &[(3, 3), (4, 3), (5, 3)]);
    publish(&server, 3, "Architectury", &[]);
    publish(&server, 4, "Ponder", &[]);
    publish(&server, 5, "Catnip", &[]);
    let installed = Installed::default();
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    assert_eq!(titles(&dto.install), ["Flywheel", "Architectury", "Ponder", "Catnip"]);
    let asked = server.seen().into_iter().filter(|s| s.path == "/v1/mods").count();
    assert_eq!(asked, 2, "Create's dependency, then Flywheel's three at once");
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_projects_are_kept_or_replaced() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create", &[(2, 3)]);
    publish(&server, 2, "Flywheel", &[]);
    let mut installed = Installed::default();
    installed.by_project.insert(
        2,
        InstalledFile {
            file_id: 20,
            filename: "Flywheel.jar".into(),
            version: "Flywheel.jar".into(),
            relative: "mods/Flywheel.jar".into(),
            date: None,
            release_type: None,
        },
    );
    installed.by_project.insert(
        1,
        InstalledFile {
            file_id: 9,
            filename: "create-old.jar".into(),
            version: "Create 0.9".into(),
            relative: "mods/create-old.jar".into(),
            date: None,
            release_type: None,
        },
    );
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    let main = dto.main.as_ref().unwrap();
    assert_eq!((main.action, main.current.as_deref()), (Action::Replace, Some("0.9")));
    assert_eq!(titles(&dto.satisfied), ["Flywheel"], "the same file stays");
    assert!(dto.install.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_embedded_and_incompatible_dependencies() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Sodium Extra", &[(2, 2), (3, 1), (4, 5)]);
    publish(&server, 2, "Reese", &[(5, 3)]);
    publish(&server, 3, "Embedded Lib", &[]);
    publish(&server, 4, "OptiFabric", &[]);
    publish(&server, 5, "Reese Lib", &[]);
    let installed = Installed::default();
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    assert_eq!(titles(&dto.optional), ["Reese"]);
    assert_eq!(dto.embedded.iter().map(|i| i.name.clone().unwrap()).collect::<Vec<_>>(), ["Embedded Lib"]);
    assert!(dto.blocking.is_empty(), "an incompatible mod that is not installed is no problem");
    let pick = Pick { project_id: "2".into(), version_id: "20".into() };
    let picked = plan(&server, &mods_target(&installed), 1, &[pick]).await;
    assert_eq!(
        titles(&picked.install),
        ["Reese", "Reese Lib"],
        "a picked option brings its own dependencies"
    );
    let mut with_optifabric = Installed::default();
    with_optifabric.by_project.insert(
        4,
        InstalledFile {
            file_id: 40,
            filename: "OptiFabric.jar".into(),
            version: "x".into(),
            relative: "mods/OptiFabric.jar".into(),
            date: None,
            release_type: None,
        },
    );
    let clash = plan(&server, &mods_target(&with_optifabric), 1, &[]).await;
    assert_eq!(clash.blocking[0].code, "incompatible_installed");
    assert_eq!(clash.blocking[0].name.as_deref(), Some("OptiFabric"));
    assert!(!clash.can_install());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dependency_without_a_file_or_a_blocked_file_stops_the_plan() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create", &[(2, 3), (3, 3)]);
    let forge_only =
        file_json(&server, 20, 2, "lib-forge.jar", b"lib", &["1.21.1", "Forge"], json!([]), false);
    server.publish(mod_json(2, "Forge Lib", MODS, 1), vec![forge_only]);
    let held = file_json(&server, 30, 3, "held.jar", b"held", FABRIC, json!([]), true);
    let mut held_mod = mod_json(3, "Held Lib", MODS, 1);
    held_mod["allowModDistribution"] = json!(false);
    server.publish(held_mod, vec![held]);
    let installed = Installed::default();
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    let codes: Vec<(&str, Option<&str>)> =
        dto.blocking.iter().map(|i| (i.code.as_str(), i.name.as_deref())).collect();
    assert_eq!(codes, [("dependency_no_file", Some("Forge Lib")), ("file_blocked", Some("Held Lib"))]);
    assert_eq!(
        dto.blocking[1].url.as_deref(),
        Some("https://www.curseforge.com/minecraft/mc-mods/held-lib/files/30"),
        "the page where it is downloaded by hand"
    );
    assert!(!dto.can_install());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_project_shows_its_page_and_a_project_without_a_file_is_an_error() {
    let server = FakeCurseForge::start().await;
    let held = file_json(&server, 10, 1, "skins.jar", b"skins", FABRIC, json!([]), true);
    server.publish(mod_json(1, "Skin Layers", MODS, 1), vec![held]);
    let installed = Installed::default();
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    assert!(dto.main.is_some());
    assert_eq!(dto.blocking[0].code, "file_blocked");
    assert!(!dto.can_install());
    let forge_only = file_json(&server, 20, 2, "f.jar", b"f", &["1.21.1", "Forge"], json!([]), false);
    server.publish(mod_json(2, "Forge Only", MODS, 1), vec![forge_only]);
    let none = resolver::plan(&api(&server), &mods_target(&installed), 2, None, &[]).await.unwrap_err();
    assert_eq!((none.code, none.params["name"].as_str()), (ErrorCode::NoCompatibleVersion, "Forge Only"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_file_the_user_saw_is_kept_and_packs_need_no_loader() {
    let server = FakeCurseForge::start().await;
    let newer = file_json(&server, 12, 1, "new.jar", b"new", FABRIC, json!([]), false);
    let older = file_json(&server, 11, 1, "old.jar", b"old", FABRIC, json!([]), false);
    server.publish(mod_json(1, "Create", MODS, 1), vec![newer, older]);
    let installed = Installed::default();
    let exact =
        resolver::plan(&api(&server), &mods_target(&installed), 1, Some(11), &[]).await.unwrap().to_dto();
    assert_eq!(exact.main.unwrap().version_id, "11");
    let pack = file_json(&server, 50, 5, "faithful.zip", b"pack", &["1.21.1"], json!([]), false);
    server.publish(mod_json(5, "Faithful", RESOURCE_PACKS, 1), vec![pack]);
    let packs = Target {
        kind: ContentKind::ResourcePacks,
        game_version: Some("1.21.1"),
        loader: None,
        installed: &installed,
        finder: None,
    };
    let dto = plan(&server, &packs, 5, &[]).await;
    assert_eq!(dto.main.unwrap().filename, "faithful.zip");
    let asked = server.seen().into_iter().rfind(|s| s.path == "/v1/mods/5/files").unwrap();
    assert!(!asked.query.iter().any(|(k, _)| k == "modLoaderType"), "a pack is not a mod of a loader");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_file_in_the_build_is_never_replaced_by_an_older_one() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create", &[]);
    let mut installed = Installed::default();
    // The build has a newer beta than the release CurseForge offers (file 10, dated 2026-09-11).
    installed.by_project.insert(
        1,
        InstalledFile {
            file_id: 99,
            filename: "create-beta.jar".into(),
            version: "Create 2.0 beta".into(),
            relative: "mods/create-beta.jar".into(),
            date: Some("2026-10-01T00:00:00Z".into()),
            release_type: None,
        },
    );
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    let main = dto.main.as_ref().unwrap();
    assert_eq!((main.action, main.version_id.as_str()), (Action::Satisfied, "99"), "the newer file stays");
    assert!(dto.changes(&[]).is_empty());
    installed.by_project.get_mut(&1).unwrap().date = Some("2026-01-01T00:00:00Z".into());
    let older = plan(&server, &mods_target(&installed), 1, &[]).await;
    assert_eq!(older.main.unwrap().action, Action::Replace, "an older file is updated");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jar_with_the_project_s_mod_id_counts_as_the_project() {
    let server = FakeCurseForge::start().await;
    publish(&server, 1, "Create Additions", &[(2, 3)]);
    publish(&server, 2, "Create", &[]);
    let mut installed = Installed::default();
    // create.jar from elsewhere: CurseForge does not know its fingerprint; its mod id is the slug.
    installed.by_mod_id.insert(
        "create".into(),
        InstalledFile {
            filename: "create.jar".into(),
            version: "6.0.9".into(),
            relative: "mods/create.jar".into(),
            ..InstalledFile::default()
        },
    );
    let dto = plan(&server, &mods_target(&installed), 1, &[]).await;
    assert_eq!(titles(&dto.satisfied), ["Create"], "the dependency is there");
    assert!(dto.install.is_empty());
    let own = plan(&server, &mods_target(&installed), 2, &[]).await;
    let main = own.main.unwrap();
    assert_eq!(
        (main.action, main.unrecognized, main.current.as_deref()),
        (Action::Replace, true, Some("6.0.9"))
    );
}
