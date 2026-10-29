//! What the module's commands do: the library, an account's skin and capes, and the changes.
//! Skin files, their pictures and players' skins by nickname are the core's
//! (`launcher_core::skins`); the account's own skins and capes are Minecraft Services'.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use launcher_core::auth::store::StoredProfile;
use launcher_core::feedback::FeedbackService;
use launcher_core::net::downloader::{DownloadTask, Downloader, Origin};
use launcher_core::skins::service::{NO_SKIN, REASON, secure};
use launcher_core::skins::{Player, SkinService, SkinTexture, render};
use launcher_core::storage::json::write_json_file;
use launcher_shared::{AccountKind, AppError, AppResult, ErrorCode, ProfileDto};
use percent_encoding::percent_decode_str;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{ParseError, Url};

use super::library::{Entry, Library, NAME_MAX};
use super::mojang::{CallError, Endpoints, MojangClient, ServicesProfile, check_nickname};
use crate::dto::{
    Account, AccountState, CHANGED_EVENT, Cape, CurrentSkin, ImportArgs, LibrarySkin, MESSAGE, SkinSource,
    SkinVariant,
};

/// A profile's session: its access token, its player name and its UUID.
pub type Session = (String, String, String);
pub type TokenFuture = Pin<Box<dyn Future<Output = AppResult<Session>> + Send>>;
/// `(profile key, force)` → the profile's session, refreshed first when `force`.
pub type TokenFn = Arc<dyn Fn(&str, bool) -> TokenFuture + Send + Sync>;

/// What the service needs of the launcher; the tests put fakes in.
pub struct Deps {
    pub endpoints: Endpoints,
    pub tokens: TokenFn,
    /// The launcher's profiles, default first.
    pub profiles: Arc<dyn Fn() -> Vec<ProfileDto> + Send + Sync>,
    /// Players' skins: skin files, players by nickname and the heads the launcher draws.
    pub skins: Arc<SkinService>,
    pub downloader: Arc<Downloader>,
    /// Where cape files may come from.
    pub textures: Origin,
    pub feedback: Arc<FeedbackService>,
    /// `<app state>`: the library lives in `skins/`.
    pub state_dir: PathBuf,
    /// The launcher's cache: capes go to `skins/capes/`.
    pub cache_dir: PathBuf,
}

/// The session of `profile` for a change of its skin: its access token, unless it needs a new
/// sign-in (`skins_error_reauth`).
pub fn session_of(profile: StoredProfile) -> AppResult<Session> {
    if profile.is_offline() {
        return Err(AppError::new(ErrorCode::Unsupported, "an offline profile has no skin to change")
            .with_param("profile", &profile.name));
    }
    match (profile.reauth_reason(), profile.access_token) {
        (None, Some(token)) => Ok((token, profile.name, profile.id)),
        (reason, _) => Err(AppError::new(
            ErrorCode::ReauthRequired,
            reason.unwrap_or_else(|| "no access token".to_string()),
        )
        .with_param("profile", &profile.name)
        .with_param(MESSAGE, "skins_error_reauth")),
    }
}

/// A name for a file made of `raw`: itself when it is short letters, digits and dashes, else a
/// digest of it.
fn file_key(raw: &str) -> String {
    let fits = !raw.is_empty()
        && raw.len() <= 64
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if fits { raw.to_string() } else { hex::encode(&Sha256::digest(raw.as_bytes())[..16]) }
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn bad_png(e: AppError) -> AppError {
    e.with_param(MESSAGE, "skins_error_bad_png")
}

/// `bytes` as a skin of the library (`render::normalise`), its model guessed; else
/// `skins_error_bad_png`.
fn read_skin(bytes: &[u8]) -> AppResult<SkinTexture> {
    SkinTexture::read(bytes).map_err(|e| if e.code == ErrorCode::InvalidInput { bad_png(e) } else { e })
}

fn variant_of(slim: bool) -> SkinVariant {
    if slim { SkinVariant::Slim } else { SkinVariant::Classic }
}

/// What the player reads when player `name`'s skin could not be had.
fn lookup_error(e: AppError, name: &str) -> AppError {
    let message = match (e.code, e.params.get(REASON).map(String::as_str)) {
        (ErrorCode::NotFound, Some(NO_SKIN)) => "skins_error_no_skin",
        (ErrorCode::NotFound, _) => "skins_error_no_player",
        (ErrorCode::RateLimited, _) => "skins_error_rate_lookup",
        (ErrorCode::InvalidInput, Some(render::BAD_PNG)) => "skins_error_bad_png",
        _ => return e,
    };
    e.with_param(MESSAGE, message).with_param("name", name)
}

/// The skin in file `path`, named after the file.
fn file_skin(path: &Path) -> AppResult<(SkinTexture, String, String)> {
    let size = fs::metadata(path).map_err(|e| io_error(path, e))?.len();
    if size > render::MAX_BYTES as u64 {
        return Err(bad_png(AppError::new(ErrorCode::InvalidInput, format!("the file is {size} bytes"))));
    }
    let bytes = fs::read(path).map_err(|e| io_error(path, e))?;
    Ok((read_skin(&bytes)?, name_of(path), "file".to_string()))
}

/// The web address the player typed, over https (the launcher downloads nothing else from the
/// web): assumed when they left it out, put in place of `http` for any host but this computer's;
/// else `skins_error_bad_link`.
fn check_link(raw: &str) -> AppResult<Url> {
    let raw = raw.trim();
    let parsed = match Url::parse(raw) {
        Err(ParseError::RelativeUrlWithoutBase) => Url::parse(&format!("https://{raw}")),
        Ok(url) if url.scheme() == "http" && !is_this_computer(&url) => {
            Url::parse(&format!("https{}", &url.as_str()["http".len()..]))
        }
        other => other,
    };
    match parsed {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => Ok(url),
        _ => Err(AppError::new(ErrorCode::InvalidInput, format!("{raw:?} is no web address"))
            .with_param(MESSAGE, "skins_error_bad_link")),
    }
}

fn is_this_computer(url: &Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

/// The default name of a skin added from `url`: its file's name, else the site's.
fn link_name(url: &Url) -> String {
    let last = url.path_segments().and_then(|mut s| s.rfind(|s| !s.is_empty())).unwrap_or_default();
    let file = percent_decode_str(last).decode_utf8_lossy();
    let file = Path::new(file.as_ref());
    if file.file_stem().is_some_and(|stem| !stem.to_string_lossy().trim().is_empty()) {
        return name_of(file);
    }
    url.host_str().map_or_else(|| "skin".to_string(), |host| host.chars().take(NAME_MAX).collect())
}

/// The default name of a skin added from `path`: its file name without the extension.
fn name_of(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default();
    let stem: String = stem.chars().take(NAME_MAX).collect();
    match stem.trim() {
        "" => "skin".to_string(),
        name => name.to_string(),
    }
}

pub struct SkinsService {
    deps: Deps,
    client: MojangClient,
    library: Library,
    /// Thumbnails by `<sha256>:<variant>`.
    thumbs: Mutex<HashMap<String, String>>,
    /// The skin each account was last seen wearing, by profile UUID (`skins/accounts.json`):
    /// a skin is added to the library once, when an account is first seen in it.
    seen: Mutex<()>,
}

impl SkinsService {
    pub fn new(deps: Deps) -> AppResult<SkinsService> {
        let client = MojangClient::new(deps.endpoints.clone())?;
        let library = Library::open(deps.state_dir.join("skins"));
        Ok(SkinsService { deps, client, library, thumbs: Mutex::new(HashMap::new()), seen: Mutex::new(()) })
    }

    fn changed(&self) {
        self.deps.feedback.module_event(CHANGED_EVENT, &Value::Null);
    }

    fn cache(&self) -> PathBuf {
        self.deps.cache_dir.join("skins")
    }

    /// The official profiles, the default first.
    pub fn accounts(&self) -> Vec<Account> {
        let mut accounts: Vec<Account> = (self.deps.profiles)()
            .into_iter()
            .filter(|p| p.kind == AccountKind::Microsoft)
            .map(|p| Account {
                key: p.key,
                name: p.name,
                is_default: p.is_default,
                reauth_required: p.reauth_required,
            })
            .collect();
        accounts.sort_by_key(|a| !a.is_default);
        accounts
    }

    /// `call` with `key`'s token; a refused token is refreshed once and `call` made again.
    async fn with_token<T, F, Fut>(&self, key: &str, call: F) -> AppResult<(T, Session)>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, CallError>>,
    {
        let session = (self.deps.tokens)(key, false).await?;
        match call(session.0.clone()).await {
            Err(CallError::Unauthorized) => {
                let session = (self.deps.tokens)(key, true).await?;
                let value = call(session.0.clone()).await.map_err(AppError::from)?;
                Ok((value, session))
            }
            other => Ok((other.map_err(AppError::from)?, session)),
        }
    }

    /// Whether the account `profile_id` was last seen in another skin than `sha256`; records it.
    fn first_sight(&self, profile_id: &str, sha256: &str) -> bool {
        let _held = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.deps.state_dir.join("skins").join("accounts.json");
        let mut seen: BTreeMap<String, String> =
            fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if seen.get(profile_id).map(String::as_str) == Some(sha256) {
            return false;
        }
        seen.insert(profile_id.to_string(), sha256.to_string());
        let value = serde_json::to_value(&seen).unwrap_or(Value::Null);
        if let Err(e) = fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|()| write_json_file(&path, &value, 2))
        {
            tracing::warn!("Unable to note the skin an account wears: {e}");
        }
        true
    }

    /// What `profile` says, with the textures fetched. The worn skin is the player's head from
    /// now on, and goes to the library the first time the account is seen in it — unless it is
    /// one of Minecraft's own (named by an alias), which is only shown.
    async fn state_of(
        &self,
        key: &str,
        session: &Session,
        profile: ServicesProfile,
    ) -> AppResult<AccountState> {
        let (_, name, id) = session;
        let skin = match profile.skins.iter().find(|s| s.state == "ACTIVE") {
            None => None,
            Some(remote) => {
                let slim = remote.variant == SkinVariant::Slim;
                let texture = self.deps.skins.wear(&Player::new(id, name), &remote.url, slim).await?;
                let default = remote.alias.is_some();
                let mut entry = self.library.find_sha(&texture.hash);
                if !default && self.first_sight(id, &texture.hash) && entry.is_none() {
                    let name: String = name.chars().take(NAME_MAX).collect();
                    let added = self.library.add(&texture, &name, remote.variant, &format!("account:{name}"));
                    match added {
                        Ok((added, _)) => entry = Some(added),
                        Err(e) => tracing::warn!("Unable to keep {name}'s skin in the library: {}", e.detail),
                    }
                }
                Some(CurrentSkin {
                    sha256: texture.hash.clone(),
                    variant: remote.variant,
                    data: render::data_url(&texture.png),
                    library_id: entry.map(|e| e.id),
                    default,
                })
            }
        };
        let dir = self.cache().join("capes");
        let tasks: Vec<DownloadTask> = profile
            .capes
            .iter()
            .map(|c| {
                DownloadTask::new(secure(&c.url), dir.join(format!("{}.png", file_key(&c.id))))
                    .origin(self.deps.textures.clone())
            })
            .collect();
        if !tasks.is_empty() {
            let report = self.deps.downloader.download_all(tasks, false, &|_| {}).await?;
            for failed in &report.failed {
                tracing::warn!("A cape of {name} could not be fetched: {failed}");
            }
        }
        let capes = profile
            .capes
            .into_iter()
            .map(|c| {
                let data = fs::read(dir.join(format!("{}.png", file_key(&c.id))))
                    .map(|bytes| render::data_url(&bytes))
                    .unwrap_or_default();
                Cape { active: c.state == "ACTIVE", id: c.id, alias: c.alias, data }
            })
            .collect();
        Ok(AccountState { profile: key.to_string(), skin, capes })
    }

    /// What account `key` wears and has.
    pub async fn state(&self, key: &str) -> AppResult<AccountState> {
        let client = &self.client;
        let (profile, session) =
            self.with_token(key, |token| async move { client.profile(&token).await }).await?;
        self.state_of(key, &session, profile).await
    }

    /// After a change of account `key`: its skin is looked up anew (the head with it), the
    /// change announced and the new state read.
    async fn after_change(
        &self,
        key: &str,
        session: &Session,
        profile: ServicesProfile,
    ) -> AppResult<AccountState> {
        let (_, name, id) = session;
        self.deps.skins.forget(&Player::new(id, name));
        let state = self.state_of(key, session, profile).await;
        self.changed();
        state
    }

    fn thumb(&self, entry: &Entry) -> String {
        let cache_key = format!("{}:{}", entry.sha256, entry.variant.as_str());
        let mut thumbs = self.thumbs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(thumb) = thumbs.get(&cache_key) {
            return thumb.clone();
        }
        let thumb = match self.library.png(&entry.id) {
            Ok(skin) => render::data_url(&render::front(&skin, entry.variant == SkinVariant::Slim)),
            Err(_) => return String::new(),
        };
        thumbs.insert(cache_key, thumb.clone());
        thumb
    }

    fn shown(&self, entry: Entry) -> LibrarySkin {
        let thumb = self.thumb(&entry);
        LibrarySkin {
            id: entry.id,
            name: entry.name,
            variant: entry.variant,
            source: entry.source,
            sha256: entry.sha256,
            added_ms: entry.added_ms,
            thumb,
        }
    }

    /// The library, the newest first.
    pub fn library(&self) -> Vec<LibrarySkin> {
        self.library.list().into_iter().map(|e| self.shown(e)).collect()
    }

    /// Skin `id` for the 3D viewer.
    pub fn skin(&self, id: &str) -> AppResult<String> {
        Ok(render::data_url(&self.library.png(id)?))
    }

    /// Adds a skin from `args.source`, named as asked (else after its file, player or link), on
    /// the model asked (else the one its arms suggest).
    pub async fn import(&self, args: &ImportArgs) -> AppResult<LibrarySkin> {
        let (texture, name, source) = match &args.source {
            SkinSource::File { path } => file_skin(Path::new(path))?,
            SkinSource::Player { name } => self.player_skin(name).await?,
            SkinSource::Link { url } => self.linked_skin(url).await?,
        };
        let name = args.name.as_deref().filter(|n| !n.trim().is_empty()).map_or(name, str::to_string);
        let variant = args.variant.unwrap_or(variant_of(texture.slim));
        let (entry, _) = self.library.add(&texture, &name, variant, &source)?;
        self.changed();
        Ok(self.shown(entry))
    }

    /// Player `name`'s skin as Mojang has it now.
    async fn player_skin(&self, name: &str) -> AppResult<(SkinTexture, String, String)> {
        let name = check_nickname(name)?;
        let texture =
            self.deps.skins.official(&Player::named(&name)).await.map_err(|e| lookup_error(e, &name))?;
        let source = format!("player:{name}");
        Ok((texture, name, source))
    }

    /// The skin at the address `raw`.
    async fn linked_skin(&self, raw: &str) -> AppResult<(SkinTexture, String, String)> {
        let url = check_link(raw)?;
        let texture =
            self.deps.skins.at_link(url.as_str()).await.map_err(|e| match e.params.get(REASON) {
                Some(reason) if reason == render::BAD_PNG => bad_png(e),
                _ => {
                    tracing::warn!("No skin could be had at {url}: {}", e.detail);
                    e.with_param(MESSAGE, "skins_error_link")
                }
            })?;
        Ok((texture, link_name(&url), format!("link:{url}")))
    }

    pub fn rename(&self, id: &str, name: &str) -> AppResult<LibrarySkin> {
        let entry = self.library.rename(id, name)?;
        self.changed();
        Ok(self.shown(entry))
    }

    pub fn set_variant(&self, id: &str, variant: SkinVariant) -> AppResult<LibrarySkin> {
        let entry = self.library.set_variant(id, variant)?;
        self.changed();
        Ok(self.shown(entry))
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        self.library.delete(id)?;
        self.changed();
        Ok(())
    }

    /// Puts library skin `id` on account `key`, with the skin's model.
    pub async fn apply(&self, key: &str, id: &str) -> AppResult<AccountState> {
        let variant = self.library.get(id)?.variant;
        let skin = self.library.png(id)?;
        let client = &self.client;
        let (profile, session) = self
            .with_token(key, |token| {
                let skin = skin.clone();
                async move { client.upload_skin(&token, skin, variant).await }
            })
            .await?;
        self.after_change(key, &session, profile).await
    }

    /// Puts cape `cape` on account `key`; `None` takes the cape off.
    pub async fn set_cape(&self, key: &str, cape: Option<String>) -> AppResult<AccountState> {
        let client = &self.client;
        let cape = cape.as_deref();
        let (profile, session) =
            self.with_token(key, |token| async move { client.set_cape(&token, cape).await }).await?;
        self.after_change(key, &session, profile).await
    }
}

#[cfg(test)]
mod tests {
    use launcher_core::auth::store::{NewProfile, ProfileStore};
    use launcher_core::skins::service::NO_PLAYER;

    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn official(refresh: Option<&str>) -> NewProfile {
        NewProfile {
            id: Some(ID.into()),
            name: "Steve_42".into(),
            access_token: Some("token".into()),
            refresh_token: refresh.map(str::to_string),
            xuid: None,
            expires_at: Some(4_000_000_000),
            auth_client_id: None,
        }
    }

    #[test]
    fn a_profile_needing_sign_in_has_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        store.save("fresh", official(Some("refresh")), false).unwrap();
        store.save("stale", official(None), false).unwrap();
        store.save("Steve", NewProfile::offline("Steve"), false).unwrap();
        assert_eq!(
            session_of(store.get("fresh").unwrap()).unwrap(),
            ("token".into(), "Steve_42".into(), ID.into())
        );
        let stale = session_of(store.get("stale").unwrap()).unwrap_err();
        assert_eq!(stale.code, ErrorCode::ReauthRequired);
        assert_eq!(stale.params.get(MESSAGE).map(String::as_str), Some("skins_error_reauth"));
        assert_eq!(session_of(store.get("Steve").unwrap()).unwrap_err().code, ErrorCode::Unsupported);
    }

    #[test]
    fn file_keys_stay_in_their_folder() {
        assert_eq!(file_key("c1-b2"), "c1-b2");
        assert_eq!(file_key("../x").len(), 32);
        assert_eq!(file_key("").len(), 32);
        assert_eq!(name_of(Path::new("C:/skins/  Мій скін .png")), "Мій скін");
        assert_eq!(name_of(Path::new(".png")), ".png");
    }

    #[test]
    fn links_are_web_addresses() {
        let said =
            |raw: &str| check_link(raw).map(|u| u.to_string()).map_err(|e| e.params.get(MESSAGE).cloned());
        assert_eq!(said(" s.namemc.com/i/abc.png "), Ok("https://s.namemc.com/i/abc.png".into()));
        assert_eq!(
            said("http://x.ua/a.png"),
            Ok("https://x.ua/a.png".into()),
            "the launcher downloads over https"
        );
        assert_eq!(
            said("http://127.0.0.1:8080/a.png"),
            Ok("http://127.0.0.1:8080/a.png".into()),
            "this computer's own"
        );
        for bad in ["", "ftp://x.ua/a.png", "file:///C:/a.png", "two words", "data:image/png;base64,AA=="] {
            assert_eq!(said(bad), Err(Some("skins_error_bad_link".into())), "{bad:?}");
        }
        let name = |raw: &str| link_name(&Url::parse(raw).unwrap());
        assert_eq!(name("https://x.ua/skins/%D0%9B%D0%B8%D1%86%D0%B0%D1%80.png?v=2"), "Лицар");
        assert_eq!(name("https://mc-heads.net/skin/Notch/"), "Notch");
        assert_eq!(name("https://x.ua/"), "x.ua", "the site's name when the address names no file");
    }

    #[test]
    fn a_failed_lookup_reads_as_the_module_s_message() {
        let said = |e: AppError| {
            let e = lookup_error(e, "Alex");
            (e.params.get(MESSAGE).cloned(), e.params.get("name").cloned())
        };
        let nothing = |reason: &str| AppError::new(ErrorCode::NotFound, "none").with_param(REASON, reason);
        let alex = Some("Alex".to_string());
        assert_eq!(said(nothing(NO_PLAYER)), (Some("skins_error_no_player".into()), alex.clone()));
        assert_eq!(said(nothing(NO_SKIN)), (Some("skins_error_no_skin".into()), alex.clone()));
        let rate = AppError::new(ErrorCode::RateLimited, "429");
        assert_eq!(said(rate), (Some("skins_error_rate_lookup".into()), alex));
        assert_eq!(said(AppError::new(ErrorCode::Network, "down")), (None, None), "the launcher's own");
    }
}
