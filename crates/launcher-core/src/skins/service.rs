//! Where a player's skin comes from. By UUID Mojang's session server names the texture and its
//! model; by nickname Mojang's profile API names the UUID first. When Mojang has nothing (an
//! offline profile, a default skin) or cannot be reached, third-party hosts give the full skin;
//! when even they fail, a head comes ready-drawn from the old head hosts.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Url;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::render;
use crate::net::downloader::{DownloadTask, Downloader, Origin};
use crate::net::{api_client, send_patiently};
use crate::storage::atomic::atomic_write;
use crate::storage::json::write_json_file;

/// The size of a profile's head in the launcher, in pixels.
pub const AVATAR_SIZE: u32 = 64;
/// How long a texture Mojang named is trusted before Mojang is asked again.
pub const OFFICIAL_TTL: Duration = Duration::from_secs(3600);
/// How long a third-party texture is trusted when Mojang said it has none.
pub const FALLBACK_TTL: Duration = Duration::from_secs(24 * 3600);
/// The parameter a `NotFound` of `official` names why with.
pub const REASON: &str = "reason";
/// No player has that nickname or UUID.
pub const NO_PLAYER: &str = "no_player";
/// The player wears one of Minecraft's default skins: Mojang names no texture.
pub const NO_SKIN: &str = "no_skin";

/// Full skins by UUID or nickname (`{id}`), asked when Mojang has none.
pub const DEFAULT_SKIN_HOSTS: [&str; 2] = ["https://mc-heads.net/skin/{id}", "https://minotar.net/skin/{id}"];
/// Ready-drawn heads (`{id}`, `{size}`): the last resort, when no skin could be had at all.
pub const DEFAULT_HEAD_HOSTS: [&str; 3] = [
    "https://mc-heads.net/avatar/{id}/{size}",
    "https://minotar.net/helm/{id}/{size}.png",
    "https://mineskin.eu/avatar/{id}/{size}",
];
/// Where Mojang keeps skin files.
pub const TEXTURES_HOST: &str = "textures.minecraft.net";

/// The longest nickname Minecraft takes.
const NICKNAME_MAX: usize = 16;
/// The players' textures as last looked up, in the service's folder.
const PLAYERS_FILE: &str = "players.json";
const TEXTURES_DIR: &str = "textures";
const HEADS_DIR: &str = "heads";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Avatar {
    Png(Vec<u8>),
    /// Nothing could be downloaded or cached: the webview may still load this address itself.
    Remote(String),
}

impl Avatar {
    /// For `<img src>`: a data URL for bytes, or the remote address.
    pub fn to_src(&self) -> String {
        match self {
            Avatar::Png(bytes) => render::data_url(bytes),
            Avatar::Remote(url) => url.clone(),
        }
    }
}

/// Whose skin: a Mojang UUID (dashed or not), else a nickname. An offline profile's UUID is made
/// from its name and Mojang knows none of them: asked by it, Mojang simply finds nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Player {
    pub id: Option<String>,
    pub name: Option<String>,
}

impl Player {
    /// A player known by UUID and nickname; a blank one is none.
    pub fn new(id: &str, name: &str) -> Player {
        let given = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        Player { id: given(id), name: given(name) }
    }

    /// A player known by nickname only.
    pub fn named(name: &str) -> Player {
        Player::new("", name)
    }

    /// The UUID as Mojang's addresses take it (32 hex digits, lowercase), when it is one.
    pub fn uuid(&self) -> Option<String> {
        let raw = self.id.as_deref()?.trim().replace('-', "").to_ascii_lowercase();
        (raw.len() == 32 && raw.chars().all(|c| c.is_ascii_hexdigit())).then_some(raw)
    }

    fn nickname(&self) -> Option<&str> {
        self.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
    }

    /// What the player's texture is kept under: the UUID, else the nickname.
    fn key(&self) -> Option<String> {
        match (self.uuid(), self.nickname()) {
            (Some(uuid), _) => Some(format!("id:{uuid}")),
            (None, Some(name)) => Some(format!("name:{}", name.to_lowercase())),
            (None, None) => None,
        }
    }

    /// What third-party hosts are asked for: the UUID, else the nickname.
    fn identifier(&self) -> Option<String> {
        self.uuid().or_else(|| self.nickname().map(str::to_string))
    }
}

/// A skin as the launcher draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinTexture {
    /// A 64×64 RGBA PNG (`render::normalise`).
    pub png: Vec<u8>,
    /// Alex's model: three-pixel arms.
    pub slim: bool,
    /// Mojang named it; `false` for one a third-party host gave.
    pub official: bool,
    /// SHA-256 of `png`, lowercase hex.
    pub hash: String,
}

impl SkinTexture {
    /// `bytes` read as a skin (`render::normalise`), its model guessed.
    pub fn read(bytes: &[u8]) -> AppResult<SkinTexture> {
        let png = render::normalise(bytes)?;
        Ok(SkinTexture { slim: render::guess_slim(&png), official: false, hash: render::sha256(&png), png })
    }
}

/// Where skins are asked for; each is replaced in the tests by a fake server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinEndpoints {
    /// Mojang's session server: a profile's textures by UUID.
    pub session: String,
    /// Mojang's profile API: a UUID by nickname.
    pub mojang_api: String,
    /// The hosts Mojang's texture files may come from.
    pub texture_hosts: Vec<String>,
    /// Full skins by `{id}`, asked in order when Mojang has none.
    pub skin_hosts: Vec<String>,
    /// Heads by `{id}` and `{size}`, asked in order when no skin could be had.
    pub head_hosts: Vec<String>,
}

impl Default for SkinEndpoints {
    fn default() -> Self {
        SkinEndpoints {
            session: "https://sessionserver.mojang.com".into(),
            mojang_api: "https://api.mojang.com".into(),
            texture_hosts: vec![TEXTURES_HOST.into()],
            skin_hosts: DEFAULT_SKIN_HOSTS.iter().map(|s| s.to_string()).collect(),
            head_hosts: DEFAULT_HEAD_HOSTS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl SkinEndpoints {
    /// Everything on one local server (a fake): the third-party hosts under `/hosts/<host>/`.
    pub fn local(base: &str) -> Self {
        let host = Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
        SkinEndpoints {
            session: base.into(),
            mojang_api: base.into(),
            texture_hosts: vec![host],
            skin_hosts: vec![
                format!("{base}/hosts/mc-heads/skin/{{id}}"),
                format!("{base}/hosts/minotar/skin/{{id}}"),
            ],
            head_hosts: vec![
                format!("{base}/hosts/mc-heads/avatar/{{id}}/{{size}}"),
                format!("{base}/hosts/minotar/helm/{{id}}/{{size}}.png"),
                format!("{base}/hosts/mineskin/avatar/{{id}}/{{size}}"),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinConfig {
    pub endpoints: SkinEndpoints,
    /// The service's folder in the cache: the textures, the players' and the heads.
    pub dir: PathBuf,
    /// How long a texture Mojang named is trusted.
    pub official_ttl: Duration,
    /// How long a third-party texture or head is trusted.
    pub fallback_ttl: Duration,
    /// The longest a question to Mojang or a head host is waited for.
    pub timeout: Duration,
}

impl SkinConfig {
    /// The real hosts, the files in `<cache>/skins`.
    pub fn new(cache_dir: &Path) -> Self {
        SkinConfig {
            endpoints: SkinEndpoints::default(),
            dir: cache_dir.join("skins"),
            official_ttl: OFFICIAL_TTL,
            fallback_ttl: FALLBACK_TTL,
            timeout: Duration::from_secs(6),
        }
    }
}

/// The skin a player was last seen in (`players.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    /// Its file in `textures/`.
    file: String,
    slim: bool,
    official: bool,
    /// Unix seconds until which no source is asked again.
    until: u64,
}

/// A texture file: its name in `textures/`, the normalised PNG and its hash.
type Fetched = (String, Vec<u8>, String);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn network(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Network, format!("Mojang: {e}"))
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

/// Mojang has nothing for the player: `reason` says why.
fn nothing(reason: &str, detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::NotFound, detail.into()).with_param(REASON, reason)
}

/// `textures.minecraft.net` hands out `http` addresses; it serves the same over `https`.
pub fn secure(url: &str) -> String {
    match url.strip_prefix("http://textures.minecraft.net/") {
        Some(rest) => format!("https://{TEXTURES_HOST}/{rest}"),
        None => url.to_string(),
    }
}

/// Percent-encodes a URL path segment (RFC 3986 unreserved characters stay as they are).
pub fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `name` can be a Minecraft nickname: `[A-Za-z0-9_]{1,16}`.
pub fn is_nickname(name: &str) -> bool {
    (1..=NICKNAME_MAX).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A short name for a file made of `text`.
fn digest(text: &str) -> String {
    hex::encode(&Sha256::digest(text.as_bytes())[..16])
}

/// The skin's address and model in a session profile: its base64 `textures` property.
fn skin_of(profile: &Value) -> Option<(String, bool)> {
    let value = profile
        .get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some("textures"))?
        .get("value")?
        .as_str()?;
    let textures: Value = serde_json::from_slice(&STANDARD.decode(value.trim()).ok()?).ok()?;
    let skin = textures.get("textures")?.get("SKIN")?;
    let url = skin.get("url")?.as_str()?.to_string();
    let slim = skin.pointer("/metadata/model").and_then(Value::as_str) == Some("slim");
    Some((url, slim))
}

fn is_fresh(file: &Path, ttl: Duration) -> bool {
    fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < ttl)
}

fn read_seen(path: &Path) -> BTreeMap<String, Seen> {
    let Ok(bytes) = fs::read(path) else { return BTreeMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!("The players' skins are unreadable ({e}); they are looked up again");
        BTreeMap::new()
    })
}

pub struct SkinService {
    cfg: SkinConfig,
    client: reqwest::Client,
    downloader: Arc<Downloader>,
    /// By `Player::key`.
    seen: Mutex<BTreeMap<String, Seen>>,
    /// Texture files read so far: the normalised PNG and its hash, by file name.
    decoded: Mutex<HashMap<String, (Vec<u8>, String)>>,
    /// One lookup per player (and download per texture) at a time; later callers find its result.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl SkinService {
    pub fn new(cfg: SkinConfig, downloader: Arc<Downloader>) -> AppResult<SkinService> {
        let client = api_client(&format!("{APP_NAME}/{VERSION}"), cfg.timeout, cfg.timeout)?;
        let seen = read_seen(&cfg.dir.join(PLAYERS_FILE));
        Ok(SkinService {
            cfg,
            client,
            downloader,
            seen: Mutex::new(seen),
            decoded: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
        })
    }

    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.locks).entry(key.to_string()).or_default().clone()
    }

    fn textures_dir(&self) -> PathBuf {
        self.cfg.dir.join(TEXTURES_DIR)
    }

    fn head_file(&self, identifier: &str, size: u32) -> PathBuf {
        self.cfg.dir.join(HEADS_DIR).join(format!("{}-{size}.png", digest(identifier)))
    }

    fn is_texture_host(hosts: &[String], url: &Url) -> bool {
        url.host_str().is_some_and(|host| hosts.iter().any(|h| h == host))
    }

    /// `segments` added to `base`, each escaped.
    fn address(base: &str, segments: &[&str]) -> AppResult<Url> {
        let mut url = Url::parse(base.trim_end_matches('/')).map_err(network)?;
        url.path_segments_mut().map_err(|()| network("the address cannot have a path"))?.extend(segments);
        Ok(url)
    }

    /// A GET of Mojang's JSON: the body of a success, `None` for 204, 400 or 404 (no such player),
    /// `RateLimited` for 429, `Network` for the rest.
    async fn ask(&self, url: Url) -> AppResult<Option<Value>> {
        let response = send_patiently(self.client.get(url)).await.map_err(network)?;
        let status = response.status().as_u16();
        match status {
            204 | 400 | 404 => Ok(None),
            429 => Err(AppError::new(ErrorCode::RateLimited, "Mojang: HTTP 429")),
            200..=299 => {
                let body = response.bytes().await.map_err(network)?;
                if body.is_empty() {
                    return Ok(None);
                }
                serde_json::from_slice(&body).map(Some).map_err(network)
            }
            _ => Err(network(format!("HTTP {status}")).with_param("status", status.to_string())),
        }
    }

    /// The UUID of nickname `name`.
    async fn uuid_of(&self, name: &str) -> AppResult<String> {
        if !is_nickname(name) {
            return Err(nothing(NO_PLAYER, format!("no nickname {name:?}")));
        }
        let url = Self::address(&self.cfg.endpoints.mojang_api, &["users", "profiles", "minecraft", name])?;
        let id = self.ask(url).await?.and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string));
        Player { id, name: None }.uuid().ok_or_else(|| nothing(NO_PLAYER, format!("no player {name}")))
    }

    /// The skin Mojang names for `player`: its address and model.
    async fn look_up(&self, player: &Player) -> AppResult<(String, bool)> {
        let uuid = match (player.uuid(), player.nickname()) {
            (Some(uuid), _) => uuid,
            (None, Some(name)) => self.uuid_of(name).await?,
            (None, None) => return Err(nothing(NO_PLAYER, "no player")),
        };
        let url = Self::address(&self.cfg.endpoints.session, &["session", "minecraft", "profile", &uuid])?;
        let Some(profile) = self.ask(url).await? else {
            return Err(nothing(NO_PLAYER, format!("no profile {uuid}")));
        };
        skin_of(&profile).ok_or_else(|| nothing(NO_SKIN, format!("{uuid} wears a default skin")))
    }

    /// Texture file `file`, read once.
    fn load(&self, file: &str) -> Option<(Vec<u8>, String)> {
        if let Some(found) = lock(&self.decoded).get(file) {
            return Some(found.clone());
        }
        let png = render::normalise(&fs::read(self.textures_dir().join(file)).ok()?).ok()?;
        let read = (png.clone(), render::sha256(&png));
        lock(&self.decoded).insert(file.to_string(), read.clone());
        Some(read)
    }

    fn texture_of(&self, seen: &Seen) -> Option<SkinTexture> {
        let (png, hash) = self.load(&seen.file)?;
        Some(SkinTexture { png, slim: seen.slim, official: seen.official, hash })
    }

    /// The skin file at `url` on one of Mojang's texture hosts, kept under its address: an
    /// address names one picture for good.
    async fn fetch_texture(&self, url: &str) -> AppResult<Fetched> {
        let url = secure(url);
        let hosts = self.cfg.endpoints.texture_hosts.clone();
        let parsed = Url::parse(&url)
            .map_err(|e| AppError::new(ErrorCode::InvalidInput, format!("no texture address: {e}")))?;
        if !Self::is_texture_host(&hosts, &parsed) {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("{url} is not on a texture host")));
        }
        let file = format!("{}.png", digest(&url));
        let held = self.lock_for(&format!("file:{file}"));
        let _held = held.lock().await;
        if let Some((png, hash)) = self.load(&file) {
            return Ok((file, png, hash));
        }
        let dest = self.textures_dir().join(&file);
        // An unreadable file is no texture: fetched anew.
        let _ = fs::remove_file(&dest);
        let origin = Origin::new(move |u| Self::is_texture_host(&hosts, u));
        let task = DownloadTask::new(&url, &dest).origin(origin).at_most(render::MAX_BYTES as u64);
        self.downloader.download_one(task, &|_| {}).await?;
        let bytes = fs::read(&dest).map_err(|e| io_error(&dest, e))?;
        match render::normalise(&bytes) {
            Ok(png) => {
                let read = (png.clone(), render::sha256(&png));
                lock(&self.decoded).insert(file.clone(), read.clone());
                Ok((file, read.0, read.1))
            }
            Err(e) => {
                let _ = fs::remove_file(&dest);
                Err(e)
            }
        }
    }

    /// The skin at `url`, asked anew (its answer changes) and kept under its hash; its bytes
    /// come from where `origin` allows.
    async fn fetch_from_host(&self, url: &str, origin: Origin) -> AppResult<Fetched> {
        let temp = self.textures_dir().join(format!("fetched-{}.tmp", uuid::Uuid::new_v4().simple()));
        // A skin is a small picture: anything larger stops as it comes.
        let task = DownloadTask::new(url, &temp).origin(origin).at_most(render::MAX_BYTES as u64).transient();
        let fetched = self.downloader.download_one(task, &|_| {}).await;
        let bytes = fetched.and_then(|()| fs::read(&temp).map_err(|e| io_error(&temp, e)));
        let _ = fs::remove_file(&temp);
        let png = render::normalise(&bytes?)?;
        let hash = render::sha256(&png);
        let file = format!("{hash}.png");
        let path = self.textures_dir().join(&file);
        if !path.is_file() {
            atomic_write(&path, &png).map_err(|e| io_error(&path, e))?;
        }
        lock(&self.decoded).insert(file.clone(), (png.clone(), hash.clone()));
        Ok((file, png, hash))
    }

    /// A full skin of `identifier` from the first third-party host that has one.
    async fn hosts_skin(&self, identifier: &str) -> Option<Fetched> {
        for template in &self.cfg.endpoints.skin_hosts {
            let url = template.replace("{id}", &encode_segment(identifier));
            let host =
                Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
            let hosts = self.cfg.endpoints.texture_hosts.clone();
            let origin =
                Origin::new(move |u| u.host_str() == Some(host.as_str()) || Self::is_texture_host(&hosts, u));
            match self.fetch_from_host(&url, origin).await {
                Ok(found) => return Some(found),
                Err(e) => tracing::debug!("No skin of {identifier} at {url}: {}", e.detail),
            }
        }
        None
    }

    fn remember(&self, key: &str, seen: Seen) {
        let mut all = lock(&self.seen);
        all.insert(key.to_string(), seen);
        self.save(&all);
    }

    fn save(&self, all: &BTreeMap<String, Seen>) {
        let path = self.cfg.dir.join(PLAYERS_FILE);
        let value = serde_json::to_value(all).unwrap_or(Value::Null);
        if let Err(e) = write_json_file(&path, &value, 2) {
            tracing::debug!("Unable to keep the players' skins: {e}");
        }
    }

    fn until(ttl: Duration) -> u64 {
        now().saturating_add(ttl.as_secs())
    }

    /// `player`'s skin: Mojang's, else a third-party host's; what was looked up last when every
    /// source fails. `None` when nothing was ever had.
    pub async fn texture(&self, player: &Player) -> Option<SkinTexture> {
        let key = player.key()?;
        let held = self.lock_for(&key);
        let _held = held.lock().await;
        let known = lock(&self.seen).get(&key).cloned();
        if let Some(seen) = known.as_ref().filter(|s| s.until > now())
            && let Some(texture) = self.texture_of(seen)
        {
            return Some(texture);
        }
        // A third-party skin holds a day when Mojang said there is none, an hour when it failed.
        let ttl = match self.look_up(player).await {
            Ok((url, slim)) => match self.fetch_texture(&url).await {
                Ok((file, png, hash)) => {
                    let until = Self::until(self.cfg.official_ttl);
                    self.remember(&key, Seen { file, slim, official: true, until });
                    return Some(SkinTexture { png, slim, official: true, hash });
                }
                Err(e) => {
                    tracing::debug!("The skin Mojang names for {key} is out of reach: {}", e.detail);
                    self.cfg.official_ttl
                }
            },
            Err(e) if e.code == ErrorCode::NotFound => self.cfg.fallback_ttl,
            Err(e) => {
                tracing::debug!("Mojang cannot say {key}'s skin: {}", e.detail);
                self.cfg.official_ttl
            }
        };
        if let Some(identifier) = player.identifier()
            && let Some((file, png, hash)) = self.hosts_skin(&identifier).await
        {
            let slim = render::guess_slim(&png);
            self.remember(&key, Seen { file, slim, official: false, until: Self::until(ttl) });
            return Some(SkinTexture { png, slim, official: false, hash });
        }
        known.and_then(|seen| self.texture_of(&seen))
    }

    /// `player`'s skin as Mojang names it, asked now: `NotFound` with `reason` = `no_player` or
    /// `no_skin`, `RateLimited` or `Network` otherwise. Never a third-party host.
    pub async fn official(&self, player: &Player) -> AppResult<SkinTexture> {
        let key = player.key().ok_or_else(|| nothing(NO_PLAYER, "no player"))?;
        let held = self.lock_for(&key);
        let _held = held.lock().await;
        let (url, slim) = self.look_up(player).await?;
        let (file, png, hash) = self.fetch_texture(&url).await?;
        self.remember(&key, Seen { file, slim, official: true, until: Self::until(self.cfg.official_ttl) });
        Ok(SkinTexture { png, slim, official: true, hash })
    }

    /// The skin file at a web address the player gave (any host, wherever it redirects), its
    /// model guessed; asked anew each time.
    pub async fn at_link(&self, url: &str) -> AppResult<SkinTexture> {
        let origin = Origin::new(|u| matches!(u.scheme(), "http" | "https"));
        let (_, png, hash) = self.fetch_from_host(url, origin).await?;
        Ok(SkinTexture { slim: render::guess_slim(&png), official: false, hash, png })
    }

    /// `player` wears the skin at `url` on the model `slim` (their own account said so): it is
    /// fetched and is their skin from now on, without asking Mojang's session server.
    pub async fn wear(&self, player: &Player, url: &str, slim: bool) -> AppResult<SkinTexture> {
        let (file, png, hash) = self.fetch_texture(url).await?;
        if let Some(key) = player.key() {
            let held = self.lock_for(&key);
            let _held = held.lock().await;
            self.remember(
                &key,
                Seen { file, slim, official: true, until: Self::until(self.cfg.official_ttl) },
            );
        }
        Ok(SkinTexture { png, slim, official: true, hash })
    }

    /// `player`'s head, `size` pixels a side: drawn from their skin, else a head host's, else the
    /// last head host's address.
    pub async fn head(&self, player: &Player, size: u32) -> Option<Avatar> {
        if let Some(texture) = self.texture(player).await {
            return Some(Avatar::Png(render::head(&texture.png, size)));
        }
        self.head_from_hosts(&player.identifier()?, size).await
    }

    /// A fresh head kept from a head host, else the first host that returns an image, else the
    /// stale one, else the last host's address.
    async fn head_from_hosts(&self, identifier: &str, size: u32) -> Option<Avatar> {
        let held = self.lock_for(&format!("head:{identifier}"));
        let _held = held.lock().await;
        let file = self.head_file(identifier, size);
        if is_fresh(&file, self.cfg.fallback_ttl)
            && let Ok(bytes) = fs::read(&file)
            && !bytes.is_empty()
        {
            return Some(Avatar::Png(bytes));
        }
        let mut last_url = None;
        for template in &self.cfg.endpoints.head_hosts {
            let url =
                template.replace("{id}", &encode_segment(identifier)).replace("{size}", &size.to_string());
            if let Some(bytes) = self.fetch_head(&url).await {
                if let Err(e) = atomic_write(&file, &bytes) {
                    tracing::debug!("Unable to keep a head: {e}");
                }
                return Some(Avatar::Png(bytes));
            }
            last_url = Some(url);
        }
        match fs::read(&file) {
            Ok(bytes) if !bytes.is_empty() => Some(Avatar::Png(bytes)),
            _ => last_url.map(Avatar::Remote),
        }
    }

    async fn fetch_head(&self, url: &str) -> Option<Vec<u8>> {
        let response = self.client.get(url).timeout(self.cfg.timeout).send().await.ok()?;
        let is_image = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("image/"));
        if response.status().as_u16() != 200 || !is_image {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (!bytes.is_empty()).then(|| bytes.to_vec())
    }

    /// `player`'s whole figure seen from the front (a PNG, 64×128 times the render's scale), on
    /// their model; nothing while their skin cannot be had.
    pub async fn front(&self, player: &Player) -> Option<Vec<u8>> {
        let texture = self.texture(player).await?;
        Some(render::front(&texture.png, texture.slim))
    }

    /// Forgets what `player` was seen wearing (their skin changed), by UUID and by nickname: the
    /// next look asks Mojang.
    pub fn forget(&self, player: &Player) {
        let uuid = player.uuid();
        let name = player.nickname().map(str::to_string);
        {
            let mut all = lock(&self.seen);
            let before = all.len();
            all.remove(&format!("id:{}", uuid.clone().unwrap_or_default()));
            all.remove(&format!("name:{}", name.clone().unwrap_or_default().to_lowercase()));
            if all.len() != before {
                self.save(&all);
            }
        }
        let prefixes: Vec<String> =
            [uuid, name].into_iter().flatten().map(|i| format!("{}-", digest(&i))).collect();
        for entry in fs::read_dir(self.cfg.dir.join(HEADS_DIR)).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if prefixes.iter().any(|p| file.starts_with(p)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_for_the_img_tag() {
        assert_eq!(Avatar::Png(vec![1, 2, 3]).to_src(), "data:image/png;base64,AQID");
        assert_eq!(Avatar::Remote("https://x/y".into()).to_src(), "https://x/y");
    }

    #[test]
    fn identifiers_are_percent_encoded() {
        assert_eq!(encode_segment("Steve_1-a.b~"), "Steve_1-a.b~");
        assert_eq!(encode_segment("a b/ї"), "a%20b%2F%D1%97");
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
    fn players_are_kept_by_uuid_else_by_name() {
        let dashed = Player::new("01234567-89AB-cdef-0123-456789abcdef", "Steve");
        assert_eq!(dashed.uuid().as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(dashed.key().as_deref(), Some("id:0123456789abcdef0123456789abcdef"));
        assert_eq!(dashed.identifier().as_deref(), Some("0123456789abcdef0123456789abcdef"));
        let named = Player::named("  Alex ");
        assert_eq!((named.uuid(), named.key().as_deref()), (None, Some("name:alex")));
        assert_eq!(named.identifier().as_deref(), Some("Alex"));
        let odd = Player::new("not-a-uuid", "Іван");
        assert_eq!(odd.key().as_deref(), Some("name:іван"), "a UUID that is none is passed by");
        assert_eq!(Player::new(" ", "").key(), None);
    }

    #[test]
    fn nicknames_are_what_minecraft_takes() {
        assert!(is_nickname("Steve_42") && is_nickname("x"));
        for bad in ["", "two words", "Стів", "a-b", &"a".repeat(17), "../x"] {
            assert!(!is_nickname(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_session_profile_names_the_skin() {
        let profile = |textures: &str| serde_json::json!({"properties": [{"name": "textures", "value": STANDARD.encode(textures)}]});
        let classic = profile(r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/ab"}}}"#);
        assert_eq!(skin_of(&classic), Some(("http://textures.minecraft.net/texture/ab".into(), false)));
        let slim = profile(
            r#"{"profileName":"Alex","textures":{"SKIN":{"url":"https://t/x","metadata":{"model":"slim"}},"CAPE":{"url":"https://t/c"}}}"#,
        );
        assert_eq!(skin_of(&slim), Some(("https://t/x".into(), true)));
        assert_eq!(skin_of(&profile(r#"{"textures":{}}"#)), None, "a default skin has no SKIN");
        assert_eq!(skin_of(&serde_json::json!({"properties": [{"name": "textures", "value": "!"}]})), None);
    }
}
