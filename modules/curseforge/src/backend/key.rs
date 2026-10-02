//! The launcher's CurseForge API key. The official builds carry it from the release workflow's
//! secret; a developer or a fork sets the variable when the launcher runs. It never lives in the
//! repository, and without it the module offers nothing.

use std::sync::Arc;

use reqwest::header::HeaderValue;
use secrecy::{ExposeSecret, SecretString};

/// The variable that carries the key: at build time (the release workflow, `cargo xtask dev`) and
/// when the launcher runs (it wins).
pub const KEY_VAR: &str = "CURSEFORGE_API_KEY";
/// The header CurseForge reads the key from.
pub const KEY_HEADER: &str = "x-api-key";

/// The key, kept out of `Debug` output and wiped from memory when dropped.
#[derive(Clone)]
pub struct ApiKey(Arc<SecretString>);

impl ApiKey {
    /// The key without the spaces around it; none when blank or not a valid header value.
    pub fn new(key: &str) -> Option<ApiKey> {
        let key = key.trim();
        if key.is_empty() || HeaderValue::from_str(key).is_err() {
            return None;
        }
        Some(ApiKey(Arc::new(SecretString::from(key.to_string()))))
    }

    /// The key as a header value marked sensitive (its `Debug` shows no value).
    pub fn header(&self) -> HeaderValue {
        let mut value = HeaderValue::from_str(self.0.expose_secret()).expect("checked when made");
        value.set_sensitive(true);
        value
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(..)")
    }
}

/// CurseForge's own addresses, the only ones the key goes to: its API and its file CDN
/// (`edge.forgecdn.net`, `mediafilez.forgecdn.net`), over HTTPS; in the module's own tests, the
/// fake CurseForge on this machine too.
pub fn keyed_host(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    let curseforge = host == "api.curseforge.com" || host.ends_with(".forgecdn.net");
    let fake = cfg!(feature = "test-servers") && url.scheme() == "http" && host == "127.0.0.1";
    (url.scheme() == "https" && url.port().is_none() && curseforge) || fake
}

/// The key the launcher uses: the one set when it runs, else the one it was built with.
pub fn resolve_key_from(runtime: Option<String>, built_in: Option<&str>) -> Option<ApiKey> {
    runtime.as_deref().and_then(ApiKey::new).or_else(|| built_in.and_then(ApiKey::new))
}

/// The key of this launcher, if it has one.
pub fn resolve_key() -> Option<ApiKey> {
    resolve_key_from(std::env::var(KEY_VAR).ok(), option_env!("CURSEFORGE_API_KEY"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_never_shows() {
        let key = ApiKey::new("secret-value-123").unwrap();
        assert_eq!(format!("{key:?}"), "ApiKey(..)");
        let header = key.header();
        assert!(header.is_sensitive());
        assert_eq!(header.to_str().unwrap(), "secret-value-123");
        assert!(!format!("{header:?}").contains("secret-value-123"));
    }

    #[test]
    fn the_key_goes_to_curseforge_only() {
        let keyed = |raw: &str| keyed_host(&reqwest::Url::parse(raw).unwrap());
        for yes in [
            "https://api.curseforge.com/v1/mods/1",
            "https://edge.forgecdn.net/files/1/2/a.jar",
            "https://mediafilez.forgecdn.net/files/1/2/a.jar",
        ] {
            assert!(keyed(yes), "{yes}");
        }
        for no in [
            "http://edge.forgecdn.net/files/a.jar",
            "https://forgecdn.net.evil.com/a.jar",
            "https://evilforgecdn.net/a.jar",
            "https://api.curseforge.com.evil.com/v1",
            "https://cdn.modrinth.com/data/a.jar",
            "https://media.forgecdn.net:8443/a.png",
        ] {
            assert!(!keyed(no), "{no}");
        }
    }

    #[test]
    fn the_running_launcher_s_key_wins_and_blank_is_none() {
        let pick = |runtime: Option<&str>, built: Option<&str>| {
            resolve_key_from(runtime.map(str::to_string), built)
                .map(|k| k.header().to_str().unwrap().to_string())
        };
        assert_eq!(pick(Some("dev"), Some("built")).as_deref(), Some("dev"));
        assert_eq!(pick(None, Some("built")).as_deref(), Some("built"));
        assert_eq!(pick(Some("  "), Some("built")).as_deref(), Some("built"), "a blank variable is no key");
        assert_eq!(pick(Some(" dev\n"), None).as_deref(), Some("dev"), "spaces around are not the key");
        assert_eq!(pick(None, Some("")), None);
        assert_eq!(pick(None, None), None);
    }
}
