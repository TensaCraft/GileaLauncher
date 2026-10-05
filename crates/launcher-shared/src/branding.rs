//! Build-profile constants — the only place in code that holds the brand. `cargo xtask build` sets
//! the `LAUNCHER_*` env vars from `build-profiles/<name>.toml`; the defaults match the `standard`
//! profile so plain `cargo build` works.

const fn or(value: Option<&'static str>, default: &'static str) -> &'static str {
    match value {
        Some(v) => v,
        None => default,
    }
}

pub const APP_NAME: &str = or(option_env!("LAUNCHER_APP_NAME"), "GileaLauncher");
pub const PROFILE: &str = or(option_env!("LAUNCHER_PROFILE"), "standard");
/// The app's reverse-DNS identifier (bundle id, shortcut ids).
pub const IDENTIFIER: &str = or(option_env!("LAUNCHER_IDENTIFIER"), "com.gilealauncher");
/// The edition (`standard`, `tensa`, …): which modules the build has and which release files it
/// updates from (`{APP_NAME}-{EDITION}…`).
pub const EDITION: &str = or(option_env!("LAUNCHER_EDITION"), "standard");
pub const SUPPORT_URL: &str =
    or(option_env!("LAUNCHER_SUPPORT_URL"), "https://discord.com/invite/mftAjQA4Pp");
/// Where bugs and suggestions go; empty means nowhere.
pub const ISSUES_URL: &str =
    or(option_env!("LAUNCHER_ISSUES_URL"), "https://github.com/TensaCraft/GileaLauncher/issues");
/// The folder of the setup wizard's pictures (`<lang>/<name>.jpg`), loaded only when shown; empty
/// means the wizard shows icons instead.
pub const MEDIA_URL: &str = or(
    option_env!("LAUNCHER_MEDIA_URL"),
    "https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup",
);
/// `owner/repo`; empty means launcher updates are disabled.
pub const UPDATE_REPO: &str = or(option_env!("LAUNCHER_UPDATE_REPO"), "");
pub const DEFAULT_UPDATE_API: &str = "https://api.github.com";
/// GitHub REST API root; build profiles may point it at the local mock server.
pub const UPDATE_API: &str = or(option_env!("LAUNCHER_UPDATE_API"), DEFAULT_UPDATE_API);
pub const MS_CLIENT_ID: &str =
    or(option_env!("LAUNCHER_MS_CLIENT_ID"), "7181ef73-10e0-4354-9984-8b8343f1513d");
/// `LAUNCHER_VERSION` lets `cargo xtask mock-releases publish` build other versions without editing Cargo.toml.
pub const VERSION: &str = or(option_env!("LAUNCHER_VERSION"), env!("CARGO_PKG_VERSION"));
