//! Picking the release and the file to install.

use launcher_shared::branding::{APP_NAME, EDITION};
use launcher_shared::{ReleaseOs, UpdateChannel, UpdateInfo, release_file_names};
use reqwest::Url;
use serde::Deserialize;

use super::version::Version;
use crate::paths::Os;

#[derive(Debug, Clone, Deserialize)]
pub struct GhRelease {
    pub id: u64,
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GhAsset {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    pub browser_download_url: String,
}

impl GhAsset {
    /// A missing `state` counts as uploaded (older API responses).
    pub fn is_uploaded(&self) -> bool {
        self.state.as_deref().is_none_or(|s| s == "uploaded")
    }

    /// Lower-case hex SHA-256 from `digest: "sha256:<hex>"`.
    pub fn sha256(&self) -> Option<String> {
        let hex = self.digest.as_deref()?.strip_prefix("sha256:")?;
        (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Arch {
        if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
    /// Running as an AppImage (see `stage::current_appimage`): only then are `.AppImage` files eligible.
    pub appimage: bool,
}

impl Platform {
    pub fn current() -> Platform {
        Platform {
            os: Os::current(),
            arch: Arch::current(),
            appimage: std::env::current_exe()
                .ok()
                .and_then(|exe| super::stage::current_appimage(&exe))
                .is_some(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub release: GhRelease,
    pub version: Version,
    pub channel: UpdateChannel,
}

impl Candidate {
    pub fn info(&self, asset: &GhAsset) -> UpdateInfo {
        UpdateInfo {
            version: self.version.as_str().to_string(),
            channel: self.channel,
            notes: self.release.body.clone().unwrap_or_default(),
            asset_name: asset.name.clone(),
            size: asset.size,
            published_at: self.release.published_at.clone(),
        }
    }
}

fn release_version(release: &GhRelease) -> Option<Version> {
    Version::parse(&release.tag_name).or_else(|| release.name.as_deref().and_then(Version::parse))
}

/// Newest non-draft release strictly newer than `current`; pre-releases only with `include_beta`.
pub fn pick_release(releases: Vec<GhRelease>, current: &Version, include_beta: bool) -> Option<Candidate> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|release| {
            let version = release_version(&release)?;
            if version <= *current {
                return None;
            }
            let beta = release.prerelease || version.is_prerelease();
            if beta && !include_beta {
                return None;
            }
            let channel = if beta { UpdateChannel::Beta } else { UpdateChannel::Stable };
            Some(Candidate { release, version, channel })
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// The release files made for this platform in edition `edition` of app `app`, named
/// `{app}-{edition}…` (the build profile's name and edition): a build updates only from its own
/// edition's files.
fn exact_names_for(app: &str, edition: &str, p: &Platform) -> Vec<String> {
    let os = match p.os {
        Os::Windows => ReleaseOs::Windows,
        Os::Linux => ReleaseOs::Linux,
        Os::MacOs => ReleaseOs::MacOs,
        Os::Other => return Vec::new(),
    };
    release_file_names(app, edition, os, p.arch.as_str(), p.appimage)
}

/// Name `cargo xtask mock-releases publish` gives the asset for this platform.
pub fn preferred_asset_name(p: &Platform) -> String {
    preferred_name_for(APP_NAME, EDITION, p)
}

/// `preferred_asset_name` for app `app` and edition `edition`: the last exact name (the one
/// without an architecture where there is one).
fn preferred_name_for(app: &str, edition: &str, p: &Platform) -> String {
    exact_names_for(app, edition, p).pop().unwrap_or_else(|| format!("{app}-{edition}"))
}

/// This build's file among `assets`: one of its edition's exact names, or none.
pub fn pick_asset<'a>(assets: &'a [GhAsset], p: &Platform) -> Option<&'a GhAsset> {
    pick_asset_for(assets, APP_NAME, EDITION, p)
}

/// `pick_asset` for app `app` and edition `edition`.
fn pick_asset_for<'a>(assets: &'a [GhAsset], app: &str, edition: &str, p: &Platform) -> Option<&'a GhAsset> {
    let usable: Vec<&GhAsset> = assets.iter().filter(|a| a.is_uploaded()).collect();
    exact_names_for(app, edition, p)
        .iter()
        .find_map(|name| usable.iter().find(|a| a.name.eq_ignore_ascii_case(name)).copied())
}

/// Safe single path component for a downloaded file.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() { "update.bin".to_string() } else { trimmed.to_string() }
}

/// Only a build made to try updates against the local mock (`cargo xtask mock-releases`, feature
/// `mock-updates`) trusts a server on this machine; a release never does.
pub(crate) const LOOPBACK_TRUSTED: bool = cfg!(feature = "mock-updates");

/// Downloads (and their redirects) may only come from GitHub, or from the very same loopback
/// server when the build points at the local mock (`LOOPBACK_TRUSTED`).
pub fn download_url_allowed(api_base: &Url, url: &Url) -> bool {
    download_allowed_with(api_base, url, LOOPBACK_TRUSTED)
}

/// `download_url_allowed`, trusting a loopback mock only when `trust_loopback`.
pub(crate) fn download_allowed_with(api_base: &Url, url: &Url, trust_loopback: bool) -> bool {
    let loopback = |u: &Url| matches!(u.host_str(), Some("127.0.0.1") | Some("localhost"));
    if trust_loopback && loopback(api_base) {
        return url.scheme() == api_base.scheme()
            && url.host_str() == api_base.host_str()
            && url.port_or_known_default() == api_base.port_or_known_default();
    }
    if url.scheme() != "https" {
        return false;
    }
    match url.host_str() {
        Some(host) => {
            host == "github.com" || host == "api.github.com" || host.ends_with(".githubusercontent.com")
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GhAsset {
        GhAsset {
            id: 1,
            name: name.into(),
            size: 10,
            state: Some("uploaded".into()),
            digest: Some(format!("sha256:{}", "a".repeat(64))),
            browser_download_url: format!("https://github.com/o/r/releases/download/v1/{name}"),
        }
    }

    fn release(tag: &str, draft: bool, prerelease: bool) -> GhRelease {
        GhRelease {
            id: 7,
            tag_name: tag.into(),
            name: None,
            body: Some("notes".into()),
            draft,
            prerelease,
            published_at: None,
            assets: vec![asset("Launcher.exe")],
        }
    }

    fn platform(os: Os, arch: Arch, appimage: bool) -> Platform {
        Platform { os, arch, appimage }
    }

    #[test]
    fn picks_the_highest_newer_release_for_the_channel() {
        let current = Version::parse("0.1.0").unwrap();
        let list = || {
            vec![
                release("v0.1.0", false, false),
                release("v0.2.0", false, false),
                release("v0.1.5", false, false),
                release("v9.9.9", true, false),
                release("v0.3.0-beta.1", false, true),
            ]
        };
        let stable = pick_release(list(), &current, false).unwrap();
        assert_eq!(stable.version.as_str(), "0.2.0");
        assert_eq!(stable.channel, UpdateChannel::Stable);
        let beta = pick_release(list(), &current, true).unwrap();
        assert_eq!(beta.version.as_str(), "0.3.0-beta.1");
        assert_eq!(beta.channel, UpdateChannel::Beta);
    }

    #[test]
    fn nothing_when_up_to_date_and_bad_tags_fall_back_to_the_name() {
        let current = Version::parse("0.2.0").unwrap();
        let same = vec![release("v0.2.0", false, false), release("v0.1.0", false, false)];
        assert!(pick_release(same, &current, true).is_none());
        let mut named = release("latest", false, false);
        named.name = Some("v0.5.0".into());
        let mut junk = release("nightly", false, false);
        junk.name = Some("Nightly build".into());
        let picked = pick_release(vec![named, junk], &current, false).unwrap();
        assert_eq!(picked.version.as_str(), "0.5.0");
    }

    #[test]
    fn prerelease_version_without_the_flag_is_still_beta() {
        let current = Version::parse("0.1.0").unwrap();
        assert!(pick_release(vec![release("v0.2.0-rc.1", false, false)], &current, false).is_none());
        let picked = pick_release(vec![release("v0.2.0-rc.1", false, false)], &current, true).unwrap();
        assert_eq!(picked.channel, UpdateChannel::Beta);
    }

    #[test]
    fn asset_names_carry_the_edition() {
        let names = |p: Platform| exact_names_for("App", "tensa", &p);
        assert_eq!(
            names(platform(Os::Windows, Arch::X86_64, false)),
            ["App-tensa-x86_64.exe", "App-tensa.exe"]
        );
        assert_eq!(names(platform(Os::Linux, Arch::X86_64, true)), ["App-tensa-x86_64.AppImage"]);
        assert_eq!(names(platform(Os::Linux, Arch::X86_64, false)), ["App-tensa-x86_64"]);
        assert_eq!(
            names(platform(Os::MacOs, Arch::Aarch64, false)),
            ["App-tensa-aarch64.dmg", "App-tensa-universal.dmg"]
        );
        assert_eq!(
            preferred_name_for("App", "standard", &platform(Os::Windows, Arch::X86_64, false)),
            "App-standard.exe"
        );
    }

    #[test]
    fn each_edition_updates_from_its_own_assets() {
        let names = [
            "App-standard.exe",
            "App-tensa.exe",
            "App-standard-Setup.exe",
            "App-tensa-x86_64.AppImage",
            "App-standard-x86_64.AppImage",
            "App-tensa-universal.dmg",
            "App-standard-universal.dmg",
        ];
        let assets: Vec<GhAsset> = names.iter().map(|n| asset(n)).collect();
        let pick =
            |edition: &str, p: Platform| pick_asset_for(&assets, "App", edition, &p).map(|a| a.name.clone());
        let windows = platform(Os::Windows, Arch::X86_64, false);
        assert_eq!(pick("standard", windows).as_deref(), Some("App-standard.exe"));
        assert_eq!(pick("tensa", windows).as_deref(), Some("App-tensa.exe"));
        let appimage = platform(Os::Linux, Arch::X86_64, true);
        assert_eq!(pick("tensa", appimage).as_deref(), Some("App-tensa-x86_64.AppImage"));
        assert_eq!(
            pick("standard", platform(Os::MacOs, Arch::Aarch64, false)).as_deref(),
            Some("App-standard-universal.dmg")
        );
    }

    #[test]
    fn no_asset_of_this_edition_means_no_update() {
        let others = vec![
            asset("App-tensa.exe"),
            asset("App.exe"),
            asset("app-win.exe"),
            asset("App-standard-Setup.exe"),
        ];
        assert!(
            pick_asset_for(&others, "App", "standard", &platform(Os::Windows, Arch::X86_64, false)).is_none()
        );
        let appimages = vec![asset("something.appimage"), asset("App-tensa-x86_64.AppImage")];
        assert!(
            pick_asset_for(&appimages, "App", "standard", &platform(Os::Linux, Arch::X86_64, true)).is_none()
        );
    }

    #[test]
    fn preferred_names_are_picked_first() {
        for p in [
            platform(Os::Windows, Arch::X86_64, false),
            platform(Os::Linux, Arch::X86_64, true),
            platform(Os::Linux, Arch::X86_64, false),
            platform(Os::MacOs, Arch::Aarch64, false),
        ] {
            let name = preferred_asset_name(&p);
            assert_eq!(pick_asset(&[asset("other.txt"), asset(&name)], &p).unwrap().name, name);
        }
    }

    #[test]
    fn skips_assets_that_are_not_uploaded() {
        let own = preferred_asset_name(&platform(Os::Windows, Arch::X86_64, false));
        let mut starter = asset(&own);
        starter.state = Some("starter".into());
        let mut missing_state = asset(&own);
        missing_state.state = None;
        let win = platform(Os::Windows, Arch::X86_64, false);
        assert!(pick_asset(&[starter], &win).is_none());
        assert!(pick_asset(&[missing_state], &win).is_some());
    }

    #[test]
    fn sha256_requires_a_valid_digest() {
        let mut a = asset("Launcher.exe");
        assert_eq!(a.sha256().unwrap(), "a".repeat(64));
        a.digest = Some(format!("sha256:{}", "AB".repeat(32)));
        assert_eq!(a.sha256().unwrap(), "ab".repeat(32));
        for bad in [
            None,
            Some("md5:abc".to_string()),
            Some("sha256:xyz".to_string()),
            Some("sha256:abc".to_string()),
        ] {
            a.digest = bad;
            assert!(a.sha256().is_none());
        }
    }

    #[test]
    fn file_names_are_sanitised() {
        assert_eq!(sanitize_file_name("App<Launcher>:*?.exe"), "App_Launcher____.exe");
        assert_eq!(sanitize_file_name("../../evil.exe"), "_.._evil.exe");
        assert_eq!(sanitize_file_name("a\u{7}b"), "a_b");
        assert_eq!(sanitize_file_name("   "), "update.bin");
    }

    #[test]
    fn download_hosts_are_restricted() {
        let github = Url::parse("https://api.github.com").unwrap();
        let ok = |u: &str| download_url_allowed(&github, &Url::parse(u).unwrap());
        assert!(ok("https://github.com/o/r/releases/download/v1/Launcher.exe"));
        assert!(ok("https://objects.githubusercontent.com/github-production-release-asset/1"));
        assert!(!ok("http://github.com/o/r/x"));
        assert!(!ok("https://github.com.evil.io/x"));
        assert!(!ok("https://evilgithubusercontent.com/x"));
        assert!(!ok("http://127.0.0.1:1430/download/1/x"));
        let mock = Url::parse("http://127.0.0.1:1430").unwrap();
        assert!(download_url_allowed(&mock, &Url::parse("http://127.0.0.1:1430/download/1/x").unwrap()));
        assert!(!download_url_allowed(&mock, &Url::parse("http://127.0.0.1:9999/download/1/x").unwrap()));
        assert!(!download_url_allowed(&mock, &Url::parse("https://github.com/x").unwrap()));
    }

    #[test]
    fn loopback_update_sources_need_the_mock_feature() {
        let mock = Url::parse("http://127.0.0.1:1430").unwrap();
        let file = Url::parse("http://127.0.0.1:1430/download/1/x").unwrap();
        assert!(
            !download_allowed_with(&mock, &file, false),
            "a build without the feature trusts no loopback"
        );
        assert!(download_allowed_with(&mock, &file, true));
        assert!(super::super::github::api_base_with("http://127.0.0.1:1430", false).is_err());
        assert!(super::super::github::api_base_with("http://localhost:1430", true).is_ok());
        assert!(super::super::github::api_base_with("https://api.github.com", false).is_ok());
    }
}
