mod support;

use std::time::Duration;

use launcher_shared::ErrorCode;
use module_tensa::backend::api::TensaApi;
use serde_json::json;
use support::Server;

fn api(server: &Server) -> TensaApi {
    TensaApi::new(&server.url("/api/mods")).unwrap().with_retry_delay(Duration::from_millis(1))
}

#[tokio::test]
async fn the_catalog_keeps_objects_only() {
    let server = Server::start().await;
    server.json("/api/mods", json!([{"client": {"id": "a"}}, 3, "x"]));
    assert_eq!(api(&server).packs().await.unwrap(), vec![json!({"client": {"id": "a"}})]);
    let other = Server::start().await;
    other.json("/api/mods", json!({"oops": 1}));
    assert!(api(&other).packs().await.unwrap().is_empty());
}

#[tokio::test]
async fn files_come_from_the_named_endpoint_or_the_pack_address() {
    let server = Server::start().await;
    server.json("/api/mods/pack", json!({"files": [{"name": "a.jar"}, 7]}));
    server.json("/custom", json!([{"name": "b.jar"}, "x"]));
    let api = api(&server);
    assert_eq!(api.files("pack", None).await.unwrap(), vec![json!({"name": "a.jar"})]);
    let named = server.url("/custom?x=1");
    assert_eq!(api.files("pack", Some(&named)).await.unwrap(), vec![json!({"name": "b.jar"})]);
    assert_eq!(server.seen(), ["/api/mods/pack", "/custom?x=1"]);
    assert_eq!(api.files(" ", None).await.unwrap_err().code, ErrorCode::InvalidInput);
}

#[tokio::test]
async fn the_force_manifest_asks_for_directory_files() {
    let server = Server::start().await;
    server.json(
        "/api/mods/pack/force-update",
        json!({
            "summary": {"returned_files_count": 1},
            "preserve_rules": [],
            "files": [{"name": "a"}, 1],
            "directories": [{"path": "mods"}, "x"]
        }),
    );
    server.json("/f", json!([{"name": "b"}]));
    let api = api(&server);
    assert_eq!(
        api.force_manifest("pack", None).await.unwrap(),
        json!({
            "summary": {"returned_files_count": 1},
            "preserve_rules": [],
            "files": [{"name": "a"}],
            "directories": [{"path": "mods"}]
        })
    );
    let named = server.url("/f?token=t&include_directory_files=0");
    assert_eq!(
        api.force_manifest("pack", Some(&named)).await.unwrap(),
        json!({"files": [{"name": "b"}], "directories": []})
    );
    assert_eq!(
        server.seen(),
        ["/api/mods/pack/force-update?include_directory_files=1", "/f?token=t&include_directory_files=1"]
    );
}

#[tokio::test]
async fn a_failing_server_is_asked_three_times() {
    let server = Server::start().await;
    server.reply("/api/mods", 500, "{}");
    assert_eq!(api(&server).packs().await.unwrap_err().code, ErrorCode::Network);
    assert_eq!(server.seen().len(), 3);

    let broken = Server::start().await;
    broken.reply("/api/mods", 200, "<html>not json</html>");
    assert_eq!(api(&broken).packs().await.unwrap_err().code, ErrorCode::Network);
    assert_eq!(broken.seen().len(), 3, "an answer that is not JSON fails too");

    let flaky = Server::start().await;
    flaky.reply("/api/mods", 500, "{}");
    flaky.reply("/api/mods", 502, "{}");
    flaky.json("/api/mods", json!([]));
    assert!(api(&flaky).packs().await.unwrap().is_empty());
    assert_eq!(flaky.seen().len(), 3);
}

#[tokio::test]
async fn an_address_the_server_refuses_is_asked_once() {
    for status in [400, 401, 403, 404, 410] {
        let server = Server::start().await;
        server.reply("/api/mods/pack/force-update?include_directory_files=1", status, "{}");
        assert_eq!(api(&server).force_manifest("pack", None).await.unwrap_err().code, ErrorCode::Network);
        assert_eq!(server.seen().len(), 1, "HTTP {status} is not asked again");
    }
    for status in [408, 429] {
        let server = Server::start().await;
        server.reply("/api/mods", status, "{}");
        server.json("/api/mods", json!([]));
        assert!(api(&server).packs().await.unwrap().is_empty(), "HTTP {status} is asked again");
        assert_eq!(server.seen().len(), 2);
    }
}
