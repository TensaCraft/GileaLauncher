mod support;

use std::time::Duration;

use launcher_core::minecraft::manifest::{MojangEndpoints, fetch_manifest};
use launcher_core::net::meta::MetaClient;
use launcher_shared::ErrorCode;
use serde_json::json;
use sha1::{Digest, Sha1};
use support::fake_files::{self, Served};

fn client(ttl: Duration) -> MetaClient {
    MetaClient::new(Duration::from_secs(5), ttl).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_are_cached_for_the_ttl() {
    let server = fake_files::start().await;
    server.put("list.json", Served { body: br#"{"a":1}"#.to_vec(), ..Served::default() });
    let meta = client(Duration::from_secs(60));
    assert_eq!(meta.get_json(&server.url("list.json")).await.unwrap(), json!({"a": 1}));
    meta.get_json(&server.url("list.json")).await.unwrap();
    assert_eq!(server.seen("list.json").len(), 1);
    let uncached = client(Duration::ZERO);
    uncached.get_bytes(&server.url("list.json")).await.unwrap();
    uncached.get_bytes(&server.url("list.json")).await.unwrap();
    assert_eq!(server.seen("list.json").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_are_not_cached() {
    let server = fake_files::start().await;
    server.put("flaky.json", Served { body: b"{}".to_vec(), fail_times: 1, ..Served::default() });
    let meta = client(Duration::from_secs(60));
    let err = meta.get_bytes(&server.url("flaky.json")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert!(err.params["error"].contains("503"), "{:?}", err.params);
    assert_eq!(meta.get_bytes(&server.url("flaky.json")).await.unwrap(), b"{}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_https_and_checked_hashes_are_accepted() {
    let server = fake_files::start().await;
    let meta = client(Duration::from_secs(60));
    let err = meta.get_bytes("http://example.com/x.json").await.unwrap_err();
    assert!(err.detail.contains("https"), "{}", err.detail);
    server.put("m.json", Served { body: b"abc".to_vec(), ..Served::default() });
    let good = hex::encode(Sha1::digest(b"abc"));
    assert_eq!(meta.get_verified(&server.url("m.json"), Some(&good)).await.unwrap(), b"abc");
    let err = meta.get_verified(&server.url("m.json"), Some(&"0".repeat(40))).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_version_list_comes_from_the_endpoint() {
    let server = fake_files::start().await;
    let list = json!({"latest": {"release": "1.21.1"}, "versions": [
        {"id": "1.21.1", "type": "release", "url": "https://x.example/1.21.1.json", "sha1": "bb"}]});
    server.put("mc/list.json", Served { body: list.to_string().into_bytes(), ..Served::default() });
    let endpoints =
        MojangEndpoints { version_manifest: server.url("mc/list.json"), ..MojangEndpoints::default() };
    let manifest = fetch_manifest(&client(Duration::from_secs(60)), &endpoints).await.unwrap();
    assert_eq!(manifest.find("1.21.1").map(|v| v.url.as_str()), Some("https://x.example/1.21.1.json"));
}
