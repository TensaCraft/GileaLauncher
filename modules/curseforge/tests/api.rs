mod support;

use launcher_shared::ErrorCode;
use launcher_shared::branding::APP_NAME;
use module_curseforge::backend::api::{CurseForgeApi, SearchQuery};
use module_curseforge::backend::key::ApiKey;
use serde_json::json;
use support::*;

fn api(server: &FakeCurseForge, key: &str) -> CurseForgeApi {
    CurseForgeApi::new(&server.base, ApiKey::new(key).unwrap()).unwrap()
}

fn sodium(server: &FakeCurseForge) {
    let file = file_json(server, 11, 1, "sodium.jar", b"sodium", &["1.21.1", "Fabric"], json!([]), false);
    server.publish(mod_json(1, "Sodium", MODS, 100), vec![file]);
}

fn pairs(seen: &Seen) -> Vec<(&str, &str)> {
    seen.query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_request_names_the_launcher_and_carries_the_key() {
    let server = FakeCurseForge::start().await;
    sodium(&server);
    let api = api(&server, TEST_KEY);
    let query = SearchQuery {
        class_id: 6,
        text: "sod",
        game_version: Some("1.21.1"),
        loaders: vec![4],
        index: 0,
        page_size: 16,
    };
    let page = api.search(&query).await.unwrap();
    assert_eq!(page["data"][0]["id"], 1);
    let seen = server.seen();
    assert_eq!(seen[0].key.as_deref(), Some(TEST_KEY));
    assert!(seen[0].agent.as_deref().unwrap().starts_with(APP_NAME), "{:?}", seen[0].agent);
    assert_eq!(
        pairs(&seen[0]),
        [
            ("classId", "6"),
            ("gameId", "432"),
            ("gameVersion", "1.21.1"),
            ("index", "0"),
            ("modLoaderType", "4"),
            ("pageSize", "16"),
            ("searchFilter", "sod"),
            ("sortField", "2"),
            ("sortOrder", "desc"),
        ]
    );
    // A Quilt build searches Fabric's mods too, in one list.
    api.search(&SearchQuery { loaders: vec![5, 4], ..query.clone() }).await.unwrap();
    let seen = server.seen();
    let asked = pairs(&seen[1]);
    assert!(
        asked.contains(&("modLoaderTypes", "[5,4]")) && !asked.iter().any(|(k, _)| *k == "modLoaderType")
    );
    api.search(&SearchQuery { text: "  ", game_version: None, loaders: Vec::new(), ..query }).await.unwrap();
    assert!(
        !pairs(&server.seen()[2]).iter().any(|(k, _)| [
            "searchFilter",
            "gameVersion",
            "modLoaderType",
            "modLoaderTypes"
        ]
        .contains(k)),
        "no text, no version, no loader: no filters"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_key_turns_curseforge_off_for_the_session() {
    let server = FakeCurseForge::start().await;
    sodium(&server);
    let api = api(&server, "revoked-key");
    let refused = api.mod_info(1).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::ProviderKeyRejected);
    assert_eq!(refused.params["provider"], "CurseForge");
    assert!(api.rejected());
    let again = api.files(1, None, None).await.unwrap_err();
    assert_eq!(again.code, ErrorCode::ProviderKeyRejected);
    assert_eq!(server.seen().len(), 1, "no request after the key was refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn server_errors_and_garbage_are_network_errors() {
    let server = FakeCurseForge::start().await;
    sodium(&server);
    server.data.lock().unwrap().status.insert("/v1/mods/1".into(), 500);
    let api = api(&server, TEST_KEY);
    let down = api.mod_info(1).await.unwrap_err();
    assert_eq!((down.code, down.params["status"].as_str()), (ErrorCode::Network, "500"));
    server.data.lock().unwrap().raw.insert("/v1/mods/1/files".into(), "not json".into());
    assert_eq!(api.files(1, None, None).await.unwrap_err().code, ErrorCode::Network);
    assert!(!api.rejected());
}

#[tokio::test(flavor = "multi_thread")]
async fn all_files_of_a_mod_come_page_by_page() {
    let server = FakeCurseForge::start().await;
    let files: Vec<_> = (0..120)
        .map(|i| {
            let name = format!("big-{i}.jar");
            file_json(&server, 1000 + i, 7, &name, name.as_bytes(), &["1.21.1", "Fabric"], json!([]), false)
        })
        .collect();
    server.publish(mod_json(7, "Big", MODS, 1), files);
    let api = api(&server, TEST_KEY);
    let all = api.files(7, Some("1.21.1"), Some(4)).await.unwrap();
    assert_eq!(all.len(), 120);
    let mut indexes: Vec<String> =
        server.seen().iter().map(|s| s.query.iter().find(|(k, _)| k == "index").unwrap().1.clone()).collect();
    // The pages after the first are asked side by side, in any order.
    indexes.sort_by_key(|i| i.parse::<u32>().unwrap());
    assert_eq!(indexes, ["0", "50", "100"]);
    let ids: Vec<u64> = all.iter().map(|f| f["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, (1000..1120).collect::<Vec<_>>(), "the files in CurseForge's order");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_found_by_their_fingerprints() {
    let server = FakeCurseForge::start().await;
    sodium(&server);
    let api = api(&server, TEST_KEY);
    let print = module_curseforge::backend::fingerprint::fingerprint(b"sodium");
    let found = api.fingerprints(&[print, 7]).await.unwrap();
    assert_eq!(found["exactMatches"][0]["file"]["id"], 11);
    assert_eq!(server.seen()[0].body.as_ref().unwrap()["fingerprints"], json!([print, 7]));
    assert!(api.fingerprints(&[]).await.unwrap()["exactMatches"].as_array().unwrap().is_empty());
    assert_eq!(server.seen().len(), 1, "nothing to ask, no request");
}

#[tokio::test(flavor = "multi_thread")]
async fn mods_and_files_come_in_batches() {
    let server = FakeCurseForge::start().await;
    sodium(&server);
    let api = api(&server, TEST_KEY);
    let mods = api.mods(&[1, 2]).await.unwrap();
    assert_eq!(mods.len(), 1, "an unknown id is left out");
    let files = api.files_by_ids(&[11]).await.unwrap();
    assert_eq!(files[0]["fileName"], "sodium.jar");
    let seen = server.seen();
    assert_eq!(seen[0].body.as_ref().unwrap()["modIds"], json!([1, 2]));
    assert_eq!(seen[1].body.as_ref().unwrap()["fileIds"], json!([11]));
    assert!(api.mods(&[]).await.unwrap().is_empty());
    assert_eq!(server.seen().len(), 2, "nothing to ask, no request");
}
