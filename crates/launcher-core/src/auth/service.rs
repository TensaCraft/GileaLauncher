//! Accounts service: the profile list, Microsoft sign-in (browser, then a device code), token
//! refresh and the identity a launch uses.

use std::sync::{Arc, Mutex, MutexGuard};

use launcher_shared::{
    AccountKind, AppError, AppResult, AuthState, ErrorCode, Level, ProfileDto, ProfilesSnapshot, Text,
};
use tokio_util::sync::CancellationToken;

use super::avatars::{Avatar, AvatarCache};
use super::http::{AuthHttp, FlowError};
use super::msa::{self, MsTokens};
use super::offline::validate_offline_name;
use super::store::{NewProfile, ProfileStore, REASON_REFRESH_INVALID, StoredProfile, TokenUpdate};
use super::{AuthConfig, UrlOpener, now_secs, xbox};
use crate::feedback::{EventSink, FeedbackService};

/// Who plays: what the launch passes to Minecraft. Stays inside `launcher-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIdentity {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    pub kind: AccountKind,
}

pub struct AuthService {
    cfg: AuthConfig,
    store: ProfileStore,
    http: AuthHttp,
    feedback: Arc<FeedbackService>,
    sink: Arc<dyn EventSink>,
    opener: Arc<dyn UrlOpener>,
    /// One Microsoft sign-in at a time; a second one is `Busy`.
    sign_in: tokio::sync::Mutex<()>,
    /// Token refreshes run one after another and re-read the store first.
    refresh: tokio::sync::Mutex<()>,
    cancel: Mutex<Option<CancellationToken>>,
    state: Mutex<AuthState>,
    avatars: AvatarCache,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn no_profile(key: &str) -> AppError {
    AppError::new(ErrorCode::NoProfile, "the profile does not exist")
        .with_param("reason", "missing_profile")
        .with_param("profile_key", key)
}

/// Refresh failures after which only a new sign-in helps.
fn is_revoked(error: &FlowError) -> bool {
    match error {
        FlowError::Failed(detail) => {
            let detail = detail.to_lowercase();
            ["invalid_grant", "expired", "revoked"].iter().any(|marker| detail.contains(marker))
        }
        _ => false,
    }
}

impl AuthService {
    pub fn new(
        cfg: AuthConfig,
        feedback: Arc<FeedbackService>,
        sink: Arc<dyn EventSink>,
        opener: Arc<dyn UrlOpener>,
    ) -> AppResult<Arc<Self>> {
        let http = AuthHttp::new(&cfg.timings)?;
        let store = ProfileStore::open(&cfg.state_dir);
        let avatars = AvatarCache::new(
            cfg.avatar_dir.clone(),
            cfg.avatar_sources.clone(),
            http.client().clone(),
            cfg.timings.avatar_timeout,
        );
        Ok(Arc::new(AuthService {
            cfg,
            store,
            http,
            feedback,
            sink,
            opener,
            sign_in: tokio::sync::Mutex::new(()),
            refresh: tokio::sync::Mutex::new(()),
            cancel: Mutex::new(None),
            state: Mutex::new(AuthState::Idle),
            avatars,
        }))
    }

    pub fn snapshot(&self) -> ProfilesSnapshot {
        self.store.snapshot()
    }

    /// The launcher has a profile of `key`.
    pub fn has_profile(&self, key: &str) -> bool {
        self.store.get(key).is_some()
    }

    pub fn auth_state(&self) -> AuthState {
        lock(&self.state).clone()
    }

    fn set_state(&self, state: AuthState) {
        *lock(&self.state) = state.clone();
        self.sink.auth(&state);
    }

    /// Emits and returns the current list.
    fn changed(&self) -> ProfilesSnapshot {
        let snapshot = self.snapshot();
        self.sink.profiles(&snapshot);
        snapshot
    }

    /// A new offline profile becomes the default; an existing name is never replaced.
    pub fn create_offline(&self, name: &str) -> AppResult<ProfilesSnapshot> {
        let name = validate_offline_name(name)?;
        self.store.save(&name, NewProfile::offline(&name), false)?;
        Ok(self.changed())
    }

    pub fn delete(&self, key: &str) -> AppResult<ProfilesSnapshot> {
        self.store.delete(key)?;
        Ok(self.changed())
    }

    pub fn set_default(&self, key: &str) -> AppResult<ProfilesSnapshot> {
        self.store.set_default(key)?;
        Ok(self.changed())
    }

    pub fn cancel_sign_in(&self) {
        if let Some(token) = lock(&self.cancel).as_ref() {
            token.cancel();
        }
    }

    /// Browser sign-in, falling back to a device code when the browser flow is unavailable.
    /// The new Microsoft profile becomes the default (an existing one with the same name is updated).
    pub async fn sign_in_microsoft(&self, lang: &str) -> AppResult<ProfileDto> {
        let Ok(_guard) = self.sign_in.try_lock() else {
            return Err(AppError::new(ErrorCode::Busy, "a Microsoft sign-in is already running"));
        };
        let cancel = CancellationToken::new();
        *lock(&self.cancel) = Some(cancel.clone());
        let result = self.run_sign_in(lang, &cancel).await;
        *lock(&self.cancel) = None;
        self.set_state(AuthState::Idle);
        match result {
            Ok(profile) => {
                tracing::info!("Microsoft profile {} signed in", profile.key);
                self.changed();
                Ok(profile.to_dto())
            }
            Err(error) => {
                tracing::warn!("Microsoft sign-in failed: {}", error.detail);
                Err(error)
            }
        }
    }

    async fn run_sign_in(&self, lang: &str, cancel: &CancellationToken) -> AppResult<StoredProfile> {
        let ep = &self.cfg.endpoints;
        let client_id = self.cfg.client_id.as_str();
        self.feedback.info(Text::key("microsoft_auth_browser_opening"));
        self.set_state(AuthState::Browser);
        let browser = msa::browser_sign_in(
            &self.http,
            ep,
            client_id,
            self.opener.as_ref(),
            lang,
            self.cfg.timings.browser_timeout,
            cancel,
        )
        .await;
        let tokens = match browser {
            Ok(tokens) => tokens,
            Err(FlowError::Unavailable(reason)) => {
                tracing::warn!("Browser sign-in unavailable ({reason}); using a device code");
                self.feedback.toast(Level::Warning, Text::key("microsoft_auth_browser_fallback"), None, None);
                self.device_sign_in(cancel).await.map_err(FlowError::into_app_error)?
            }
            Err(error) => return Err(error.into_app_error()),
        };
        if cancel.is_cancelled() {
            return Err(FlowError::Cancelled.into_app_error());
        }
        self.set_state(AuthState::Finishing);
        let session = xbox::minecraft_session(&self.http, ep, &tokens.access_token, now_secs())
            .await
            .map_err(FlowError::into_app_error)?;
        let profile = NewProfile {
            id: Some(session.id),
            name: session.name.clone(),
            access_token: Some(session.access_token),
            refresh_token: tokens.refresh_token,
            xuid: session.xuid,
            expires_at: Some(session.expires_at),
            auth_client_id: Some(self.cfg.client_id.clone()),
        };
        self.store.save(&session.name, profile, true)
    }

    async fn device_sign_in(&self, cancel: &CancellationToken) -> Result<MsTokens, FlowError> {
        let ep = &self.cfg.endpoints;
        let code = msa::device_start(&self.http, ep, &self.cfg.client_id).await?;
        self.set_state(AuthState::DeviceCode {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            open_url: code.open_url.clone(),
        });
        if !self.opener.open(&code.open_url) {
            tracing::info!("The device code page was not opened automatically");
        }
        msa::device_poll(&self.http, ep, &self.cfg.client_id, &code, &self.cfg.timings, cancel).await
    }

    /// `Some` when no network refresh is needed (or none can help).
    fn settled(&self, profile: &StoredProfile, force: bool) -> AppResult<Option<StoredProfile>> {
        if profile.is_offline() {
            return Ok(Some(profile.clone()));
        }
        if let Some(reason) = profile.reauth_reason() {
            if !profile.is_marked() {
                self.store.mark_reauth(&profile.key, &reason)?;
                self.changed();
            }
            return Ok(Some(self.store.get(&profile.key).unwrap_or_else(|| profile.clone())));
        }
        if !force && profile.is_fresh(now_secs()) {
            return Ok(Some(profile.clone()));
        }
        Ok(None)
    }

    async fn renew(&self, refresh_token: &str) -> Result<TokenUpdate, FlowError> {
        let ep = &self.cfg.endpoints;
        let tokens = msa::refresh_tokens(&self.http, ep, &self.cfg.client_id, refresh_token).await?;
        let session = xbox::minecraft_session(&self.http, ep, &tokens.access_token, now_secs()).await?;
        Ok(TokenUpdate {
            id: session.id,
            name: session.name,
            access_token: session.access_token,
            refresh_token: tokens.refresh_token,
            xuid: session.xuid,
            expires_at: session.expires_at,
            auth_client_id: self.cfg.client_id.clone(),
        })
    }

    /// Brings a Microsoft profile's tokens up to date. A network failure keeps
    /// the current session; a rejected refresh token marks the profile for a new sign-in.
    pub async fn refresh(&self, key: &str, force: bool) -> AppResult<StoredProfile> {
        let profile = self.store.get(key).ok_or_else(|| no_profile(key))?;
        if let Some(done) = self.settled(&profile, force)? {
            return Ok(done);
        }
        let seen_token = profile.access_token.clone();
        let _guard = self.refresh.lock().await;
        let profile = self.store.get(key).ok_or_else(|| no_profile(key))?;
        // Another task refreshed this profile while this one waited: use its result.
        if profile.access_token != seen_token && profile.is_fresh(now_secs()) {
            return Ok(profile);
        }
        if let Some(done) = self.settled(&profile, force)? {
            return Ok(done);
        }
        let refresh_token = profile.refresh_token.clone().unwrap_or_default();
        match self.renew(&refresh_token).await {
            Ok(update) => {
                let saved = self.store.update_tokens(key, &update)?;
                self.changed();
                Ok(saved)
            }
            Err(error) if is_revoked(&error) => {
                tracing::warn!("The Microsoft refresh token of {key} was rejected; a new sign-in is needed");
                self.store.mark_reauth(key, REASON_REFRESH_INVALID)?;
                self.changed();
                self.store.get(key).ok_or_else(|| no_profile(key))
            }
            Err(error) => {
                tracing::warn!(
                    "Token refresh for {key} failed; keeping the session: {}",
                    error.into_app_error().detail
                );
                Ok(profile)
            }
        }
    }

    /// Startup: refresh every Microsoft profile; one failure never stops the others.
    pub async fn refresh_all(&self) {
        for profile in self.store.list() {
            if profile.is_offline() {
                continue;
            }
            if let Err(e) = self.refresh(&profile.key, false).await {
                tracing::warn!("Refreshing {} failed: {}", profile.key, e.detail);
            }
        }
    }

    /// The head of `key`'s player (by UUID, like the original launcher; by name if there is none).
    pub async fn avatar(&self, key: &str) -> Option<Avatar> {
        let profile = self.store.get(key)?;
        let identifier = if profile.id.trim().is_empty() { profile.name } else { profile.id };
        self.avatars.get(&identifier).await
    }

    /// The account a launch uses: `key`, or the default profile.
    pub async fn launch_identity(&self, key: Option<&str>) -> AppResult<LaunchIdentity> {
        let key = match key {
            Some(key) => key.to_string(),
            None => self.store.default_profile().map(|p| p.key).ok_or_else(|| no_profile(""))?,
        };
        let profile = self.refresh(&key, false).await?;
        if let Some(reason) = profile.reauth_reason() {
            return Err(AppError::new(ErrorCode::ReauthRequired, reason).with_param("profile", &profile.name));
        }
        let access_token = profile.access_token.clone().ok_or_else(|| {
            AppError::new(ErrorCode::ReauthRequired, "no access token").with_param("profile", &profile.name)
        })?;
        Ok(LaunchIdentity {
            username: profile.name,
            uuid: profile.id,
            access_token,
            xuid: profile.xuid,
            client_id: profile.auth_client_id,
            kind: profile.kind,
        })
    }
}
