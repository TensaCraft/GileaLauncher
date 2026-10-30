mod support;

use launcher_core::auth::AuthTimings;
use launcher_core::auth::http::AuthHttp;
use launcher_core::auth::servers::{AuthServers, Origin, Preset};
use launcher_shared::ErrorCode;
use support::fake_ygg::{self, SERVER_NAME};

fn http() -> AuthHttp {
    AuthHttp::new(&AuthTimings::fast()).unwrap()
}

fn preset(id: &'static str, address: &str) -> Preset {
    Preset { id, name: "Preset".into(), address: address.to_string() }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_module_s_server_is_kept_under_the_id_it_gives() {
    let fake = fake_ygg::start().await;
    let dir = tempfile::tempdir().unwrap();
    let servers = AuthServers::open(dir.path(), Vec::new());
    let origin = Origin::Module("tensa".into());
    let added = servers.register(&http(), "tensa", &fake.base, origin).await.unwrap();
    assert_eq!(added.id, "tensa");
    assert_eq!((added.name.as_str(), added.api_root.as_str()), (SERVER_NAME, fake.api_root().as_str()));
    assert!(added.metadata.contains(SERVER_NAME));
    let reopened = AuthServers::open(dir.path(), Vec::new());
    assert_eq!(reopened.get("tensa"), Some(added), "kept on disk");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preset_is_found_the_first_time_it_is_needed() {
    let fake = fake_ygg::start().await;
    let dir = tempfile::tempdir().unwrap();
    let servers = AuthServers::open(dir.path(), vec![preset("elyby", &fake.base)]);
    assert_eq!(servers.get("elyby"), None, "nothing asked yet");
    let found = servers.ensure(&http(), "elyby").await.unwrap();
    assert_eq!((found.id.as_str(), found.api_root.as_str()), ("elyby", fake.api_root().as_str()));
    assert_eq!(found.origin, Origin::Preset);
    assert_eq!(servers.ensure(&http(), "elyby").await.unwrap(), found, "kept");
    let err = servers.ensure(&http(), "nowhere").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

/// A module (Tensa) points its server at another address: the server keeps its id, so the
/// accounts on it stay, and the next sign-in or launch goes to the new address.
#[tokio::test(flavor = "multi_thread")]
async fn a_module_server_moves_to_another_address_under_the_same_id() {
    let old = fake_ygg::start().await;
    let new = fake_ygg::start().await;
    let dir = tempfile::tempdir().unwrap();
    let servers = AuthServers::open(dir.path(), Vec::new());
    let first = servers.register(&http(), "tensa", &old.base, Origin::Module("tensa".into())).await.unwrap();
    assert_eq!(first.api_root, old.api_root());
    let moved = servers.register(&http(), "tensa", &new.base, Origin::Module("tensa".into())).await.unwrap();
    assert_eq!((moved.id.as_str(), moved.api_root.as_str()), ("tensa", new.api_root().as_str()));
    assert_eq!(servers.list().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_without_a_server_is_not_kept() {
    let fake = fake_ygg::start().await;
    fake.set(|k| k.no_ali = true);
    let dir = tempfile::tempdir().unwrap();
    let servers = AuthServers::open(dir.path(), Vec::new());
    let err =
        servers.register(&http(), "tensa", &fake.base, Origin::Module("tensa".into())).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthServerInvalid);
    assert!(servers.list().is_empty());
}
