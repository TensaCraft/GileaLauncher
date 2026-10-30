//! Accounts service: the profile list, Microsoft sign-in (browser, then a device code), sign-in on
//! other auth servers (authlib-injector's API), token refresh and the identity a launch uses.

use std::sync::{Arc, Mutex, MutexGuard};

use launcher_shared::{
    AccountKind, AppError, AppResult, AuthState, ErrorCode, Level, ProfileDto, ProfilesSnapshot, Text,
};
use tokio_util::sync::CancellationToken;

use super::http::{AuthHttp, FlowError};
use super::msa::{self, MsTokens};
use super::offline::validate_offline_name;
use super::page::PageLook;
use super::provider::AccountProvider;
use super::servers::{AuthServers, Origin};
use super::store::{
    NewProfile, OFFLINE_TOKEN, ProfileStore, REASON_REFRESH_INVALID, ServerRef, StoredProfile, TokenUpdate,
};
use super::{AuthConfig, LAUNCH_SESSION_MARGIN, TOKEN_REFRESH_LEEWAY, UrlOpener, now_secs, xbox, yggdrasil};
use crate::feedback::{EventSink, FeedbackService};
use crate::skins::Player;

/// Who plays: what the launch passes to Minecraft. Stays inside `launcher-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIdentity {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    pub kind: AccountKind,
    /// An account on another auth server: the game reaches it through the authlib-injector agent.
    pub agent: Option<AgentServer>,
}

/// The auth server the authlib-injector agent points the game at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentServer {
    pub api_root: String,
    /// Its metadata, handed to the agent so the game starts without asking the server.
    pub metadata: String,
}

pub struct AuthService {
    cfg: AuthConfig,
    store: ProfileStore,
    servers: AuthServers,
    /// The modules' site sign-ins, by id.
    providers: Mutex<Vec<Arc<dyn AccountProvider>>>,
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
        let servers = AuthServers::open(&cfg.state_dir, cfg.servers.clone());
        Ok(Arc::new(AuthService {
            cfg,
            store,
            servers,
            providers: Mutex::new(Vec::new()),
            http,
            feedback,
            sink,
            opener,
            sign_in: tokio::sync::Mutex::new(()),
            refresh: tokio::sync::Mutex::new(()),
            cancel: Mutex::new(None),
            state: Mutex::new(AuthState::Idle),
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

    /// `delete`, ending an account's session on its auth server first (best effort: a server out
    /// of reach does not keep the account).
    pub async fn delete_signed_out(&self, key: &str) -> AppResult<ProfilesSnapshot> {
        if let Some(profile) = self.store.get(key).filter(|p| p.kind == AccountKind::Module)
            && let (Some(provider), Some(token)) =
                (profile.server.as_deref().and_then(|id| self.provider(id)), &profile.access_token)
            && let Err(e) = provider.sign_out(token).await
        {
            tracing::debug!("The site did not take the sign-out of {key}: {}", e.detail);
        }
        if let Some(profile) = self.store.get(key).filter(|p| p.kind == AccountKind::Yggdrasil)
            && let (Some(server), Some(access), Some(client)) = (
                profile.server.as_deref().and_then(|id| self.servers.get(id)),
                &profile.access_token,
                &profile.refresh_token,
            )
        {
            yggdrasil::invalidate(&self.http, &server.api_root, access, client).await;
        }
        self.delete(key)
    }

    /// A module's site sign-in: its accounts can be added and launched from now on.
    pub fn register_provider(&self, provider: Arc<dyn AccountProvider>) {
        let mut all = lock(&self.providers);
        all.retain(|p| p.id() != provider.id());
        all.push(provider);
    }

    fn provider(&self, id: &str) -> Option<Arc<dyn AccountProvider>> {
        lock(&self.providers).iter().find(|p| p.id() == id).cloned()
    }

    /// A module's auth server (`id`, at `address`), found now: the module may move it to another
    /// address later, and its accounts stay.
    pub async fn register_server(&self, id: &str, address: &str, module: &str) -> AppResult<()> {
        self.servers.register(&self.http, id, address, Origin::Module(module.to_string())).await.map(|_| ())
    }

    /// Signs in on an auth server other than Microsoft's: `server` is a known one's id (Ely.by's
    /// `elyby`, or one a module registered); the player never names an address. `AuthCodeRequired`
    /// asks for the second factor; nothing is kept until the sign-in succeeds. The account becomes
    /// the default; signing in again updates it.
    pub async fn sign_in_server(
        &self,
        server: &str,
        login: &str,
        password: &str,
        code: Option<&str>,
    ) -> AppResult<ProfileDto> {
        if let Some(provider) = self.provider(server) {
            return self.sign_in_provider(provider, login, password, code).await;
        }
        let found = self.servers.ensure(&self.http, server).await?;
        let client_token = uuid::Uuid::new_v4().simple().to_string();
        let credentials = yggdrasil::Credentials { login, password, code, client_token: &client_token };
        let session = yggdrasil::authenticate(&self.http, &found.api_root, &credentials).await?;
        let profile =
            session.profile.clone().or_else(|| session.profiles.first().cloned()).ok_or_else(|| {
                AppError::new(ErrorCode::AuthFailed, "the account has no game profile on this server")
            })?;
        let key = format!("{}:{}", found.id, profile.name);
        let saved = self.store.save(
            &key,
            NewProfile {
                id: Some(profile.id),
                name: profile.name,
                access_token: Some(session.access_token),
                refresh_token: Some(session.client_token).filter(|t| !t.is_empty()).or(Some(client_token)),
                server: Some(ServerRef {
                    id: found.id.clone(),
                    name: found.name.clone(),
                    kind: AccountKind::Yggdrasil,
                }),
                ..NewProfile::default()
            },
            true,
        )?;
        tracing::info!("Profile {key} signed in on {}", found.api_root);
        self.changed();
        Ok(saved.to_dto())
    }

    /// Signs in on a module's site; the account becomes the default, signing in again updates it.
    async fn sign_in_provider(
        &self,
        provider: Arc<dyn AccountProvider>,
        login: &str,
        password: &str,
        code: Option<&str>,
    ) -> AppResult<ProfileDto> {
        let account = provider.sign_in(login, password, code).await?;
        let key = format!("{}:{}", provider.id(), account.name);
        let saved = self.store.save(
            &key,
            NewProfile {
                id: Some(account.id),
                name: account.name,
                access_token: Some(account.token.clone()),
                refresh_token: Some(account.token),
                server: Some(ServerRef {
                    id: provider.id().to_string(),
                    name: provider.name(),
                    kind: AccountKind::Module,
                }),
                ..NewProfile::default()
            },
            true,
        )?;
        tracing::info!("Profile {key} signed in on the site of module {}", provider.id());
        self.changed();
        Ok(saved.to_dto())
    }

    /// A site account as a launch needs it: the site vouches for it now; one it ended asks for a
    /// new sign-in; a site out of reach keeps the account (single player needs none).
    async fn site_identity(&self, profile: StoredProfile) -> AppResult<LaunchIdentity> {
        let identity = |profile: &StoredProfile| LaunchIdentity {
            username: profile.name.clone(),
            uuid: profile.id.clone(),
            access_token: OFFLINE_TOKEN.to_string(),
            xuid: None,
            client_id: None,
            kind: AccountKind::Module,
            agent: None,
        };
        let module = profile.server.clone().unwrap_or_default();
        let (Some(provider), Some(token)) = (self.provider(&module), profile.access_token.clone()) else {
            return Err(AppError::new(ErrorCode::ReauthRequired, format!("no sign-in of module {module}"))
                .with_param("profile", &profile.name));
        };
        match provider.verify(&token).await {
            Ok(_) => Ok(identity(&profile)),
            Err(e) if e.code == ErrorCode::ReauthRequired => {
                tracing::warn!("The site of {module} ended the session of {}", profile.key);
                self.store.mark_reauth(&profile.key, REASON_REFRESH_INVALID)?;
                self.changed();
                Err(e.with_param("profile", &profile.name))
            }
            Err(e) => {
                tracing::warn!("The site of {module} could not vouch for {}: {}", profile.key, e.detail);
                Ok(identity(&profile))
            }
        }
    }

    /// A `Yggdrasil` account's session as a launch needs it: checked, renewed when it went stale,
    /// marked for a new sign-in when it ended. A server out of reach keeps the session (single
    /// player needs none).
    async fn server_session(&self, profile: StoredProfile) -> AppResult<(StoredProfile, AgentServer)> {
        let server_id = profile.server.clone().unwrap_or_default();
        let server = self.servers.refreshed(&self.http, &server_id).await?;
        let agent = AgentServer { api_root: server.api_root.clone(), metadata: server.metadata.clone() };
        // One renewal at a time: the server takes a token back once, so a launch that waited plays
        // on the session the other one renewed.
        let seen_token = profile.access_token.clone();
        let _guard = self.refresh.lock().await;
        let profile = self.store.get(&profile.key).ok_or_else(|| no_profile(&profile.key))?;
        if profile.access_token != seen_token {
            return Ok((profile, agent));
        }
        let (Some(access), Some(client)) = (profile.access_token.clone(), profile.refresh_token.clone())
        else {
            return Ok((profile, agent));
        };
        match yggdrasil::validate(&self.http, &server.api_root, &access, &client).await {
            Ok(true) => return Ok((profile, agent)),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("The session of {} could not be checked: {}", profile.key, e.detail);
                return Ok((profile, agent));
            }
        }
        match yggdrasil::refresh(&self.http, &server.api_root, &access, &client).await {
            Ok(renewed) => {
                let saved = self.store.update_session(&profile.key, &renewed.access_token)?;
                self.changed();
                Ok((saved, agent))
            }
            Err(e) if e.code == ErrorCode::ReauthRequired => {
                tracing::warn!("The session of {} ended on its server; a new sign-in is needed", profile.key);
                self.store.mark_reauth(&profile.key, REASON_REFRESH_INVALID)?;
                self.changed();
                Err(e.with_param("profile", &profile.name))
            }
            Err(e) => {
                tracing::warn!("The session of {} could not be renewed: {}", profile.key, e.detail);
                Ok((profile, agent))
            }
        }
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
    pub async fn sign_in_microsoft(&self, look: PageLook<'_>) -> AppResult<ProfileDto> {
        let Ok(_guard) = self.sign_in.try_lock() else {
            return Err(AppError::new(ErrorCode::Busy, "a Microsoft sign-in is already running"));
        };
        let cancel = CancellationToken::new();
        *lock(&self.cancel) = Some(cancel.clone());
        let result = self.run_sign_in(look, &cancel).await;
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

    async fn run_sign_in(&self, look: PageLook<'_>, cancel: &CancellationToken) -> AppResult<StoredProfile> {
        let ep = &self.cfg.endpoints;
        let client_id = self.cfg.client_id.as_str();
        self.feedback.info(Text::key("microsoft_auth_browser_opening"));
        self.set_state(AuthState::Browser);
        let browser = msa::browser_sign_in(
            &self.http,
            ep,
            client_id,
            self.opener.as_ref(),
            look,
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
            server: None,
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

    /// `Some` when no network refresh is needed (or none can help): the session lasts `margin`
    /// seconds more.
    fn settled(&self, profile: &StoredProfile, force: bool, margin: i64) -> AppResult<Option<StoredProfile>> {
        if profile.is_offline() {
            return Ok(Some(profile.clone()));
        }
        if profile.key_unavailable() {
            return Err(AppError::new(
                ErrorCode::CredentialStorageUnavailable,
                "the sign-in key cannot be read for now",
            )
            .with_param("profile", &profile.name));
        }
        if let Some(reason) = profile.reauth_reason() {
            if !profile.is_marked() {
                self.store.mark_reauth(&profile.key, &reason)?;
                self.changed();
            }
            return Ok(Some(self.store.get(&profile.key).unwrap_or_else(|| profile.clone())));
        }
        if !force && profile.is_fresh_for(now_secs(), margin) {
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
        self.refresh_within(key, force, TOKEN_REFRESH_LEEWAY).await
    }

    /// `refresh`, renewing a session that ends within `margin` seconds.
    async fn refresh_within(&self, key: &str, force: bool, margin: i64) -> AppResult<StoredProfile> {
        self.renewed(key, force, margin).await.map(|(profile, _)| profile)
    }

    /// `refresh_within`, with why the session could not be renewed when it was kept (a network or
    /// Xbox failure: no new sign-in helps).
    async fn renewed(
        &self,
        key: &str,
        force: bool,
        margin: i64,
    ) -> AppResult<(StoredProfile, Option<AppError>)> {
        let profile = self.store.get(key).ok_or_else(|| no_profile(key))?;
        // Another server's session is that server's to renew (at launch): never Microsoft's.
        if matches!(profile.kind, AccountKind::Yggdrasil | AccountKind::Module) {
            return Ok((profile, None));
        }
        if let Some(done) = self.settled(&profile, force, margin)? {
            return Ok((done, None));
        }
        let seen_token = profile.access_token.clone();
        let _guard = self.refresh.lock().await;
        let profile = self.store.get(key).ok_or_else(|| no_profile(key))?;
        // Another task refreshed this profile while this one waited: use its result.
        if profile.access_token != seen_token && profile.is_fresh_for(now_secs(), margin) {
            return Ok((profile, None));
        }
        if let Some(done) = self.settled(&profile, force, margin)? {
            return Ok((done, None));
        }
        let refresh_token = profile.refresh_token.clone().unwrap_or_default();
        match self.renew(&refresh_token).await {
            Ok(update) => {
                let saved = self.store.update_tokens(key, &update)?;
                self.changed();
                Ok((saved, None))
            }
            Err(error) if is_revoked(&error) => {
                tracing::warn!("The Microsoft refresh token of {key} was rejected; a new sign-in is needed");
                self.store.mark_reauth(key, REASON_REFRESH_INVALID)?;
                self.changed();
                Ok((self.store.get(key).ok_or_else(|| no_profile(key))?, None))
            }
            Err(error) => {
                let failure = error.into_app_error();
                tracing::warn!("Token refresh for {key} failed; keeping the session: {}", failure.detail);
                Ok((profile, Some(failure)))
            }
        }
    }

    /// Startup: refresh every Microsoft profile; one failure never stops the others.
    pub async fn refresh_all(&self) {
        for profile in self.store.list() {
            if profile.is_offline() || matches!(profile.kind, AccountKind::Yggdrasil | AccountKind::Module) {
                continue;
            }
            if let Err(e) = self.refresh(&profile.key, false).await {
                tracing::warn!("Refreshing {} failed: {}", profile.key, e.detail);
            }
        }
    }

    /// Whose skin profile `key` wears: its UUID and name (`skins::SkinService` finds the skin);
    /// an account on another auth server wears that server's.
    pub fn player(&self, key: &str) -> Option<Player> {
        let profile = self.store.get(key)?;
        if profile.kind == AccountKind::Module {
            let url = self.provider(profile.server.as_deref()?)?.skin_url(&profile.name);
            return Some(Player::with_skin(&profile.id, &profile.name, url));
        }
        if profile.kind == AccountKind::Yggdrasil {
            let server = self.servers.get(profile.server.as_deref()?)?;
            return Some(Player::on_server(
                &profile.id,
                &profile.name,
                &format!("{}sessionserver/", server.api_root),
            ));
        }
        Some(Player::new(&profile.id, &profile.name))
    }

    /// The account a launch uses: `key`, or the default profile.
    pub async fn launch_identity(&self, key: Option<&str>) -> AppResult<LaunchIdentity> {
        let key = match key {
            Some(key) => key.to_string(),
            None => self.store.default_profile().map(|p| p.key).ok_or_else(|| no_profile(""))?,
        };
        let stored = self.store.get(&key).ok_or_else(|| no_profile(&key))?;
        if stored.kind == AccountKind::Module {
            if let Some(reason) = stored.reauth_reason() {
                return Err(
                    AppError::new(ErrorCode::ReauthRequired, reason).with_param("profile", &stored.name)
                );
            }
            return self.site_identity(stored).await;
        }
        if stored.kind == AccountKind::Yggdrasil {
            if let Some(reason) = stored.reauth_reason() {
                return Err(
                    AppError::new(ErrorCode::ReauthRequired, reason).with_param("profile", &stored.name)
                );
            }
            let (profile, agent) = self.server_session(stored).await?;
            let access_token = profile.access_token.clone().ok_or_else(|| {
                AppError::new(ErrorCode::ReauthRequired, "no access token")
                    .with_param("profile", &profile.name)
            })?;
            return Ok(LaunchIdentity {
                username: profile.name,
                uuid: profile.id,
                access_token,
                xuid: None,
                client_id: None,
                kind: profile.kind,
                agent: Some(agent),
            });
        }
        let (profile, failure) = self.renewed(&key, false, LAUNCH_SESSION_MARGIN).await?;
        if let Some(reason) = profile.reauth_reason() {
            return Err(AppError::new(ErrorCode::ReauthRequired, reason).with_param("profile", &profile.name));
        }
        let Some(access_token) = profile.access_token.clone() else {
            // No session at all: why it could not be had (the network), not a sign-in to redo.
            let error =
                failure.unwrap_or_else(|| AppError::new(ErrorCode::ReauthRequired, "no access token"));
            return Err(error.with_param("profile", &profile.name));
        };
        if failure.is_some() && !profile.is_fresh(now_secs()) {
            // Single player needs no session; servers will refuse an ended one.
            let said = Text::key("auth_session_not_renewed").param("profile", &profile.name);
            self.feedback.toast(Level::Warning, said, None, None);
        }
        Ok(LaunchIdentity {
            username: profile.name,
            uuid: profile.id,
            access_token,
            xuid: profile.xuid,
            client_id: profile.auth_client_id,
            kind: profile.kind,
            agent: None,
        })
    }
}
