//! What the module's commands do: the library, an account's skin and capes, and the changes.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use launcher_core::auth::store::StoredProfile;
use launcher_core::feedback::FeedbackService;
use launcher_core::net::downloader::{DownloadTask, Downloader, Origin};
use launcher_core::storage::json::write_json_file;
use launcher_shared::{AccountKind, AppError, AppResult, ErrorCode, ProfileDto};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::library::{Entry, Library, NAME_MAX};
use super::mojang::{CallError, Endpoints, MojangClient, ServicesProfile, check_nickname};
use super::png::{self, MAX_BYTES, SkinImage};
use crate::dto::{
    Account, AccountState, CHANGED_EVENT, Cape, CurrentSkin, LibrarySkin, MESSAGE, SkinVariant,
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
    /// Forgets a profile's cached head (its skin changed).
    pub forget_avatar: Arc<dyn Fn(&str) + Send + Sync>,
    pub downloader: Arc<Downloader>,
    /// Where skin and cape files may come from.
    pub textures: Origin,
    pub feedback: Arc<FeedbackService>,
    /// `<app state>`: the library lives in `skins/`.
    pub state_dir: PathBuf,
    /// The launcher's cache: textures go to `skins/`.
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

/// `textures.minecraft.net` hands out `http` addresses; it serves the same over `https`.
pub fn secure(url: &str) -> String {
    match url.strip_prefix("http://textures.minecraft.net/") {
        Some(rest) => format!("https://textures.minecraft.net/{rest}"),
        None => url.to_string(),
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

fn bad_png(detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidInput, detail.into()).with_param(MESSAGE, "skins_error_bad_png")
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

    /// Fetches `url` to `dest` (a file already there is kept).
    async fn fetch(&self, url: &str, dest: &Path) -> AppResult<Vec<u8>> {
        let task = DownloadTask::new(secure(url), dest).origin(self.deps.textures.clone());
        self.deps.downloader.download_one(task, &|_| {}).await?;
        fs::read(dest).map_err(|e| io_error(dest, e))
    }

    /// The skin an account wears, from the cache or its host: `<cache>/skins/current/<profile>/
    /// <texture>.png`, the profile's other files dropped.
    async fn current_skin(&self, profile_id: &str, url: &str) -> AppResult<SkinImage> {
        let dir = self.cache().join("current").join(file_key(profile_id));
        let texture = file_key(url.trim_end_matches('/').rsplit('/').next().unwrap_or_default());
        let dest = dir.join(format!("{texture}.png"));
        let bytes = self.fetch(url, &dest).await?;
        for old in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if old.path() != dest {
                let _ = fs::remove_file(old.path());
            }
        }
        png::read_skin(&bytes)
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

    /// What `profile` says, with the textures fetched; the worn skin goes to the library the first
    /// time the account is seen in it.
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
                let image = self.current_skin(id, &remote.url).await?;
                let mut entry = self.library.find_sha(&image.sha256);
                if self.first_sight(id, &image.sha256) && entry.is_none() {
                    let name: String = name.chars().take(NAME_MAX).collect();
                    let added = self.library.add(&image, &name, remote.variant, &format!("account:{name}"));
                    match added {
                        Ok((added, _)) => entry = Some(added),
                        Err(e) => tracing::warn!("Unable to keep {name}'s skin in the library: {}", e.detail),
                    }
                }
                Some(CurrentSkin {
                    sha256: image.sha256.clone(),
                    variant: remote.variant,
                    data: png::data_url(&image.png),
                    library_id: entry.map(|e| e.id),
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
                    .map(|bytes| png::data_url(&bytes))
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

    fn thumb(&self, entry: &Entry) -> String {
        let cache_key = format!("{}:{}", entry.sha256, entry.variant.as_str());
        let mut thumbs = self.thumbs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(thumb) = thumbs.get(&cache_key) {
            return thumb.clone();
        }
        let thumb = match self.library.png(&entry.id) {
            Ok(skin) => png::data_url(&png::thumbnail(&skin, entry.variant)),
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
        Ok(png::data_url(&self.library.png(id)?))
    }

    /// Adds the skin in file `path`, named `name` or after the file, its model guessed.
    pub fn import_file(&self, path: &str, name: Option<&str>) -> AppResult<LibrarySkin> {
        let path = Path::new(path);
        let size = fs::metadata(path).map_err(|e| io_error(path, e))?.len();
        if size > MAX_BYTES as u64 {
            return Err(bad_png(format!("the file is {size} bytes")));
        }
        let bytes = fs::read(path).map_err(|e| io_error(path, e))?;
        let image = png::read_skin(&bytes)?;
        let name = name.map(str::to_string).unwrap_or_else(|| name_of(path));
        let (entry, _) = self.library.add(&image, &name, png::guess_variant(&image.png), "file")?;
        self.changed();
        Ok(self.shown(entry))
    }

    /// Adds player `name`'s skin.
    pub async fn import_player(&self, name: &str) -> AppResult<LibrarySkin> {
        let name = check_nickname(name)?;
        let (url, variant) = self.client.player_skin(&name).await.map_err(|e| match e {
            CallError::NotFound => AppError::from(e).with_param("name", &name),
            other => AppError::from(other),
        })?;
        let dest = self.cache().join("players").join(format!("{}.png", uuid::Uuid::new_v4().simple()));
        let task = DownloadTask::new(secure(&url), &dest).origin(self.deps.textures.clone()).transient();
        let fetched = self.deps.downloader.download_one(task, &|_| {}).await;
        let bytes = fetched.and_then(|()| fs::read(&dest).map_err(|e| io_error(&dest, e)));
        let _ = fs::remove_file(&dest);
        let image = png::read_skin(&bytes?)?;
        let (entry, _) = self.library.add(&image, &name, variant, &format!("player:{name}"))?;
        self.changed();
        Ok(self.shown(entry))
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
        (self.deps.forget_avatar)(key);
        self.changed();
        self.state_of(key, &session, profile).await
    }

    /// Puts cape `cape` on account `key`; `None` takes the cape off.
    pub async fn set_cape(&self, key: &str, cape: Option<String>) -> AppResult<AccountState> {
        let client = &self.client;
        let cape = cape.as_deref();
        let (profile, session) =
            self.with_token(key, |token| async move { client.set_cape(&token, cape).await }).await?;
        self.changed();
        self.state_of(key, &session, profile).await
    }
}

#[cfg(test)]
mod tests {
    use launcher_core::auth::store::{NewProfile, ProfileStore};

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
    fn texture_addresses_go_over_https() {
        assert_eq!(
            secure("http://textures.minecraft.net/texture/abc"),
            "https://textures.minecraft.net/texture/abc"
        );
        assert_eq!(secure("http://127.0.0.1:5/textures/a"), "http://127.0.0.1:5/textures/a");
        assert_eq!(secure("http://textures.minecraft.net.evil/x"), "http://textures.minecraft.net.evil/x");
    }

    #[test]
    fn file_keys_stay_in_their_folder() {
        assert_eq!(file_key("c1-b2"), "c1-b2");
        assert_eq!(file_key("../x").len(), 32);
        assert_eq!(file_key("").len(), 32);
        assert_eq!(name_of(Path::new("C:/skins/  Мій скін .png")), "Мій скін");
        assert_eq!(name_of(Path::new(".png")), ".png");
    }
}
