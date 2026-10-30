mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use launcher_core::auth::crypto::TokenCipher;
use launcher_core::auth::page::PageLook;
use launcher_core::auth::provider::{AccountProvider, ProviderAccount, ProviderFuture};
use launcher_core::auth::servers::Preset;
use launcher_core::auth::service::AuthService;
use launcher_core::auth::store::{PROFILES_FILE, REASON_DECRYPTION_FAILED, REASON_REFRESH_INVALID};
use launcher_core::auth::{AuthConfig, AuthTimings, NoOpener, UrlOpener};
use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_shared::{
    AccountKind, ActivityEntry, Alert, AuthState, ErrorCode, OpsSnapshot, ProfilesSnapshot, Text, Toast,
};
use serde_json::{Map, Value, json};
use support::fake_ms::{self, Browser, Fake, FakeBrowser, MC_ID, MC_NAME, USER_CODE, XUID};
use support::fake_ygg::{self, FakeYgg};

#[derive(Default)]
struct Rec {
    profiles: Mutex<Vec<ProfilesSnapshot>>,
    auth: Mutex<Vec<AuthState>>,
    toasts: Mutex<Vec<Toast>>,
}

impl EventSink for Rec {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, _: &ActivityEntry) {}
    fn toast(&self, t: &Toast) {
        self.toasts.lock().unwrap().push(t.clone());
    }
    fn alert(&self, _: &Alert) {}
    fn profiles(&self, s: &ProfilesSnapshot) {
        self.profiles.lock().unwrap().push(s.clone());
    }
    fn auth(&self, s: &AuthState) {
        self.auth.lock().unwrap().push(s.clone());
    }
}

fn service(fake: &Fake, dir: &Path, opener: Arc<dyn UrlOpener>) -> (Arc<AuthService>, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let mut cfg = AuthConfig::new("test-client", dir);
    cfg.endpoints = fake.endpoints();
    cfg.timings = AuthTimings::fast();
    let svc = AuthService::new(cfg, FeedbackService::new(rec.clone()), rec.clone(), opener).unwrap();
    (svc, rec)
}

/// Adds a record to `profiles.json` as an earlier run would have left it.
fn put(dir: &Path, key: &str, record: Value) {
    let path = dir.join(PROFILES_FILE);
    let mut all: Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.insert(key.into(), record);
    std::fs::write(&path, Value::Object(all).to_string()).unwrap();
}

fn stored_microsoft(dir: &Path, name: &str, access: Option<&str>, expires_at: i64, default: bool) {
    let cipher = TokenCipher::new(dir);
    put(
        dir,
        name,
        json!({
            "id": MC_ID,
            "name": name,
            "access_token": access.map(|a| cipher.encrypt(a).unwrap()),
            "refresh_token": cipher.encrypt(&format!("refresh-of-{name}")).unwrap(),
            "xuid": XUID,
            "expires_at": expires_at,
            "auth_client_id": "test-client",
            "default": default
        }),
    );
}

/// Refresh tokens sent to the token endpoint, in order.
fn refresh_requests(fake: &Fake) -> Vec<String> {
    fake.requests("/token")
        .into_iter()
        .filter(|r| r.form.get("grant_type").map(String::as_str) == Some("refresh_token"))
        .filter_map(|r| r.form.get("refresh_token").cloned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_sign_in_creates_a_default_microsoft_profile() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (svc, rec) = service(&fake, dir.path(), FakeBrowser::new(Browser::Consent));
    let dto = svc.sign_in_microsoft(PageLook::plain("en_US")).await.unwrap();
    assert_eq!((dto.name.as_str(), dto.id.as_str()), (MC_NAME, MC_ID));
    assert_eq!((dto.kind, dto.is_default, dto.reauth_required), (AccountKind::Microsoft, true, false));
    let text = std::fs::read_to_string(dir.path().join(PROFILES_FILE)).unwrap();
    assert!(!text.contains("ms-refresh-1") && text.contains("enc::"), "{text}");
    let identity = svc.launch_identity(None).await.unwrap();
    assert_eq!(identity.username, MC_NAME);
    assert_eq!(identity.uuid, MC_ID);
    assert_eq!(identity.xuid.as_deref(), Some(XUID));
    assert_eq!(identity.client_id.as_deref(), Some("test-client"));
    assert_eq!(fake.requests("/token").len(), 1, "a fresh session is not refreshed again");
    assert_eq!(*rec.auth.lock().unwrap(), [AuthState::Browser, AuthState::Finishing, AuthState::Idle]);
    assert_eq!(rec.profiles.lock().unwrap().last().unwrap().profiles[0].name, MC_NAME);
    assert_eq!(svc.auth_state(), AuthState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn falls_back_to_a_device_code_when_no_browser_opens() {
    let fake = fake_ms::start().await;
    fake.knobs().device_pending = 1;
    let dir = tempfile::tempdir().unwrap();
    let (svc, rec) = service(&fake, dir.path(), FakeBrowser::new(Browser::Broken));
    svc.sign_in_microsoft(PageLook::plain("en_US")).await.unwrap();
    let toasts = rec.toasts.lock().unwrap().clone();
    assert!(toasts.iter().any(|t| t.title == Text::key("microsoft_auth_browser_fallback")), "{toasts:?}");
    let device = rec
        .auth
        .lock()
        .unwrap()
        .iter()
        .find_map(|s| match s {
            AuthState::DeviceCode { user_code, open_url, .. } => Some((user_code.clone(), open_url.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(device.0, USER_CODE);
    assert!(device.1.ends_with("otc=ABCD-1234"), "{}", device.1);
}

#[tokio::test(flavor = "multi_thread")]
async fn minecraft_outage_does_not_fall_back_to_a_device_code() {
    let fake = fake_ms::start().await;
    fake.knobs().mc_login_status = Some(503);
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(&fake, dir.path(), FakeBrowser::new(Browser::Consent));
    let err = svc.sign_in_microsoft(PageLook::plain("en_US")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::MinecraftServicesUnavailable);
    assert!(fake.requests("/devicecode").is_empty());
    assert!(svc.snapshot().profiles.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_sign_in_is_busy_and_cancel_stops_the_first() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(&fake, dir.path(), FakeBrowser::new(Browser::Idle));
    let first = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.sign_in_microsoft(PageLook::plain("en_US")).await })
    };
    for _ in 0..200 {
        if svc.auth_state() == AuthState::Browser {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(svc.auth_state(), AuthState::Browser);
    assert_eq!(svc.sign_in_microsoft(PageLook::plain("en_US")).await.unwrap_err().code, ErrorCode::Busy);
    svc.cancel_sign_in();
    assert_eq!(first.await.unwrap().unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(svc.auth_state(), AuthState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_keeps_the_old_refresh_token_when_none_is_returned() {
    let fake = fake_ms::start().await;
    fake.knobs().omit_refresh_token = true;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("old-access"), 0, true);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.refresh("Notch_UA", false).await.unwrap();
    svc.refresh("Notch_UA", true).await.unwrap();
    assert_eq!(refresh_requests(&fake), ["refresh-of-Notch_UA", "refresh-of-Notch_UA"]);
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_renews_a_session_that_would_end_during_play() {
    // A Minecraft session lasts a day: one with two hours left would drop out of servers mid-game.
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("old-access"), now() + 2 * 3600, true);
    stored_microsoft(dir.path(), "Steve_UA", Some("day-access"), now() + 20 * 3600, false);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let identity = svc.launch_identity(Some("Notch_UA")).await.unwrap();
    assert_ne!(identity.access_token, "old-access");
    svc.launch_identity(Some("Steve_UA")).await.unwrap();
    assert_eq!(refresh_requests(&fake), ["refresh-of-Notch_UA"], "a session with most of a day left is used");
}

fn warned(rec: &Rec, key: &str) -> bool {
    rec.toasts.lock().unwrap().iter().any(|t| serde_json::to_value(&t.title).unwrap()["key"] == key)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_that_cannot_be_renewed_still_plays_with_a_warning() {
    let fake = fake_ms::start().await;
    fake.knobs().token_failures = 1_000;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("old-access"), 1_000, true);
    let (svc, rec) = service(&fake, dir.path(), Arc::new(NoOpener));
    let identity = svc.launch_identity(None).await.unwrap();
    assert_eq!(identity.access_token, "old-access", "single player needs no session");
    assert!(warned(&rec, "auth_session_not_renewed"), "the player learns why servers may refuse");
    assert!(!svc.snapshot().profiles[0].reauth_required, "no new sign-in is asked for a network failure");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_session_and_no_network_is_told_as_such() {
    let fake = fake_ms::start().await;
    fake.knobs().token_failures = 1_000;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", None, 0, true);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let error = svc.launch_identity(None).await.unwrap_err();
    assert_ne!(error.code, ErrorCode::ReauthRequired, "signing in again would not help: {error:?}");
}

/// A scanner holds the token key for a moment: the accounts are not to be signed into again.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_token_key_unreadable_for_a_moment_asks_for_no_new_sign_in() {
    use std::os::windows::fs::OpenOptionsExt;
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("day-access"), now() + 20 * 3600, true);
    let key = dir.path().join(launcher_core::auth::crypto::KEY_FILE);
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&key).unwrap();
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let error = svc.launch_identity(None).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::CredentialStorageUnavailable);
    assert!(!svc.snapshot().profiles[0].reauth_required, "nothing is wrong with the account");
    drop(held);
    assert_eq!(svc.launch_identity(None).await.unwrap().access_token, "day-access");
    assert!(fake.requests("/token").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_access_token_forces_a_refresh() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", None, 4_000_000_000, true);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let refreshed = svc.refresh("Notch_UA", false).await.unwrap();
    assert!(refreshed.access_token.is_some());
    svc.refresh("Notch_UA", false).await.unwrap();
    assert_eq!(refresh_requests(&fake).len(), 1, "fresh after the first refresh");
}

#[tokio::test(flavor = "multi_thread")]
async fn undecryptable_tokens_are_never_sent() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "Notch_UA",
        json!({ "id": MC_ID, "name": "Notch_UA", "access_token": "enc::garbage",
                "refresh_token": "enc::garbage", "expires_at": 0, "default": true }),
    );
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let profile = svc.refresh("Notch_UA", false).await.unwrap();
    assert!(fake.requests("/token").is_empty());
    assert_eq!(profile.to_dto().reauth_reason.as_deref(), Some(REASON_DECRYPTION_FAILED));
    assert_eq!(svc.launch_identity(None).await.unwrap_err().code, ErrorCode::ReauthRequired);
    assert!(svc.snapshot().profiles[0].reauth_required);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_all_includes_non_default_profiles() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Alpha", Some("a"), 0, true);
    stored_microsoft(dir.path(), "Beta", Some("b"), 0, false);
    put(
        dir.path(),
        "Steve",
        json!({ "name": "Steve", "access_token": "offline", "refresh_token": "offline" }),
    );
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.refresh_all().await;
    let mut sent = refresh_requests(&fake);
    sent.sort();
    assert_eq!(sent, ["refresh-of-Alpha", "refresh-of-Beta"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_refresh_token_asks_for_a_new_sign_in() {
    let fake = fake_ms::start().await;
    fake.knobs().refresh_error = Some("invalid_grant".into());
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("a"), 0, true);
    let (svc, rec) = service(&fake, dir.path(), Arc::new(NoOpener));
    let profile = svc.refresh("Notch_UA", false).await.unwrap();
    assert_eq!(profile.to_dto().reauth_reason.as_deref(), Some(REASON_REFRESH_INVALID));
    assert!(rec.profiles.lock().unwrap().last().unwrap().profiles[0].reauth_required);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_failure_keeps_the_stale_session_unmarked() {
    let fake = fake_ms::start().await;
    fake.knobs().token_failures = 100;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("stale"), 0, true);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let profile = svc.refresh("Notch_UA", false).await.unwrap();
    assert_eq!(profile.access_token.as_deref(), Some("stale"));
    assert!(profile.reauth_reason().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_refreshes_hit_the_network_once() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("a"), 0, true);
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let (a, b) = tokio::join!(svc.refresh("Notch_UA", false), svc.refresh("Notch_UA", false));
    assert_eq!(a.unwrap().access_token, b.unwrap().access_token);
    assert_eq!(refresh_requests(&fake).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_profile_rules() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    stored_microsoft(dir.path(), "Notch_UA", Some("mc-access"), 4_000_000_000, true);
    let (svc, rec) = service(&fake, dir.path(), Arc::new(NoOpener));
    let snapshot = svc.create_offline(" Steve ").unwrap();
    assert_eq!(snapshot.profiles[0].name, "Steve");
    assert_eq!(snapshot.profiles[0].id, "5627dd98-e6be-bc21-f8a8-e92344183641");
    assert!(!rec.profiles.lock().unwrap().is_empty());
    assert_eq!(svc.create_offline("Steve").unwrap_err().code, ErrorCode::ProfileExists);
    assert_eq!(svc.create_offline("Notch_UA").unwrap_err().code, ErrorCode::ProfileExists);
    assert_eq!(svc.create_offline("two words").unwrap_err().code, ErrorCode::ProfileNameInvalid);
    assert_eq!(
        svc.create_offline("Іван").unwrap_err().code,
        ErrorCode::ProfileNameInvalid,
        "Minecraft lets in names of visible ASCII only"
    );
    let microsoft = svc.launch_identity(Some("Notch_UA")).await.unwrap();
    assert_eq!(microsoft.access_token, "mc-access", "the Microsoft profile is intact");
    let offline = svc.launch_identity(Some("Steve")).await.unwrap();
    assert_eq!((offline.kind, offline.access_token.as_str()), (AccountKind::Offline, "offline"));
    assert!(fake.requests("/token").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_and_choosing_the_default_profile() {
    let fake = fake_ms::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.create_offline("Alex").unwrap();
    svc.create_offline("Mia").unwrap();
    let snapshot = svc.delete("Mia").unwrap();
    assert!(snapshot.profiles[0].is_default && snapshot.profiles[0].name == "Alex");
    assert_eq!(svc.set_default("nobody").unwrap_err().code, ErrorCode::NotFound);
    svc.delete("Alex").unwrap();
    assert_eq!(svc.launch_identity(None).await.unwrap_err().code, ErrorCode::NoProfile);
}

/// A service whose Ely.by preset is the local fake auth server.
fn with_server(fake: &Fake, ygg: &FakeYgg, dir: &Path) -> (Arc<AuthService>, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let mut cfg = AuthConfig::new("test-client", dir);
    cfg.endpoints = fake.endpoints();
    cfg.timings = AuthTimings::fast();
    cfg.servers = vec![Preset { id: "elyby", name: "Ely.by".into(), address: ygg.base.clone() }];
    let svc =
        AuthService::new(cfg, FeedbackService::new(rec.clone()), rec.clone(), Arc::new(NoOpener)).unwrap();
    (svc, rec)
}

const ELY_KEY: &str = "elyby:Alex_UA";

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_on_another_server_adds_its_account() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, rec) = with_server(&fake, &ygg, dir.path());
    let dto = svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    assert_eq!(dto.key, ELY_KEY, "the server and the name: the same nick elsewhere is another account");
    assert_eq!((dto.name.as_str(), dto.id.as_str()), (fake_ygg::PROFILE_NAME, fake_ygg::PROFILE_ID));
    assert_eq!(dto.kind, AccountKind::Yggdrasil);
    assert_eq!(
        (dto.server.as_deref(), dto.server_name.as_deref()),
        (Some("elyby"), Some(fake_ygg::SERVER_NAME))
    );
    assert!(dto.is_default && !dto.reauth_required);
    assert!(rec.profiles.lock().unwrap().last().unwrap().profiles.iter().any(|p| p.key == ELY_KEY));
    let stored = std::fs::read_to_string(dir.path().join(PROFILES_FILE)).unwrap();
    assert!(!stored.contains("token-1"), "the session is sealed like Microsoft's");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_factor_is_asked_for_and_nothing_is_kept_until_it_comes() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    ygg.set(|k| k.two_factor = true);
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    let asked = svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap_err();
    assert_eq!(asked.code, ErrorCode::AuthCodeRequired);
    assert!(svc.snapshot().profiles.is_empty());
    let dto =
        svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, Some(fake_ygg::TOTP)).await.unwrap();
    assert_eq!(dto.key, ELY_KEY);
}

/// The player names no server: only Ely.by and the servers modules register (owner, 2026-10-30).
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_goes_only_to_a_known_server_never_to_an_address() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    let err = svc.sign_in_server(&ygg.base, fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(ygg.posts("authserver/authenticate").is_empty(), "nothing was sent there");
    svc.register_server("tensa", &ygg.base, "tensa").await.unwrap();
    let dto = svc.sign_in_server("tensa", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    assert_eq!(dto.key, format!("tensa:{}", fake_ygg::PROFILE_NAME));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_on_such_an_account_carries_the_agent_and_its_server() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    let identity = svc.launch_identity(Some(ELY_KEY)).await.unwrap();
    assert_eq!(identity.uuid, fake_ygg::PROFILE_ID, "no dashes, as the agent's servers expect");
    let agent = identity.agent.unwrap();
    assert_eq!(agent.api_root, ygg.api_root());
    assert!(agent.metadata.contains(fake_ygg::SERVER_NAME), "prefetched for the game");
    assert_eq!(ygg.posts("authserver/validate").len(), 1, "the session is checked before a launch");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_session_is_renewed_before_a_launch_and_an_ended_one_asks_for_the_password() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    ygg.stale_all();
    let renewed = svc.launch_identity(Some(ELY_KEY)).await.unwrap();
    assert_eq!(renewed.access_token, "token-2");
    ygg.end_all();
    let err = svc.launch_identity(Some(ELY_KEY)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ReauthRequired);
    let profile = svc.snapshot().profiles.into_iter().find(|p| p.key == ELY_KEY).unwrap();
    assert!(profile.reauth_required, "the card asks for a new sign-in");
}

/// Two builds started at once with a stale session: one renews it, the other plays on the renewed
/// one (the server takes the old token back once: a second renewal of it would end the account).
#[tokio::test(flavor = "multi_thread")]
async fn two_launches_at_once_renew_a_stale_session_once() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    ygg.stale_all();
    let (a, b) = tokio::join!(svc.launch_identity(Some(ELY_KEY)), svc.launch_identity(Some(ELY_KEY)));
    assert_eq!(a.unwrap().access_token, b.unwrap().access_token);
    assert_eq!(ygg.posts("authserver/refresh").len(), 1);
    let profile = svc.snapshot().profiles.into_iter().find(|p| p.key == ELY_KEY).unwrap();
    assert!(!profile.reauth_required);
}

/// A nickname taken on Ely.by since the sign-in: the renewed session names it, and the game
/// starts under it (the account stays the one it was).
#[tokio::test(flavor = "multi_thread")]
async fn a_renewed_session_brings_the_nickname_taken_since() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    ygg.set(|k| k.renamed = Some("Alex_New".into()));
    ygg.stale_all();
    let identity = svc.launch_identity(Some(ELY_KEY)).await.unwrap();
    assert_eq!(identity.username, "Alex_New");
    let card = svc.snapshot().profiles.into_iter().find(|p| p.key == ELY_KEY).unwrap();
    assert_eq!(card.name, "Alex_New");
}

/// Microsoft renews Microsoft accounts only: another server's token never goes there.
#[tokio::test(flavor = "multi_thread")]
async fn a_microsoft_refresh_leaves_an_account_of_another_server_alone() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    let kept = svc.refresh(ELY_KEY, true).await.unwrap();
    assert_eq!(kept.access_token.as_deref(), Some("token-1"));
    assert!(fake.requests("/token").is_empty(), "nothing went to Microsoft");
    let card = svc.snapshot().profiles.into_iter().find(|p| p.key == ELY_KEY).unwrap();
    assert!(!card.reauth_required);
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_such_an_account_ends_its_session_on_the_server() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    svc.delete_signed_out(ELY_KEY).await.unwrap();
    assert!(svc.snapshot().profiles.is_empty());
    assert_eq!(ygg.posts("authserver/invalidate").len(), 1);
}

/// Mojang's skin of the same nick is someone else's: an account on another server wears its
/// own server's skin.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_on_another_server_is_a_player_of_that_server() {
    let (fake, ygg, dir) = (fake_ms::start().await, fake_ygg::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = with_server(&fake, &ygg, dir.path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    let player = svc.player(ELY_KEY).unwrap();
    assert_eq!(player.session.as_deref(), Some(ygg.session().as_str()));
    svc.create_offline("Steve").unwrap();
    assert_eq!(svc.player("Steve").unwrap().session, None, "Mojang's");
}

/// A module's site sign-in, as a test sees it: one account, an optional second factor, a token
/// it may end.
#[derive(Default)]
struct Site {
    two_factor: bool,
    ended: Mutex<bool>,
    banned: Mutex<bool>,
    /// The nickname the player took on the site since signing in.
    renamed: Mutex<Option<String>>,
    /// A sign-out the site never answers.
    silent_sign_out: bool,
    signed_out: Mutex<Vec<String>>,
}

const SITE_ID: &str = "2ca6c1be-e03b-32d5-a995-86a64e1a3b2a";

impl AccountProvider for Site {
    fn id(&self) -> &'static str {
        "tensa"
    }
    fn name(&self) -> String {
        "Сайт".into()
    }
    fn sign_in<'a>(
        &'a self,
        login: &'a str,
        password: &'a str,
        code: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move {
            if login != "Builder" || password != "pw" {
                return Err(launcher_shared::AppError::new(
                    ErrorCode::AuthCredentialsInvalid,
                    "invalid_credentials",
                ));
            }
            if self.two_factor && code.is_none() {
                return Err(launcher_shared::AppError::new(ErrorCode::AuthCodeRequired, "2fa"));
            }
            Ok(ProviderAccount { id: SITE_ID.into(), name: "Builder".into(), token: "site-token".into() })
        })
    }
    fn verify<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move {
            if *self.ended.lock().unwrap() {
                return Err(launcher_shared::AppError::new(ErrorCode::ReauthRequired, "invalid_token"));
            }
            if *self.banned.lock().unwrap() {
                return Err(launcher_shared::AppError::new(ErrorCode::AuthDenied, "user_banned"));
            }
            let name = self.renamed.lock().unwrap().clone().unwrap_or_else(|| "Builder".into());
            Ok(ProviderAccount { id: SITE_ID.into(), name, token: token.into() })
        })
    }
    fn sign_out<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            if self.silent_sign_out {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
            self.signed_out.lock().unwrap().push(token.to_string());
            Ok(())
        })
    }
    fn skin_url(&self, name: &str) -> Option<String> {
        Some(format!("https://site.example/skins/{name}.png"))
    }
}

const SITE_KEY: &str = "tensa:Builder";

#[tokio::test(flavor = "multi_thread")]
async fn a_module_s_site_signs_its_players_in() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.register_provider(Arc::new(Site { two_factor: true, ..Site::default() }));
    let asked = svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap_err();
    assert_eq!(asked.code, ErrorCode::AuthCodeRequired);
    let wrong = svc.sign_in_server("tensa", "Builder", "nope", Some("1")).await.unwrap_err();
    assert_eq!(wrong.code, ErrorCode::AuthCredentialsInvalid);
    let dto = svc.sign_in_server("tensa", "Builder", "pw", Some("123456")).await.unwrap();
    assert_eq!((dto.key.as_str(), dto.name.as_str(), dto.id.as_str()), (SITE_KEY, "Builder", SITE_ID));
    assert_eq!(dto.kind, AccountKind::Module);
    assert_eq!((dto.server.as_deref(), dto.server_name.as_deref()), (Some("tensa"), Some("Сайт")));
    let stored = std::fs::read_to_string(dir.path().join(PROFILES_FILE)).unwrap();
    assert!(!stored.contains("site-token"), "sealed like the others");
}

/// The owner's choice «А»: the site vouches for who plays, the game starts as an offline player
/// of that name and UUID.
#[tokio::test(flavor = "multi_thread")]
async fn a_site_account_launches_offline_style_once_the_site_vouches_for_it() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let site = Arc::new(Site::default());
    svc.register_provider(site.clone());
    svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap();
    let identity = svc.launch_identity(Some(SITE_KEY)).await.unwrap();
    assert_eq!((identity.username.as_str(), identity.uuid.as_str()), ("Builder", SITE_ID));
    assert_eq!(identity.access_token, "offline");
    assert!(identity.agent.is_none());
    *site.ended.lock().unwrap() = true;
    let err = svc.launch_identity(Some(SITE_KEY)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ReauthRequired, "a sign-in elsewhere ended the token");
    assert!(svc.snapshot().profiles.iter().find(|p| p.key == SITE_KEY).unwrap().reauth_required);
    svc.delete_signed_out(SITE_KEY).await.unwrap();
    assert_eq!(*site.signed_out.lock().unwrap(), ["site-token"]);
}

/// The site's word stands at each launch: a banned player does not play, a renamed one plays
/// under the new nickname (the account stays the one it was).
#[tokio::test(flavor = "multi_thread")]
async fn a_banned_player_does_not_launch_and_a_renamed_one_plays_under_the_new_nick() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let site = Arc::new(Site::default());
    svc.register_provider(site.clone());
    svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap();
    *site.renamed.lock().unwrap() = Some("Architect".into());
    let identity = svc.launch_identity(Some(SITE_KEY)).await.unwrap();
    assert_eq!((identity.username.as_str(), identity.uuid.as_str()), ("Architect", SITE_ID));
    let card = svc.snapshot().profiles.into_iter().find(|p| p.key == SITE_KEY).unwrap();
    assert_eq!(card.name, "Architect", "the card follows");
    *site.banned.lock().unwrap() = true;
    let err = svc.launch_identity(Some(SITE_KEY)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthDenied);
}

/// A sign-out the server never answers does not hold the deletion: the account goes at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_does_not_answer_a_sign_out_does_not_hold_a_deletion() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.register_provider(Arc::new(Site { silent_sign_out: true, ..Site::default() }));
    svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap();
    let gone =
        tokio::time::timeout(std::time::Duration::from_secs(10), svc.delete_signed_out(SITE_KEY)).await;
    assert!(gone.expect("not held by the site").unwrap().profiles.is_empty());

    let ygg = fake_ygg::start().await;
    ygg.set(|k| k.silent_sign_out = true);
    let (svc, _) = with_server(&fake, &ygg, tempfile::tempdir().unwrap().path());
    svc.sign_in_server("elyby", fake_ygg::LOGIN, fake_ygg::PASSWORD, None).await.unwrap();
    let gone = tokio::time::timeout(std::time::Duration::from_secs(10), svc.delete_signed_out(ELY_KEY)).await;
    assert!(gone.expect("not held by Ely.by").unwrap().profiles.is_empty());
}

/// A site account wears the site's skin of its nickname, and nobody else's.
#[tokio::test(flavor = "multi_thread")]
async fn a_site_account_is_a_player_of_its_site() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    svc.register_provider(Arc::new(Site::default()));
    svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap();
    let player = svc.player(SITE_KEY).unwrap();
    assert_eq!(player.site, Some(Some("https://site.example/skins/Builder.png".to_string())));
    assert_eq!(player.session, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_its_module_a_site_account_cannot_sign_in() {
    let (fake, dir) = (fake_ms::start().await, tempfile::tempdir().unwrap());
    let (svc, _) = service(&fake, dir.path(), Arc::new(NoOpener));
    let err = svc.sign_in_server("tensa", "Builder", "pw", None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
