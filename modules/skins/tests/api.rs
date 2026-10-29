//! The module against a fake Minecraft Services and Mojang.

mod support;

use std::sync::{Arc, Mutex};

use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_core::net::downloader::{Downloader, DownloaderConfig, Origin};
use launcher_shared::{
    AccountKind, ActivityEntry, Alert, AppError, AppResult, ErrorCode, OpsSnapshot, ProfileDto, Toast,
};
use module_skins::backend::mojang::{CallError, Endpoints, MojangClient};
use module_skins::backend::png::read_skin;
use module_skins::backend::service::{Deps, SkinsService, TokenFuture};
use module_skins::dto::{CHANGED_EVENT, MESSAGE, SkinVariant};
use support::fake_services::{self, Fake, Player, TOKEN};

fn client(fake: &Fake) -> MojangClient {
    MojangClient::new(Endpoints::local(&fake.base)).unwrap()
}

/// A skin PNG told apart from others by `mark`; slim when `slim`.
fn skin_png(mark: u8, slim: bool) -> Vec<u8> {
    let mut image = image::RgbaImage::from_pixel(64, 64, image::Rgba([90, 60, 40, 255]));
    image.put_pixel(9, 9, image::Rgba([mark, 0, 0, 255]));
    if slim {
        for y in 20..32 {
            image.put_pixel(54, y, image::Rgba([0, 0, 0, 0]));
            image.put_pixel(55, y, image::Rgba([0, 0, 0, 0]));
        }
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    png.into_inner()
}

const PROFILE_ID: &str = "0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Events(Mutex<Vec<String>>);

impl EventSink for Events {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, _: &Toast) {}
    fn alert(&self, _: &Alert) {}
    fn module_event(&self, event: &str, _: &serde_json::Value) {
        self.0.lock().unwrap().push(event.to_string());
    }
}

fn profile(key: &str, kind: AccountKind, is_default: bool) -> ProfileDto {
    ProfileDto {
        key: key.into(),
        name: key.into(),
        id: PROFILE_ID.into(),
        kind,
        is_default,
        reauth_required: false,
        reauth_reason: None,
    }
}

struct Rig {
    fake: Fake,
    dir: tempfile::TempDir,
    svc: SkinsService,
    /// The token source's calls: `(profile, force)`.
    calls: Arc<Mutex<Vec<(String, bool)>>>,
    events: Arc<Events>,
    forgotten: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    fn library_names(&self) -> Vec<String> {
        self.svc.library().into_iter().map(|s| s.name).collect()
    }
}

/// The service against a fake: profile `steve` («Steve_42») gives `stale` unless forced, then
/// `token-1`; profile `gone` needs a new sign-in.
async fn rig() -> Rig {
    let fake = fake_services::start().await;
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Events::default());
    let forgotten = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let tokens = Arc::new(move |key: &str, force: bool| -> TokenFuture {
        seen.lock().unwrap().push((key.to_string(), force));
        let key = key.to_string();
        Box::pin(async move {
            match key.as_str() {
                "steve" => {
                    let token = if force { TOKEN } else { "stale" };
                    Ok((token.to_string(), "Steve_42".to_string(), PROFILE_ID.to_string()))
                }
                _ => Err(AppError::new(ErrorCode::ReauthRequired, "sign in again")
                    .with_param(MESSAGE, "skins_error_reauth")),
            }
        })
    });
    let gone = forgotten.clone();
    let deps = Deps {
        endpoints: Endpoints::local(&fake.base),
        tokens,
        profiles: Arc::new(|| {
            vec![
                profile("Steve", AccountKind::Offline, false),
                profile("steve", AccountKind::Microsoft, false),
                profile("alex", AccountKind::Microsoft, true),
            ]
        }),
        forget_avatar: Arc::new(move |key: &str| gone.lock().unwrap().push(key.to_string())),
        downloader: Arc::new(Downloader::new(DownloaderConfig::default()).unwrap()),
        textures: Origin::new(|url| url.host_str() == Some("127.0.0.1")),
        feedback: FeedbackService::new(events.clone()),
        state_dir: dir.path().join("state"),
        cache_dir: dir.path().join("cache"),
    };
    let svc = SkinsService::new(deps).unwrap();
    Rig { fake, dir, svc, calls, events, forgotten }
}

/// `steve` takes any token; he wears skin `mark` and has two capes, the first on.
fn dressed(rig: &Rig, mark: u8, slim: bool) {
    rig.fake.add_texture("current", skin_png(mark, slim));
    rig.fake.add_texture("cape1", b"\x89PNG cape one".to_vec());
    rig.fake.add_texture("cape2", b"\x89PNG cape two".to_vec());
    let mut account = rig.fake.account();
    account.skin = Some(("current".into(), slim));
    account.capes = vec![
        ("c1".into(), "Migrator".into(), "cape1".into()),
        ("c2".into(), "Vanilla".into(), "cape2".into()),
    ];
    account.active_cape = Some("c1".into());
}

fn message<T: std::fmt::Debug>(result: AppResult<T>) -> Option<String> {
    result.unwrap_err().params.get(MESSAGE).cloned()
}

#[test]
fn accounts_are_the_official_profiles_default_first() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let rig = rt.block_on(rig());
    let accounts = rig.svc.accounts();
    let keys: Vec<(&str, bool)> = accounts.iter().map(|a| (a.key.as_str(), a.is_default)).collect();
    assert_eq!(keys, [("alex", true), ("steve", false)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn state_adds_the_current_skin_once() {
    let rig = rig().await;
    dressed(&rig, 7, true);
    let state = rig.svc.state("steve").await.unwrap();
    let skin = state.skin.clone().unwrap();
    assert_eq!(skin.sha256, read_skin(&skin_png(7, true)).unwrap().sha256);
    assert_eq!(skin.variant, SkinVariant::Slim);
    assert!(skin.data.starts_with("data:image/png;base64,"));
    let library = rig.svc.library();
    assert_eq!(library.len(), 1);
    assert_eq!(
        (library[0].name.as_str(), library[0].source.as_str(), library[0].variant),
        ("Steve_42", "account:Steve_42", SkinVariant::Slim)
    );
    assert_eq!(skin.library_id.as_deref(), Some(library[0].id.as_str()));
    assert!(library[0].thumb.starts_with("data:image/png;base64,"));
    let capes: Vec<(&str, &str, bool)> =
        state.capes.iter().map(|c| (c.id.as_str(), c.alias.as_str(), c.active)).collect();
    assert_eq!(capes, [("c1", "Migrator", true), ("c2", "Vanilla", false)]);
    assert!(state.capes.iter().all(|c| c.data.starts_with("data:image/png;base64,")));

    let again = rig.svc.state("steve").await.unwrap();
    assert_eq!(again, state);
    assert_eq!(rig.library_names(), ["Steve_42"], "the skin is added once");
    assert_eq!(rig.fake.requests("/textures/current").len(), 1, "the skin is kept in the cache");
    assert_eq!(rig.fake.requests("/textures/cape1").len(), 1, "and so are the capes");

    rig.svc.delete(&library[0].id).unwrap();
    let after = rig.svc.state("steve").await.unwrap();
    assert!(rig.library_names().is_empty(), "a skin the player deleted is not brought back");
    assert_eq!(after.skin.unwrap().library_id, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_skin_is_read_anew() {
    let rig = rig().await;
    dressed(&rig, 7, false);
    rig.svc.state("steve").await.unwrap();
    rig.fake.add_texture("changed", skin_png(8, false));
    rig.fake.account().skin = Some(("changed".into(), false));
    let state = rig.svc.state("steve").await.unwrap();
    assert_eq!(state.skin.unwrap().sha256, read_skin(&skin_png(8, false)).unwrap().sha256);
    assert_eq!(rig.library_names().len(), 2, "a skin changed elsewhere is kept too");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_without_a_custom_skin_adds_nothing() {
    let rig = rig().await;
    let state = rig.svc.state("steve").await.unwrap();
    assert_eq!(state.skin, None);
    assert!(state.capes.is_empty());
    assert!(rig.svc.library().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_uploads_the_library_skin_with_its_variant_and_fires_the_event() {
    let rig = rig().await;
    let file = rig.dir.path().join("Мій скін.png");
    std::fs::write(&file, skin_png(3, false)).unwrap();
    let entry = rig.svc.import_file(&file.to_string_lossy(), None).unwrap();
    assert_eq!((entry.name.as_str(), entry.source.as_str()), ("Мій скін", "file"));
    assert_eq!(entry.variant, SkinVariant::Classic, "guessed from the arm");
    rig.svc.set_variant(&entry.id, SkinVariant::Slim).unwrap();
    let state = rig.svc.apply("steve", &entry.id).await.unwrap();
    let sent = &rig.fake.requests("/skins")[0];
    assert_eq!(sent.fields.get("variant").map(Vec::as_slice), Some(&b"slim"[..]));
    assert_eq!(sent.fields.get("file"), Some(&read_skin(&skin_png(3, false)).unwrap().png));
    assert_eq!(state.skin.unwrap().library_id, Some(entry.id.clone()), "the worn skin is the library's");
    assert!(rig.events.0.lock().unwrap().iter().any(|e| e == CHANGED_EVENT));
    assert_eq!(*rig.forgotten.lock().unwrap(), ["steve"], "the profile's head is drawn again");
    assert_eq!(rig.library_names(), ["Мій скін"], "the worn skin is not added twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_file_is_refused() {
    let rig = rig().await;
    let file = rig.dir.path().join("photo.png");
    std::fs::write(&file, b"\xFF\xD8\xFF not a png").unwrap();
    assert_eq!(
        message(rig.svc.import_file(&file.to_string_lossy(), None)).as_deref(),
        Some("skins_error_bad_png")
    );
    let big = rig.dir.path().join("big.png");
    std::fs::write(&big, vec![0u8; 200 * 1024]).unwrap();
    assert_eq!(
        message(rig.svc.import_file(&big.to_string_lossy(), None)).as_deref(),
        Some("skins_error_bad_png")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_cape_none_takes_it_off() {
    let rig = rig().await;
    dressed(&rig, 1, false);
    let state = rig.svc.set_cape("steve", Some("c2".into())).await.unwrap();
    let active: Vec<&str> = state.capes.iter().filter(|c| c.active).map(|c| c.id.as_str()).collect();
    assert_eq!(active, ["c2"]);
    let state = rig.svc.set_cape("steve", None).await.unwrap();
    assert!(state.capes.iter().all(|c| !c.active));
    assert_eq!(rig.fake.requests("/capes")[1].method, "DELETE");
    assert_eq!(rig.events.0.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_401_is_refreshed_and_retried_once() {
    let rig = rig().await;
    rig.fake.knobs().valid_token = Some(TOKEN.into());
    dressed(&rig, 1, false);
    rig.svc.state("steve").await.unwrap();
    assert_eq!(*rig.calls.lock().unwrap(), [("steve".to_string(), false), ("steve".to_string(), true)]);
    let bearers: Vec<Option<String>> = rig.fake.requests("/profile").into_iter().map(|r| r.bearer).collect();
    assert_eq!(bearers, [Some("stale".to_string()), Some(TOKEN.to_string())]);

    rig.fake.knobs().valid_token = Some("nobody's".into());
    rig.calls.lock().unwrap().clear();
    assert_eq!(message(rig.svc.state("steve").await).as_deref(), Some("skins_error_reauth"));
    assert_eq!(rig.calls.lock().unwrap().len(), 2, "one refresh, one retry, no more");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_429_says_try_again_later() {
    let rig = rig().await;
    rig.fake.knobs().profile_status = Some(429);
    assert_eq!(message(rig.svc.state("steve").await).as_deref(), Some("skins_error_rate"));
    rig.fake.knobs().profile_status = None;
    let file = rig.dir.path().join("a.png");
    std::fs::write(&file, skin_png(2, false)).unwrap();
    let entry = rig.svc.import_file(&file.to_string_lossy(), None).unwrap();
    rig.fake.knobs().upload_status = Some(400);
    assert_eq!(message(rig.svc.apply("steve", &entry.id).await).as_deref(), Some("skins_error_rejected"));
    assert!(rig.forgotten.lock().unwrap().is_empty(), "nothing changed, nothing forgotten");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_needing_sign_in_says_so() {
    let rig = rig().await;
    assert_eq!(message(rig.svc.state("gone").await).as_deref(), Some("skins_error_reauth"));
    assert!(rig.fake.all_requests().is_empty(), "nothing is asked without a token");
}

#[tokio::test(flavor = "multi_thread")]
async fn import_player_adds_the_player_s_skin() {
    let rig = rig().await;
    rig.fake.add_texture("alex", skin_png(5, true));
    rig.fake.add_player(Player {
        id: "bb22".into(),
        name: "Alex".into(),
        texture: Some("alex".into()),
        slim: true,
    });
    let entry = rig.svc.import_player(" Alex ").await.unwrap();
    assert_eq!(
        (entry.name.as_str(), entry.source.as_str(), entry.variant),
        ("Alex", "player:Alex", SkinVariant::Slim)
    );
    assert_eq!(rig.svc.import_player("Alex").await.unwrap().id, entry.id, "the same skin is one entry");
    assert_eq!(message(rig.svc.import_player("Nobody").await).as_deref(), Some("skins_error_no_player"));
    assert_eq!(
        rig.svc.skin(&entry.id).unwrap(),
        module_skins::backend::png::data_url(&read_skin(&skin_png(5, true)).unwrap().png)
    );
}

fn wear(fake: &Fake, texture: &str, slim: bool) {
    fake.account().skin = Some((texture.into(), slim));
}

fn two_capes(fake: &Fake) {
    let mut account = fake.account();
    account.capes = vec![
        ("c1".into(), "Migrator".into(), "cape1".into()),
        ("c2".into(), "Vanilla".into(), "cape2".into()),
    ];
    account.active_cape = Some("c1".into());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_profile_is_read() {
    let fake = fake_services::start().await;
    wear(&fake, "steve", true);
    two_capes(&fake);
    let profile = client(&fake).profile(TOKEN).await.unwrap();
    assert_eq!(profile.skins.len(), 1);
    assert_eq!(profile.skins[0].state, "ACTIVE");
    assert_eq!(profile.skins[0].variant, SkinVariant::Slim, "Mojang's SLIM reads as slim");
    assert_eq!(profile.skins[0].url, fake.texture_url("steve"));
    let capes: Vec<(&str, &str, &str)> =
        profile.capes.iter().map(|c| (c.id.as_str(), c.alias.as_str(), c.state.as_str())).collect();
    assert_eq!(capes, [("c1", "Migrator", "ACTIVE"), ("c2", "Vanilla", "INACTIVE")]);
    assert_eq!(fake.requests("/profile")[0].bearer.as_deref(), Some(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skin_is_uploaded_as_multipart() {
    let fake = fake_services::start().await;
    let png = b"\x89PNG fake skin".to_vec();
    let profile = client(&fake).upload_skin(TOKEN, png.clone(), SkinVariant::Slim).await.unwrap();
    let sent = &fake.requests("/skins")[0];
    assert_eq!((sent.method.as_str(), sent.bearer.as_deref()), ("POST", Some(TOKEN)));
    assert_eq!(sent.fields.get("variant").map(Vec::as_slice), Some(&b"slim"[..]));
    assert_eq!(sent.fields.get("file"), Some(&png));
    assert_eq!(profile.skins[0].variant, SkinVariant::Slim, "the answer is the new profile");
    assert!(fake.requests("/profile").is_empty(), "no second request when the answer is a profile");

    fake.knobs().bare_answers = true;
    let profile = client(&fake).upload_skin(TOKEN, png, SkinVariant::Classic).await.unwrap();
    assert_eq!(profile.skins[0].variant, SkinVariant::Classic);
    assert_eq!(fake.requests("/profile").len(), 1, "an empty answer is followed by the profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cape_is_put_on_and_taken_off() {
    let fake = fake_services::start().await;
    two_capes(&fake);
    let client = client(&fake);
    let profile = client.set_cape(TOKEN, Some("c2")).await.unwrap();
    let put = &fake.requests("/capes")[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.json, Some(serde_json::json!({"capeId": "c2"})));
    let active: Vec<&str> =
        profile.capes.iter().filter(|c| c.state == "ACTIVE").map(|c| c.id.as_str()).collect();
    assert_eq!(active, ["c2"]);
    let profile = client.set_cape(TOKEN, None).await.unwrap();
    assert_eq!(fake.requests("/capes")[1].method, "DELETE");
    assert!(profile.capes.iter().all(|c| c.state != "ACTIVE"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_player_s_skin_is_found_by_name() {
    let fake = fake_services::start().await;
    fake.add_player(Player {
        id: "aa11".into(),
        name: "Steve_42".into(),
        texture: Some("s42".into()),
        slim: false,
    });
    fake.add_player(Player {
        id: "bb22".into(),
        name: "Alex".into(),
        texture: Some("alex".into()),
        slim: true,
    });
    let client = client(&fake);
    assert_eq!(
        client.player_skin("  Steve_42 ").await.unwrap(),
        (fake.texture_url("s42"), SkinVariant::Classic)
    );
    assert_eq!(client.player_skin("Alex").await.unwrap(), (fake.texture_url("alex"), SkinVariant::Slim));
    assert_eq!(fake.requests("/name/Steve_42").len(), 1, "the name is trimmed");
    assert_eq!(fake.requests("/session/aa11").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_player_is_not_found() {
    let fake = fake_services::start().await;
    let client = client(&fake);
    assert_eq!(client.player_skin("Nobody").await, Err(CallError::NotFound));
    fake.knobs().unknown_is_204 = true;
    assert_eq!(client.player_skin("Nobody").await, Err(CallError::NotFound));
    let refused = client.player_skin("two words").await;
    assert!(matches!(refused, Err(CallError::Other(_))), "{refused:?}");
    assert!(fake.all_requests().iter().all(|r| !r.path.contains("two")), "a bad name asks nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn statuses_map_to_errors() {
    let fake = fake_services::start().await;
    let client = client(&fake);
    fake.knobs().valid_token = Some("another".into());
    assert_eq!(client.profile(TOKEN).await, Err(CallError::Unauthorized));
    fake.knobs().valid_token = None;
    fake.knobs().profile_status = Some(429);
    assert_eq!(client.profile(TOKEN).await, Err(CallError::Rate));
    fake.knobs().upload_status = Some(400);
    assert_eq!(client.upload_skin(TOKEN, vec![1], SkinVariant::Classic).await, Err(CallError::Rejected));
    fake.knobs().cape_status = Some(500);
    assert!(matches!(client.set_cape(TOKEN, None).await, Err(CallError::Other(_))));
}
