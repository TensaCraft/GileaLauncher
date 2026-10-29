//! The skin service against a fake Mojang and fake third-party hosts.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::skins::service::{NO_PLAYER, NO_SKIN, REASON};
use launcher_core::skins::{Avatar, Player, SkinConfig, SkinService, render};
use launcher_shared::{AppResult, ErrorCode};
use support::fake_skins::{self, FakeSkins, Player as Known, skin_png};

const STEVE_ID: &str = "0123456789abcdef0123456789abcdef";
const STEVE_DASHED: &str = "01234567-89ab-cdef-0123-456789abcdef";
const ALEX_ID: &str = "bb22bb22bb22bb22bb22bb22bb22bb22";
/// An offline profile's UUID (made from «Steve»): Mojang knows none.
const OFFLINE_DASHED: &str = "5627dd98-e6be-bc21-f8a8-e92344183641";
const OFFLINE_ID: &str = "5627dd98e6bebc21f8a8e92344183641";

fn config(fake: &FakeSkins, dir: &Path) -> SkinConfig {
    let mut cfg = SkinConfig::new(dir);
    cfg.endpoints = fake.endpoints();
    cfg.timeout = Duration::from_secs(5);
    cfg
}

fn downloader() -> Arc<Downloader> {
    let cfg =
        DownloaderConfig { retries: 2, retry_delay: Duration::from_millis(1), ..DownloaderConfig::default() };
    Arc::new(Downloader::new(cfg).unwrap())
}

fn service(cfg: SkinConfig) -> SkinService {
    SkinService::new(cfg, downloader()).unwrap()
}

fn normalised(bytes: &[u8]) -> Vec<u8> {
    render::normalise(bytes).unwrap()
}

/// The fake knows Steve_42 (classic, texture `steve`) and Alex (slim, texture `alex`).
async fn known() -> FakeSkins {
    let fake = fake_skins::start().await;
    fake.add_texture("steve", skin_png(7, false));
    fake.add_texture("alex", skin_png(5, true));
    fake.add_player(Known::new(STEVE_ID, "Steve_42", Some("steve"), false));
    fake.add_player(Known::new(ALEX_ID, "Alex", Some("alex"), true));
    fake
}

fn steve() -> Player {
    Player::new(STEVE_DASHED, "Steve_42")
}

fn reason<T: std::fmt::Debug>(result: AppResult<T>) -> (ErrorCode, Option<String>) {
    let error = result.unwrap_err();
    (error.code, error.params.get(REASON).cloned())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_official_head_comes_first() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let head = svc.head(&steve(), 64).await;
    assert_eq!(head, Some(Avatar::Png(render::head(&normalised(&skin_png(7, false)), 64))));
    assert_eq!(fake.requests(&format!("/session/{STEVE_ID}")).len(), 1, "asked by the UUID, no dashes");
    assert!(fake.requests_to("/skin/").is_empty(), "no third-party skin");
    assert!(fake.requests_to("/head/").is_empty(), "no third-party head");
    assert!(fake.all_requests().iter().all(|r| r.bearer.is_none()), "nothing carries a token");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nickname_finds_the_uuid_then_the_texture() {
    let fake = known().await;
    // The model is what Mojang says, not what the picture looks like.
    fake.add_texture("alex", skin_png(5, false));
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let texture = svc.texture(&Player::named(" Alex ")).await.unwrap();
    assert!(texture.official);
    assert!(texture.slim, "Mojang's metadata names the model");
    assert_eq!(texture.png, normalised(&skin_png(5, false)));
    assert_eq!(texture.hash, render::sha256(&texture.png));
    assert_eq!(fake.requests("/name/Alex").len(), 1, "the name is trimmed");
    assert_eq!(fake.requests(&format!("/session/{ALEX_ID}")).len(), 1);
    assert_eq!(fake.requests("/textures/alex").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn third_party_hosts_answer_when_mojang_has_nothing() {
    for status in [204, 404, 500] {
        let fake = known().await;
        fake.knobs().session_status = Some(status);
        fake.set_host_skin(0, skin_png(101, true));
        let dir = tempfile::tempdir().unwrap();
        let svc = service(config(&fake, dir.path()));
        let texture = svc.texture(&Player::new(OFFLINE_DASHED, "Steve")).await.unwrap();
        assert!(!texture.official, "{status}");
        assert!(texture.slim, "{status}: the model is guessed");
        assert_eq!(texture.png, normalised(&skin_png(101, true)), "{status}");
        let asked = fake.requests("/skin/mc-heads");
        assert_eq!(asked[0].id.as_deref(), Some(OFFLINE_ID), "{status}: by the UUID, no dashes");
        assert!(fake.requests_to("/head/").is_empty(), "{status}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_or_wrong_host_gives_way_to_the_next() {
    let fake = known().await;
    fake.knobs().session_status = Some(204);
    fake.set_host_skin(1, skin_png(102, false));
    for first in [(500, "image/png"), (404, "image/png"), (200, "text/html")] {
        fake.knobs().skins[0] = first;
        let dir = tempfile::tempdir().unwrap();
        let svc = service(config(&fake, dir.path()));
        let texture = svc.texture(&Player::named("Somebody")).await.unwrap();
        assert_eq!(texture.png, normalised(&skin_png(102, false)), "{first:?}");
        assert_eq!(fake.requests("/skin/minotar").last().unwrap().id.as_deref(), Some("Somebody"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lookup_is_kept_and_survives_a_restart() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let first = svc.texture(&steve()).await.unwrap();
    assert_eq!(svc.texture(&steve()).await.unwrap(), first);
    assert_eq!(svc.texture(&Player::new(STEVE_ID, "")).await.unwrap(), first, "dashes or not, one player");
    let again = service(config(&fake, dir.path()));
    assert_eq!(again.texture(&steve()).await.unwrap(), first, "kept on disk");
    assert_eq!(fake.requests(&format!("/session/{STEVE_ID}")).len(), 1);
    assert_eq!(fake.requests("/textures/steve").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_lookup_is_asked_again_and_used_when_every_source_fails() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&fake, dir.path());
    cfg.official_ttl = Duration::ZERO;
    let svc = service(cfg);
    let first = svc.texture(&steve()).await.unwrap();
    svc.texture(&steve()).await.unwrap();
    assert_eq!(fake.requests(&format!("/session/{STEVE_ID}")).len(), 2, "a stale lookup is asked again");
    assert_eq!(fake.requests("/textures/steve").len(), 1, "the texture file is kept");

    fake.knobs().session_status = Some(500);
    fake.knobs().skins = [(500, ""), (503, "")];
    assert_eq!(svc.texture(&steve()).await, Some(first), "the stale one, when nothing answers");
    assert!(!fake.requests("/skin/minotar").is_empty(), "after the third-party hosts were asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_third_party_skin_is_kept_a_day_unless_mojang_failed() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&fake, dir.path());
    cfg.official_ttl = Duration::ZERO;
    let svc = service(cfg);
    let offline = Player::new(OFFLINE_DASHED, "Steve");
    svc.texture(&offline).await.unwrap();
    svc.texture(&offline).await.unwrap();
    assert_eq!(fake.requests(&format!("/session/{OFFLINE_ID}")).len(), 1, "Mojang said it has none");
    assert_eq!(fake.requests("/skin/mc-heads").len(), 1);

    let unreachable = Player::named("Alex");
    fake.knobs().session_status = Some(500);
    svc.texture(&unreachable).await.unwrap();
    svc.texture(&unreachable).await.unwrap();
    assert_eq!(fake.requests(&format!("/session/{ALEX_ID}")).len(), 2, "Mojang failed: asked again soon");
}

#[tokio::test(flavor = "multi_thread")]
async fn forget_asks_mojang_again() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    svc.texture(&steve()).await.unwrap();
    fake.add_texture("changed", skin_png(8, true));
    fake.add_player(Known::new(STEVE_ID, "Steve_42", Some("changed"), true));
    assert_eq!(svc.texture(&steve()).await.unwrap().png, normalised(&skin_png(7, false)), "kept");
    svc.forget(&steve());
    let texture = svc.texture(&steve()).await.unwrap();
    assert_eq!((texture.png, texture.slim), (normalised(&skin_png(8, true)), true));
    assert_eq!(fake.requests(&format!("/session/{STEVE_ID}")).len(), 2);
    svc.forget(&Player::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_texture_by_url_is_fetched_once() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let slim = svc.texture_by_url(&fake.texture_url("steve"), true).await.unwrap();
    let classic = svc.texture_by_url(&fake.texture_url("steve"), false).await.unwrap();
    assert_eq!(slim.png, normalised(&skin_png(7, false)));
    assert_eq!(slim.hash, classic.hash);
    assert!(slim.slim && !classic.slim, "the model is the caller's");
    assert!(slim.official);
    assert_eq!(fake.requests("/textures/steve").len(), 1);
    let again = service(config(&fake, dir.path()));
    assert_eq!(again.texture_by_url(&fake.texture_url("steve"), false).await.unwrap(), classic);
    assert_eq!(fake.requests("/textures/steve").len(), 1, "kept on disk");

    let elsewhere = svc.texture_by_url("https://example.com/skin.png", false).await;
    assert_eq!(elsewhere.unwrap_err().code, ErrorCode::InvalidInput, "only Mojang's texture hosts");
    assert!(svc.texture_by_url(&fake.texture_url("missing"), false).await.is_err());
    fake.add_texture("broken", b"not a png".to_vec());
    assert!(svc.texture_by_url(&fake.texture_url("broken"), false).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skin_at_a_link_is_fetched_and_its_model_guessed() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let alex = svc.at_link(&fake.texture_url("alex")).await.unwrap();
    assert_eq!(alex.png, normalised(&skin_png(5, true)));
    assert!(alex.slim, "guessed from the arm");
    assert!(!alex.official, "a link is the player's word, not Mojang's");
    let steve = svc.at_link(&fake.texture_url("steve")).await.unwrap();
    assert!(!steve.slim);
    svc.at_link(&fake.texture_url("steve")).await.unwrap();
    assert_eq!(fake.requests("/textures/steve").len(), 2, "a link is asked anew: its picture may change");

    fake.add_texture("broken", b"<html>a page</html>".to_vec());
    let page = svc.at_link(&fake.texture_url("broken")).await.unwrap_err();
    assert_eq!(
        (page.code, page.params.get(REASON).map(String::as_str)),
        (ErrorCode::InvalidInput, Some(render::BAD_PNG))
    );
    assert!(svc.at_link(&fake.texture_url("missing")).await.is_err());

    fake.add_texture("huge", vec![0; 4 * render::MAX_BYTES]);
    let huge = svc.at_link(&fake.texture_url("huge")).await.unwrap_err();
    assert_ne!(
        huge.params.get(REASON).map(String::as_str),
        Some(render::BAD_PNG),
        "cut off while it comes, never written whole and read: {huge:?}"
    );
    let left: Vec<_> =
        walkdir(dir.path()).into_iter().filter(|p| p.to_string_lossy().ends_with(".tmp")).collect();
    assert!(left.is_empty(), "nothing is left behind: {left:?}");
}

/// Every file below `dir`.
fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() { out.extend(walkdir(&path)) } else { out.push(path) }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn official_says_why_it_has_nothing() {
    let fake = known().await;
    fake.add_player(Known::new("cc33cc33cc33cc33cc33cc33cc33cc33", "Plain", None, false));
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let no_player = (ErrorCode::NotFound, Some(NO_PLAYER.to_string()));
    assert_eq!(reason(svc.official(&Player::named("Nobody")).await), no_player);
    fake.knobs().unknown_is_204 = true;
    assert_eq!(reason(svc.official(&Player::named("Nobody")).await), no_player);
    assert_eq!(reason(svc.official(&Player::named("two words")).await), no_player);
    assert!(fake.requests_to("/name/two").is_empty(), "a name no player can have asks nothing");
    assert_eq!(reason(svc.official(&Player::new(OFFLINE_DASHED, "")).await), no_player);
    assert_eq!(
        reason(svc.official(&Player::named("Plain")).await),
        (ErrorCode::NotFound, Some(NO_SKIN.to_string()))
    );
    let alex = svc.official(&Player::named("alex")).await.unwrap();
    assert!(alex.official && alex.slim);
    fake.knobs().name_status = Some(429);
    assert_eq!(reason(svc.official(&Player::named("Alex")).await).0, ErrorCode::RateLimited);
    fake.knobs().name_status = None;
    fake.knobs().session_status = Some(500);
    assert_eq!(reason(svc.official(&Player::named("Alex")).await).0, ErrorCode::Network);
    assert!(fake.requests_to("/skin/").is_empty(), "never a third-party host");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worn_skin_is_the_player_s_at_once() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    svc.texture(&steve()).await.unwrap();
    fake.add_texture("new", skin_png(9, false));
    let worn = svc.wear(&steve(), &fake.texture_url("new"), true).await.unwrap();
    assert!(worn.slim && worn.official);
    assert_eq!(svc.texture(&steve()).await.unwrap(), worn);
    assert_eq!(svc.head(&steve(), 8).await, Some(Avatar::Png(render::head(&worn.png, 8))));
    assert_eq!(fake.requests(&format!("/session/{STEVE_ID}")).len(), 1, "the session server is not asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_front_is_drawn_on_the_player_s_model() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    let front = svc.front(&Player::named("Alex")).await.unwrap();
    assert_eq!(front, render::front(&normalised(&skin_png(5, true)), true));
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_has_nothing() {
    let fake = known().await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(config(&fake, dir.path()));
    assert_eq!(svc.texture(&Player::default()).await, None);
    assert_eq!(svc.head(&Player::new("  ", " "), 64).await, None);
    assert!(fake.all_requests().is_empty());
}
