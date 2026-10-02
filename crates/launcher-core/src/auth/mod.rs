//! Accounts: offline and Microsoft profiles, token storage, sign-in.

pub mod avatars;
pub mod crypto;
pub mod http;
pub mod msa;
pub mod offline;
pub mod page;
pub mod service;
pub mod store;
pub mod xbox;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An access token is refreshed when it expires within this many seconds.
pub const TOKEN_REFRESH_LEEWAY: i64 = 300;

pub const SCOPE: &str = "XboxLive.signin offline_access";
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Microsoft, Xbox and Minecraft addresses; tests point them at a local fake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthEndpoints {
    pub authorize: String,
    pub token: String,
    pub device_code: String,
    pub xbox_user: String,
    pub xsts: String,
    pub mc_login: String,
    pub mc_profile: String,
    /// Port of the registered redirect `http://localhost:8080/callback`; 0 picks a free one (tests).
    pub redirect_port: u16,
}

impl Default for AuthEndpoints {
    fn default() -> Self {
        let ms = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
        AuthEndpoints {
            authorize: format!("{ms}/authorize"),
            token: format!("{ms}/token"),
            device_code: format!("{ms}/devicecode"),
            xbox_user: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            mc_login: "https://api.minecraftservices.com/authentication/login_with_xbox".into(),
            mc_profile: "https://api.minecraftservices.com/minecraft/profile".into(),
            redirect_port: 8080,
        }
    }
}

impl AuthEndpoints {
    /// Everything on one local server (the test fake).
    pub fn local(base: &str) -> Self {
        let ms = format!("{base}/consumers/oauth2/v2.0");
        AuthEndpoints {
            authorize: format!("{ms}/authorize"),
            token: format!("{ms}/token"),
            device_code: format!("{ms}/devicecode"),
            xbox_user: format!("{base}/user/authenticate"),
            xsts: format!("{base}/xsts/authorize"),
            mc_login: format!("{base}/authentication/login_with_xbox"),
            mc_profile: format!("{base}/minecraft/profile"),
            redirect_port: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthTimings {
    pub http_timeout: Duration,
    /// Pause before retry `n` is `retry_backoff × n`.
    pub retry_backoff: Duration,
    pub browser_timeout: Duration,
    pub device_min_interval: Duration,
    pub slow_down_step: Duration,
    pub avatar_timeout: Duration,
}

impl Default for AuthTimings {
    fn default() -> Self {
        AuthTimings {
            http_timeout: Duration::from_secs(20),
            retry_backoff: Duration::from_millis(600),
            browser_timeout: Duration::from_secs(180),
            device_min_interval: Duration::from_secs(1),
            slow_down_step: Duration::from_secs(2),
            avatar_timeout: Duration::from_secs(4),
        }
    }
}

impl AuthTimings {
    /// Same logic with test-sized waits.
    pub fn fast() -> Self {
        AuthTimings {
            http_timeout: Duration::from_secs(5),
            retry_backoff: Duration::from_millis(1),
            browser_timeout: Duration::from_secs(5),
            device_min_interval: Duration::from_millis(10),
            slow_down_step: Duration::from_millis(20),
            avatar_timeout: Duration::from_secs(2),
        }
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Opens a URL in the user's browser; `false` when that is impossible.
pub trait UrlOpener: Send + Sync + 'static {
    fn open(&self, url: &str) -> bool;
}

/// For headless runs (smoke test, unit tests): the browser flow falls back to a device code.
pub struct NoOpener;

impl UrlOpener for NoOpener {
    fn open(&self, _url: &str) -> bool {
        false
    }
}

/// Avatar hosts in the original launcher's order; `{id}` and `{size}` are filled in.
pub const DEFAULT_AVATAR_SOURCES: [&str; 3] = [
    "https://mc-heads.net/avatar/{id}/{size}",
    "https://minotar.net/helm/{id}/{size}.png",
    "https://mineskin.eu/avatar/{id}/{size}",
];

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub endpoints: AuthEndpoints,
    pub timings: AuthTimings,
    /// Holds `profiles.json` and `profile-token.key`.
    pub state_dir: PathBuf,
    pub avatar_dir: PathBuf,
    pub avatar_sources: Vec<String>,
}

impl AuthConfig {
    pub fn new(client_id: &str, state_dir: &Path, cache_dir: &Path) -> Self {
        AuthConfig {
            client_id: client_id.to_string(),
            endpoints: AuthEndpoints::default(),
            timings: AuthTimings::default(),
            state_dir: state_dir.to_path_buf(),
            avatar_dir: cache_dir.join("avatars"),
            avatar_sources: DEFAULT_AVATAR_SOURCES.iter().map(|s| s.to_string()).collect(),
        }
    }
}
