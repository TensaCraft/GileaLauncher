mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use launcher_core::auth::avatars::{Avatar, AvatarCache, cache_file};
use launcher_core::auth::http::AuthHttp;
use launcher_core::auth::service::AuthService;
use launcher_core::auth::{AuthConfig, AuthTimings, NoOpener};
use launcher_core::feedback::{FeedbackService, NullSink};
use support::fake_ms::{self, Fake};

fn cache(fake: &Fake, dir: &Path) -> AvatarCache {
    let http = AuthHttp::new(&AuthTimings::fast()).unwrap();
    AvatarCache::new(
        dir.join("avatars"),
        fake.avatar_sources(),
        http.client().clone(),
        Duration::from_secs(2),
    )
}

fn hits(fake: &Fake) -> [usize; 3] {
    ["mc-heads", "minotar", "mineskin"].map(|host| fake.requests(&format!("/avatar/{host}")).len())
}

#[tokio::test(flavor = "multi_thread")]
async fn first_host_with_an_image_wins() {
    let fake = fake_ms::start().await;
    fake.knobs().avatars = [(500, "image/png"), (200, "text/html"), (200, "image/png")];
    let dir = tempfile::tempdir().unwrap();
    let avatar = cache(&fake, dir.path()).get("Steve").await;
    assert_eq!(avatar, Some(Avatar::Png(b"PNG-mineskin".to_vec())));
    assert_eq!(hits(&fake), [1, 1, 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_cache_skips_the_network() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = cache(&fake, dir.path());
    avatars.get("Steve").await.unwrap();
    assert_eq!(avatars.get("Steve").await, Some(Avatar::Png(b"PNG-mc-heads".to_vec())));
    assert_eq!(hits(&fake), [1, 0, 0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_cache_is_used_when_every_host_fails() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = cache(&fake, dir.path());
    avatars.get("Steve").await.unwrap();
    let file = cache_file(&dir.path().join("avatars"), "Steve");
    let old = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    fake.knobs().avatars = [(500, ""), (500, ""), (500, "")];
    assert_eq!(avatars.get("Steve").await, Some(Avatar::Png(b"PNG-mc-heads".to_vec())));
    assert_eq!(hits(&fake), [2, 1, 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_cache_the_last_address_is_returned() {
    let fake = fake_ms::start().await;
    fake.knobs().avatars = [(500, ""), (404, ""), (503, "")];
    let dir = tempfile::tempdir().unwrap();
    match cache(&fake, dir.path()).get("Steve").await {
        Some(Avatar::Remote(url)) => assert!(url.ends_with("/mineskin/avatar/Steve/64"), "{url}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blank_and_non_ascii_identifiers() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = cache(&fake, dir.path());
    avatars.get("   ").await.unwrap();
    avatars.get("Іван").await.unwrap();
    let ids: Vec<String> = fake
        .requests("/avatar/mc-heads")
        .iter()
        .map(|r| r.form.get("id").cloned().unwrap_or_default())
        .collect();
    assert_eq!(ids, ["Steve", "Іван"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_uses_the_profile_uuid() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AuthConfig::new("c", dir.path(), &dir.path().join("cache"));
    cfg.endpoints = fake.endpoints();
    cfg.timings = AuthTimings::fast();
    cfg.avatar_sources = fake.avatar_sources();
    let sink = Arc::new(NullSink);
    let svc = AuthService::new(cfg, FeedbackService::new(sink.clone()), sink, Arc::new(NoOpener)).unwrap();
    svc.create_offline("Steve").unwrap();
    assert!(svc.avatar("Steve").await.unwrap().to_src().starts_with("data:image/png;base64,"));
    assert_eq!(
        fake.requests("/avatar/mc-heads")[0].form.get("id").map(String::as_str),
        Some("5627dd98-e6be-bc21-f8a8-e92344183641")
    );
    assert!(svc.avatar("nobody").await.is_none());
}
