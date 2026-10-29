//! Profile heads: drawn from the player's skin, ready-drawn heads from the head hosts only when no
//! skin could be had at all.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use launcher_core::auth::service::AuthService;
use launcher_core::auth::{AuthConfig, AuthTimings, NoOpener};
use launcher_core::feedback::{FeedbackService, NullSink};
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::skins::{AVATAR_SIZE, Avatar, Player, SkinConfig, SkinService, render};
use support::fake_skins::{self, FakeSkins, Player as Known, skin_png};

fn skins(fake: &FakeSkins, dir: &Path) -> SkinService {
    let mut cfg = SkinConfig::new(&dir.join("cache"));
    cfg.endpoints = fake.endpoints();
    cfg.timeout = Duration::from_secs(2);
    let downloader = DownloaderConfig { retries: 1, ..DownloaderConfig::default() };
    SkinService::new(cfg, Arc::new(Downloader::new(downloader).unwrap())).unwrap()
}

/// Mojang knows nobody and the skin hosts are down: only the head hosts are left.
async fn heads_only() -> FakeSkins {
    let fake = fake_skins::start().await;
    fake.knobs().session_status = Some(204);
    fake.knobs().skins = [(500, ""), (500, "")];
    fake
}

fn hits(fake: &FakeSkins) -> [usize; 3] {
    ["mc-heads", "minotar", "mineskin"].map(|host| fake.requests(&format!("/head/{host}")).len())
}

fn auth(dir: &Path) -> Arc<AuthService> {
    let mut cfg = AuthConfig::new("c", dir);
    cfg.timings = AuthTimings::fast();
    let sink = Arc::new(NullSink);
    AuthService::new(cfg, FeedbackService::new(sink.clone()), sink, Arc::new(NoOpener)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn first_head_host_with_an_image_wins() {
    let fake = heads_only().await;
    fake.knobs().heads = [(500, "image/png"), (200, "text/html"), (200, "image/png")];
    let dir = tempfile::tempdir().unwrap();
    let avatar = skins(&fake, dir.path()).head(&Player::named("Steve"), AVATAR_SIZE).await;
    assert_eq!(avatar, Some(Avatar::Png(b"PNG-mineskin".to_vec())));
    assert_eq!(hits(&fake), [1, 1, 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_head_skips_the_head_hosts() {
    let fake = heads_only().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = skins(&fake, dir.path());
    avatars.head(&Player::named("Steve"), AVATAR_SIZE).await.unwrap();
    let again = avatars.head(&Player::named("Steve"), AVATAR_SIZE).await;
    assert_eq!(again, Some(Avatar::Png(b"PNG-mc-heads".to_vec())));
    assert_eq!(hits(&fake), [1, 0, 0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_head_is_used_when_every_host_fails() {
    let fake = heads_only().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = skins(&fake, dir.path());
    avatars.head(&Player::named("Steve"), AVATAR_SIZE).await.unwrap();
    let kept: Vec<_> =
        std::fs::read_dir(dir.path().join("cache/skins/heads")).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(kept.len(), 1);
    let old = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options().write(true).open(&kept[0]).unwrap().set_modified(old).unwrap();
    fake.knobs().heads = [(500, ""), (500, ""), (500, "")];
    let avatar = avatars.head(&Player::named("Steve"), AVATAR_SIZE).await;
    assert_eq!(avatar, Some(Avatar::Png(b"PNG-mc-heads".to_vec())));
    assert_eq!(hits(&fake), [2, 1, 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_any_head_the_last_address_is_returned() {
    let fake = heads_only().await;
    fake.knobs().heads = [(500, ""), (404, ""), (503, "")];
    let dir = tempfile::tempdir().unwrap();
    match skins(&fake, dir.path()).head(&Player::named("Steve"), AVATAR_SIZE).await {
        Some(Avatar::Remote(url)) => assert!(url.ends_with("/mineskin/avatar/Steve/64"), "{url}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn non_ascii_names_are_asked_for_escaped() {
    let fake = heads_only().await;
    let dir = tempfile::tempdir().unwrap();
    skins(&fake, dir.path()).head(&Player::named("Іван"), AVATAR_SIZE).await.unwrap();
    assert!(fake.requests_to("/name/").is_empty(), "no nickname of Minecraft's: Mojang is not asked");
    let ids: Vec<String> =
        fake.requests("/head/mc-heads").iter().map(|r| r.id.clone().unwrap_or_default()).collect();
    assert_eq!(ids, ["Іван"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_s_head_is_its_skin_by_uuid() {
    let fake = fake_skins::start().await;
    let dir = tempfile::tempdir().unwrap();
    let auth = auth(dir.path());
    let avatars = skins(&fake, dir.path());
    auth.create_offline("Steve").unwrap();
    let player = auth.player("Steve").unwrap();
    assert_eq!(player, Player::new("5627dd98-e6be-bc21-f8a8-e92344183641", "Steve"));
    assert!(auth.player("nobody").is_none());

    // An offline profile: Mojang knows nobody by its UUID, a skin host draws the head.
    let head = avatars.head(&player, AVATAR_SIZE).await.unwrap();
    assert!(head.to_src().starts_with("data:image/png;base64,"));
    assert_eq!(fake.requests("/session/5627dd98e6bebc21f8a8e92344183641").len(), 1);
    assert_eq!(fake.requests("/skin/mc-heads")[0].id.as_deref(), Some("5627dd98e6bebc21f8a8e92344183641"));
    assert!(fake.requests_to("/head/").is_empty(), "the head comes from the skin");

    // A player Mojang knows: their own skin, not a host's.
    fake.add_texture("real", skin_png(42, false));
    fake.add_player(Known::new("5627dd98e6bebc21f8a8e92344183641", "Steve", Some("real"), false));
    avatars.forget(&player);
    let head = avatars.head(&player, AVATAR_SIZE).await.unwrap();
    let skin = render::normalise(&skin_png(42, false)).unwrap();
    assert_eq!(head, Avatar::Png(render::head(&skin, AVATAR_SIZE)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forgotten_head_is_asked_for_again() {
    let fake = heads_only().await;
    let dir = tempfile::tempdir().unwrap();
    let avatars = skins(&fake, dir.path());
    let steve = Player::new("5627dd98-e6be-bc21-f8a8-e92344183641", "Steve");
    avatars.head(&steve, AVATAR_SIZE).await.unwrap();
    avatars.head(&steve, AVATAR_SIZE).await.unwrap();
    assert_eq!(fake.requests("/head/mc-heads").len(), 1, "the second look is kept");
    avatars.forget(&steve);
    avatars.head(&steve, AVATAR_SIZE).await.unwrap();
    assert_eq!(fake.requests("/head/mc-heads").len(), 2, "a forgotten head is asked for again");
    avatars.forget(&Player::named("nobody"));
}
