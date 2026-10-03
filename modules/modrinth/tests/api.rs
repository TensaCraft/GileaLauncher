mod support;

use launcher_shared::ErrorCode;
use module_modrinth::backend::api::{ModrinthApi, user_agent};
use serde_json::json;
use support::{FakeModrinth, pairs};

#[tokio::test(flavor = "multi_thread")]
async fn search_asks_modrinth_with_its_user_agent() {
    let server = FakeModrinth::start().await;
    server.data.lock().unwrap().search = Some(json!({"hits": [], "total_hits": 0}));
    let api = ModrinthApi::new(&server.base).unwrap();
    let facets = r#"[["project_type:mod"]]"#;
    assert_eq!(api.search("sod ium", facets, 16, 16).await.unwrap(), json!({"hits": [], "total_hits": 0}));
    let (path, agent) = server.seen().remove(0);
    assert!(path.starts_with("/v2/search?"), "{path}");
    assert_eq!(
        pairs(&path),
        [("query", "sod ium"), ("facets", facets), ("index", "relevance"), ("offset", "16"), ("limit", "16")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    assert_eq!(agent, user_agent());
    let name = format!("{}/", launcher_shared::branding::APP_NAME);
    assert!(agent.starts_with(&name) && agent.contains(" (https://"), "{agent}");
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_are_narrowed_by_loader_and_version() {
    let server = FakeModrinth::start().await;
    server.data.lock().unwrap().versions.insert("AANobbMI".into(), json!([]));
    server.data.lock().unwrap().versions.insert("x y".into(), json!([{"id": "v"}]));
    let api = ModrinthApi::new(&format!("{}/", server.base)).unwrap();
    api.project_versions("AANobbMI", Some("fabric"), Some("1.21.1")).await.unwrap();
    assert_eq!(api.project_versions("x y", None, None).await.unwrap(), json!([{"id": "v"}]));
    let seen = server.seen();
    assert!(seen[0].0.starts_with("/v2/project/AANobbMI/version?"), "{}", seen[0].0);
    assert_eq!(
        pairs(&seen[0].0),
        [("loaders", r#"["fabric"]"#), ("game_versions", r#"["1.21.1"]"#)]
            .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    assert_eq!(seen[1].0, "/v2/project/x%20y/version", "no filters, no query");
    // A Quilt build asks for Fabric's versions too.
    api.project_versions("AANobbMI", Some("quilt"), None).await.unwrap();
    assert_eq!(
        pairs(&server.seen()[2].0),
        [("loaders", r#"["quilt","fabric"]"#)].map(|(k, v)| (k.to_string(), v.to_string()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn modrinth_errors_are_network_errors() {
    let server = FakeModrinth::start().await;
    let api = ModrinthApi::new(&server.base).unwrap();
    server.data.lock().unwrap().search_status = Some(503);
    let unavailable = api.search("", "[]", 0, 16).await.unwrap_err();
    assert_eq!(
        (unavailable.code, unavailable.params.get("status").map(String::as_str)),
        (ErrorCode::Network, Some("503"))
    );
    server.data.lock().unwrap().search_status = None;
    assert_eq!(api.search("", "[]", 0, 16).await.unwrap_err().code, ErrorCode::Network, "not JSON");
    let missing = api.project_versions("nope", None, None).await.unwrap_err();
    assert_eq!(
        (missing.code, missing.params.get("status").map(String::as_str)),
        (ErrorCode::Network, Some("404"))
    );
    drop(server);
    let offline = ModrinthApi::new("http://127.0.0.1:9/v2").unwrap();
    assert_eq!(offline.search("", "[]", 0, 16).await.unwrap_err().code, ErrorCode::Network);
}

#[tokio::test(flavor = "multi_thread")]
async fn identification_asks_in_batches_of_a_hundred() {
    let server = FakeModrinth::start().await;
    let known = support::sha512(b"known");
    server.data.lock().unwrap().hashes.insert(known.clone(), json!({"id": "v1", "project_id": "p"}));
    let api = ModrinthApi::new(&server.base).unwrap();
    let mut hashes: Vec<String> = (0..150).map(|i| format!("{i:0128}")).collect();
    hashes.push(known.clone());
    hashes.push(known.clone());
    let found = api.versions_by_hashes(&hashes).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[&known]["project_id"], "p");
    let posts = server.seen().iter().filter(|(path, _)| path == "/v2/version_files").count();
    assert_eq!(posts, 2, "151 distinct hashes, 100 a request");
    server.data.lock().unwrap().hashes_status = Some(503);
    assert_eq!(api.versions_by_hashes(&hashes).await.unwrap_err().code, ErrorCode::Network);
    server.data.lock().unwrap().projects.insert("p".into(), json!({"id": "p", "title": "P"}));
    assert_eq!(api.project("p").await.unwrap()["title"], "P");
    assert_eq!(api.version("nope").await.unwrap_err().code, ErrorCode::Network);
}

#[tokio::test(flavor = "multi_thread")]
async fn newer_versions_are_asked_by_hash_100_at_a_time_for_the_build() {
    let server = FakeModrinth::start().await;
    let api = ModrinthApi::new(&server.base).unwrap();
    let hashes: Vec<String> = (0..150).map(|i| format!("{i:0128}")).chain(["0".repeat(128)]).collect();
    let found = api.latest_versions(&hashes, &["fabric"], &["1.21.1"]).await.unwrap();
    assert!(found.is_empty());
    let bodies = server.data.lock().unwrap().update_bodies.clone();
    assert_eq!(bodies.len(), 2, "150 distinct hashes, two batches");
    assert_eq!(bodies[0]["hashes"].as_array().unwrap().len(), 100);
    assert_eq!(
        (bodies[0]["algorithm"].as_str(), bodies[0]["loaders"].clone(), bodies[0]["game_versions"].clone()),
        (Some("sha512"), json!(["fabric"]), json!(["1.21.1"]))
    );
    api.latest_versions(&hashes[..1], &[], &[]).await.unwrap();
    let last = server.data.lock().unwrap().update_bodies.last().cloned().unwrap();
    assert!(last.get("loaders").is_none() && last.get("game_versions").is_none(), "no filter, no key");
}
