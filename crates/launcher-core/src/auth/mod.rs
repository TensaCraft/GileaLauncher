//! Accounts: offline and Microsoft profiles, token storage, sign-in.

pub mod crypto;
pub mod http;
pub mod injector;
pub mod msa;
pub mod offline;
pub mod page;
pub mod provider;
pub mod servers;
pub mod service;
pub mod store;
pub mod xbox;
pub mod yggdrasil;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An access token is refreshed when it expires within this many seconds.
pub const TOKEN_REFRESH_LEEWAY: i64 = 300;
/// A launch renews a session that ends sooner than this (sessions last a day): a game played on it
/// would drop out of servers when it ends.
pub const LAUNCH_SESSION_MARGIN: i64 = 12 * 60 * 60;

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
    /// How long a deletion waits for the account's server to take the sign-out.
    pub sign_out_within: Duration,
}

impl Default for AuthTimings {
    fn default() -> Self {
        AuthTimings {
            http_timeout: Duration::from_secs(20),
            retry_backoff: Duration::from_millis(600),
            browser_timeout: Duration::from_secs(180),
            device_min_interval: Duration::from_secs(1),
            slow_down_step: Duration::from_secs(2),
            sign_out_within: Duration::from_secs(5),
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
            sign_out_within: Duration::from_millis(300),
        }
    }
}

/// A password or a token as `Debug` prints it: whether there is one, never what it is.
pub(crate) struct Hidden(pub bool);

impl Hidden {
    pub(crate) fn of(secret: &Option<String>) -> Hidden {
        Hidden(secret.is_some())
    }
}

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0 { "<hidden>" } else { "None" })
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

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub endpoints: AuthEndpoints,
    pub timings: AuthTimings,
    /// Holds `profiles.json`, `auth_servers.json` and `profile-token.key`.
    pub state_dir: PathBuf,
    /// The auth servers known before they are asked for (Ely.by); tests point them at a fake.
    pub servers: Vec<servers::Preset>,
}

impl AuthConfig {
    pub fn new(client_id: &str, state_dir: &Path) -> Self {
        AuthConfig {
            client_id: client_id.to_string(),
            endpoints: AuthEndpoints::default(),
            timings: AuthTimings::default(),
            state_dir: state_dir.to_path_buf(),
            servers: servers::presets(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::provider::ProviderAccount;
    use super::service::LaunchIdentity;
    use super::yggdrasil::{Credentials, Session};
    use launcher_shared::AccountKind;

    /// What a log could print of a sign-in, a session or a launch: never a password or a token.
    #[test]
    fn secrets_printed_for_a_log_stay_hidden() {
        let credentials =
            Credentials { login: "alex", password: "hunter2", code: Some("123456"), client_token: "c-1" };
        let session = Session {
            access_token: "token-1".into(),
            client_token: "c-1".into(),
            profile: None,
            profiles: Vec::new(),
        };
        let account =
            ProviderAccount { id: "id-1".into(), name: "Builder".into(), token: "site-token".into() };
        let identity = LaunchIdentity {
            username: "Builder".into(),
            uuid: "id-1".into(),
            access_token: "mc-access".into(),
            xuid: None,
            client_id: None,
            kind: AccountKind::Microsoft,
            agent: None,
        };
        let printed = [
            format!("{credentials:?}"),
            format!("{session:?}"),
            format!("{account:?}"),
            format!("{identity:?}"),
        ];
        for (text, secret) in printed.iter().zip(["hunter2", "token-1", "site-token", "mc-access"]) {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(
            printed[0].contains("alex") && printed[2].contains("Builder") && printed[3].contains("Builder")
        );
        assert!(!printed[0].contains("123456"), "nor the second factor");
    }
}
