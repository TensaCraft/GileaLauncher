mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use launcher_core::auth::crypto::TokenCipher;
use launcher_core::auth::service::AuthService;
use launcher_core::auth::store::{PROFILES_FILE, REASON_DECRYPTION_FAILED, REASON_REFRESH_INVALID};
use launcher_core::auth::{AuthConfig, AuthTimings, NoOpener, UrlOpener};
use launcher_core::feedback::{EventSink, FeedbackService};
use launcher_shared::{
    AccountKind, ActivityEntry, Alert, AuthState, ErrorCode, OpsSnapshot, ProfilesSnapshot, Text, Toast,
};
use serde_json::{Map, Value, json};
use support::fake_ms::{self, Browser, Fake, FakeBrowser, MC_ID, MC_NAME, USER_CODE, XUID};

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
    let mut cfg = AuthConfig::new("test-client", dir, &dir.join("cache"));
    cfg.endpoints = fake.endpoints();
    cfg.timings = AuthTimings::fast();
    cfg.avatar_sources = fake.avatar_sources();
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
    let dto = svc.sign_in_microsoft("en_US").await.unwrap();
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
    svc.sign_in_microsoft("en_US").await.unwrap();
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
    let err = svc.sign_in_microsoft("en_US").await.unwrap_err();
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
        tokio::spawn(async move { svc.sign_in_microsoft("en_US").await })
    };
    for _ in 0..200 {
        if svc.auth_state() == AuthState::Browser {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(svc.auth_state(), AuthState::Browser);
    assert_eq!(svc.sign_in_microsoft("en_US").await.unwrap_err().code, ErrorCode::Busy);
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
