mod support;

use std::sync::Arc;

use launcher_core::providers::ContentProvider;
use launcher_shared::ErrorCode;
use launcher_shared::provider::{PackArgs, PackInstallArgs, PacksArgs};
use module_tensa::backend::provider::{ServerBuilds, info};
use serde_json::json;
use support::World;

fn three(w: &World) {
    w.server.json(
        "/api/mods",
        json!([
            {"client": {"id": "aero", "name": "Aeronautics", "minecraft_version": "26.3", "loader_id": "fabric",
                        "loader_version": "0.19.5", "description": "Planes", "image": "a.png"}},
            {"client": {"id": "aero_voxy", "name": "Aeronautics (Roxy)", "minecraft_version": "26.3", "loader_id": "fabric"}},
            {"client": {"id": "tensa-lite", "name": "Tensa", "minecraft_version": "1.21.1", "loader_id": "neoforge",
                        "loader_version": "21.1.77", "description": "Light aero-free build"}}
        ]),
    );
}

#[test]
fn the_provider_offers_modpacks_only() {
    let info = info();
    assert_eq!(
        (info.id.as_str(), info.name.as_str(), info.icon.as_str()),
        ("tensa", "TensaCraft", "rocket_launch")
    );
    assert!(info.modpacks && !info.modpack_updates && info.content.is_empty() && info.updates.is_empty());
}

#[tokio::test]
async fn the_catalog_is_a_page_of_modpacks_filtered_by_the_query() {
    let w = World::start().await;
    three(&w);
    let provider = ServerBuilds::new(w.deps.clone());
    let all = provider.modpacks(PacksArgs { query: " ".into(), offset: 0 }).await.unwrap();
    assert_eq!(
        all.hits.iter().map(|h| h.project_id.as_str()).collect::<Vec<_>>(),
        ["aero", "aero_voxy", "tensa-lite"]
    );
    assert_eq!((all.total, all.offset), (3, 0));
    let first = &all.hits[0];
    assert_eq!(
        (first.title.as_str(), first.author.as_str(), first.description.as_str()),
        ("Aeronautics", "Fabric 26.3", "Planes")
    );
    assert_eq!((first.icon_url.as_deref(), first.url.as_deref(), first.downloads), (Some("a.png"), None, 0));
    let found = provider.modpacks(PacksArgs { query: "AERO".into(), offset: 0 }).await.unwrap();
    assert_eq!(found.total, 3, "names, ids and descriptions are searched");
    let roxy = provider.modpacks(PacksArgs { query: "roxy".into(), offset: 0 }).await.unwrap();
    assert_eq!(roxy.hits.iter().map(|h| h.project_id.as_str()).collect::<Vec<_>>(), ["aero_voxy"]);
    let past = provider.modpacks(PacksArgs { query: String::new(), offset: 40 }).await.unwrap();
    assert!(past.hits.is_empty() && past.total == 3);
}

#[tokio::test]
async fn a_server_build_has_one_version_the_current_one() {
    let w = World::start().await;
    three(&w);
    let provider = ServerBuilds::new(w.deps.clone());
    let versions = provider.modpack_versions(PackArgs { project_id: "tensa-lite".into() }).await.unwrap();
    assert_eq!(versions.len(), 1);
    let current = &versions[0];
    assert_eq!((current.id.as_str(), current.version_number.as_str()), ("tensa-lite", "NeoForge 21.1.77"));
    assert_eq!(
        (current.game_versions.as_slice(), current.loaders.as_slice()),
        (&["1.21.1".to_string()][..], &["neoforge".to_string()][..])
    );
    assert_eq!(current.label(), "NeoForge 21.1.77 (1.21.1)");
    let bare = provider.modpack_versions(PackArgs { project_id: "aero_voxy".into() }).await.unwrap();
    assert_eq!(bare[0].version_number, "Fabric");
    let missing = provider.modpack_versions(PackArgs { project_id: "nope".into() }).await.unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn installing_a_modpack_makes_a_server_build() {
    let w = World::start().await;
    w.catalog(
        json!({"id": "aero", "minecraft_version": "26.3", "loader_id": "fabric", "loader_version": "0.19.5",
                     "files_endpoint": w.server.url("/files-list")}),
    );
    w.server.json("/files-list", json!({"files": []}));
    let provider = ServerBuilds::new(Arc::clone(&w.deps));
    let args = PackInstallArgs {
        project_id: "aero".into(),
        version_id: "aero".into(),
        name: "My Aero".into(),
        icon_url: None,
    };
    let installed = provider.install_modpack(args).await.unwrap();
    assert_eq!(installed.name, "My Aero");
    let build = w.deps.versions.get(&installed.key).unwrap();
    assert_eq!(build.remote_pack_id, Some(json!("aero")));
}
