mod support;

use std::sync::Arc;

use launcher_core::auth::injector::{self, Artifact};
use launcher_core::auth::service::AgentServer;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_shared::ErrorCode;
use sha2::{Digest, Sha256};
use support::fake_files::{self, Served};

fn downloader() -> Arc<Downloader> {
    Arc::new(Downloader::new(DownloaderConfig { retries: 1, ..DownloaderConfig::default() }).unwrap())
}

fn artifact(urls: Vec<String>, body: &[u8]) -> Artifact {
    Artifact {
        version: "9.9.9".into(),
        urls,
        sha256: hex::encode(Sha256::digest(body)),
        size: body.len() as u64,
    }
}

#[test]
fn the_agent_points_the_game_at_the_server_with_its_metadata_prefetched() {
    let server = AgentServer { api_root: "https://ely.by/api/".into(), metadata: "{\"meta\":{}}".into() };
    let args = injector::agent_arguments(std::path::Path::new("/agents/agent.jar"), &server);
    assert_eq!(args[0], "-javaagent:/agents/agent.jar=https://ely.by/api/");
    assert_eq!(args[1], "-Dauthlibinjector.yggdrasil.prefetched=eyJtZXRhIjp7fX0=");
}

#[test]
fn the_pinned_agent_is_a_known_release_checked_by_its_digest() {
    let pinned = injector::pinned();
    assert_eq!(pinned.sha256.len(), 64);
    assert!(pinned.urls.iter().all(|u| u.starts_with("https://")), "{:?}", pinned.urls);
    assert!(pinned.urls[0].contains(&pinned.version));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_is_downloaded_once_and_checked() {
    let files = fake_files::start().await;
    let body = b"agent jar bytes".to_vec();
    files.put("agent.jar", Served { body: body.clone(), ..Served::default() });
    let dir = tempfile::tempdir().unwrap();
    let wanted = artifact(vec![files.url("agent.jar")], &body);
    let jar = injector::ensure_jar(&downloader(), dir.path(), &wanted).await.unwrap();
    assert_eq!(std::fs::read(&jar).unwrap(), body);
    assert!(jar.ends_with("authlib-injector-9.9.9.jar"));
    injector::ensure_jar(&downloader(), dir.path(), &wanted).await.unwrap();
    assert_eq!(files.seen("agent.jar").len(), 1, "kept for the next launch");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_address_is_tried_and_a_wrong_file_is_refused() {
    let files = fake_files::start().await;
    let body = b"agent jar bytes".to_vec();
    files.put("down.jar", Served { status: Some(404), ..Served::default() });
    files.put("mirror.jar", Served { body: body.clone(), ..Served::default() });
    files.put("other.jar", Served { body: b"agent jar bytez".to_vec(), ..Served::default() });
    let dir = tempfile::tempdir().unwrap();
    let wanted = artifact(vec![files.url("down.jar"), files.url("mirror.jar")], &body);
    let jar = injector::ensure_jar(&downloader(), dir.path(), &wanted).await.unwrap();
    assert_eq!(std::fs::read(jar).unwrap(), body);
    let other = tempfile::tempdir().unwrap();
    let wrong = artifact(vec![files.url("other.jar")], &body);
    let err = injector::ensure_jar(&downloader(), other.path(), &wrong).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::IntegrityMismatch);
}
