//! Build profiles: which modules are compiled in and which branding values are baked in.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const KNOWN_MODULES: &[&str] = &["tensa", "modrinth", "curseforge", "backups", "reports", "diagnostics"];
/// Modules put off until after the release: `--modules all` leaves them out; named, they are
/// built in (`--modules diagnostics`).
pub const POSTPONED_MODULES: &[&str] = &["diagnostics"];

#[derive(Debug, Deserialize)]
struct ProfileFile {
    profile: ProfileSection,
    branding: Branding,
    #[serde(default)]
    modules: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Deserialize)]
struct ProfileSection {
    name: String,
    modules: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Branding {
    app_name: String,
    identifier: String,
    #[serde(default)]
    support_url: String,
    /// Where bugs and suggestions go (the repository's issues).
    #[serde(default)]
    issues_url: String,
    #[serde(default)]
    update_repo: String,
    /// GitHub API root; only test profiles set it (the local mock server).
    #[serde(default)]
    update_api: String,
    /// Which modules the build has and which release files it updates from (`[a-z0-9]+`).
    edition: String,
    ms_client_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    pub name: String,
    pub modules: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl BuildSpec {
    /// The app's name in this profile (`[branding] app_name`).
    pub fn app_name(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "LAUNCHER_APP_NAME").map_or("", |(_, v)| v.as_str())
    }

    /// The profile's edition (`[branding] edition`).
    pub fn edition(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "LAUNCHER_EDITION").map_or("", |(_, v)| v.as_str())
    }

    /// The app's identifier (`[branding] identifier`).
    pub fn identifier(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "LAUNCHER_IDENTIFIER").map_or("", |(_, v)| v.as_str())
    }

    /// The GitHub repository updates come from (`[branding] update_repo`; empty: none).
    pub fn update_repo(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "LAUNCHER_UPDATE_REPO").map_or("", |(_, v)| v.as_str())
    }

    pub fn features(&self) -> Vec<String> {
        self.modules.iter().map(|m| format!("mod-{m}")).collect()
    }

    /// The profile's `update_api` (empty: GitHub's).
    pub fn update_api(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "LAUNCHER_UPDATE_API").map_or("", |(_, v)| v.as_str())
    }

    /// Updates come from a server on this machine (the mock): a test profile.
    pub fn local_updates(&self) -> bool {
        let rest = self.update_api().strip_prefix("http://").unwrap_or_default();
        matches!(rest.split(['/', ':']).next(), Some("127.0.0.1" | "localhost"))
    }

    /// The app's features: the modules', and `mock-updates` for a profile whose updates are local.
    pub fn app_features(&self) -> Vec<String> {
        let mut features = self.features();
        if self.local_updates() {
            features.push("mock-updates".to_string());
        }
        features
    }

    pub fn features_arg(&self) -> String {
        self.features().join(",")
    }
}

fn validate_modules(modules: &[String]) -> Result<()> {
    let mut seen = Vec::new();
    for m in modules {
        if !KNOWN_MODULES.contains(&m.as_str()) {
            bail!("unknown module '{m}' (known: {})", KNOWN_MODULES.join(", "));
        }
        if seen.contains(m) {
            bail!("module '{m}' is listed twice");
        }
        seen.push(m.clone());
    }
    Ok(())
}

fn validate_repo(repo: &str) -> Result<()> {
    if repo.is_empty() {
        return Ok(());
    }
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty() || p.contains(char::is_whitespace)) {
        bail!("update_repo must look like 'owner/repo', got '{repo}'");
    }
    Ok(())
}

/// `https://…`, or plain `http://` to 127.0.0.1/localhost only (the mock server).
pub fn validate_update_api(api: &str) -> Result<()> {
    if api.is_empty() {
        return Ok(());
    }
    if let Some(rest) = api.strip_prefix("https://") {
        if rest.is_empty() || rest.starts_with('/') || rest.contains(char::is_whitespace) {
            bail!("update_api must be a URL, got '{api}'");
        }
        return Ok(());
    }
    if let Some(rest) = api.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or_default();
        if host == "127.0.0.1" || host == "localhost" {
            return Ok(());
        }
    }
    bail!("update_api must use https (plain http is allowed only for 127.0.0.1/localhost), got '{api}'")
}

/// An edition is one lower-case word of letters and digits (it names release files).
fn validate_edition(edition: &str) -> Result<()> {
    if edition.is_empty() || !edition.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        bail!("edition must be lower-case letters and digits, got '{edition}'");
    }
    Ok(())
}

pub fn parse_profile(text: &str) -> Result<BuildSpec> {
    let file: ProfileFile = toml::from_str(text).context("invalid build profile TOML")?;
    validate_modules(&file.profile.modules)?;
    validate_edition(&file.branding.edition)?;
    validate_repo(&file.branding.update_repo)?;
    validate_update_api(&file.branding.update_api)?;
    for key in file.modules.keys() {
        if !KNOWN_MODULES.contains(&key.as_str()) {
            bail!("unknown module section [modules.{key}]");
        }
    }
    let b = &file.branding;
    let mut env = vec![
        ("LAUNCHER_PROFILE".to_string(), file.profile.name.clone()),
        ("LAUNCHER_APP_NAME".to_string(), b.app_name.clone()),
        ("LAUNCHER_IDENTIFIER".to_string(), b.identifier.clone()),
        ("LAUNCHER_EDITION".to_string(), b.edition.clone()),
        ("LAUNCHER_SUPPORT_URL".to_string(), b.support_url.clone()),
        ("LAUNCHER_ISSUES_URL".to_string(), b.issues_url.clone()),
        ("LAUNCHER_UPDATE_REPO".to_string(), b.update_repo.clone()),
        ("LAUNCHER_MS_CLIENT_ID".to_string(), b.ms_client_id.clone()),
    ];
    if !b.update_api.is_empty() {
        env.push(("LAUNCHER_UPDATE_API".to_string(), b.update_api.clone()));
    }
    for (module, table) in &file.modules {
        for (key, value) in table {
            let value = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            env.push((format!("LAUNCHER_MOD_{}_{}", module.to_uppercase(), key.to_uppercase()), value));
        }
    }
    Ok(BuildSpec { name: file.profile.name, modules: file.profile.modules, env })
}

pub fn load_profile(root: &Path, name: &str) -> Result<BuildSpec> {
    let path = root.join("build-profiles").join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    parse_profile(&text).with_context(|| format!("in {}", path.display()))
}

/// Every `modules/<id>` directory that has a `Cargo.toml`, sorted alphabetically. This is what
/// `--modules all` expands to, so a new module needs no edit here — just a `Cargo.toml`.
pub fn discover_modules(root: &Path) -> Result<Vec<String>> {
    let dir = root.join("modules");
    let mut modules = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir()
            && path.join("Cargo.toml").is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            modules.push(name.to_string());
        }
    }
    modules.sort();
    Ok(modules)
}

/// `--modules a,b` override: keeps the profile branding, replaces the module list.
/// `--modules all` expands to every discovered module but the postponed ones; `--modules none`
/// (or `""`) clears the list.
pub fn with_modules(mut base: BuildSpec, root: &Path, list: &str) -> Result<BuildSpec> {
    let modules: Vec<String> = match list.trim() {
        "all" => {
            discover_modules(root)?.into_iter().filter(|m| !POSTPONED_MODULES.contains(&m.as_str())).collect()
        }
        "none" => Vec::new(),
        other => other.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect(),
    };
    validate_modules(&modules)?;
    base.modules = modules;
    base.name = "custom".into();
    for (k, v) in base.env.iter_mut() {
        if k == "LAUNCHER_PROFILE" {
            *v = "custom".into();
        }
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD: &str = include_str!("../../build-profiles/standard.toml");
    const FULL: &str = include_str!("../../build-profiles/full.toml");
    const CORE: &str = include_str!("../../build-profiles/core.toml");
    const MOCK: &str = include_str!("../../build-profiles/mock-updates.toml");

    /// `text` with its `update_repo = …` line replaced by `line`.
    fn with_repo_line(text: &str, line: &str) -> String {
        text.lines()
            .map(|l| if l.starts_with("update_repo = ") { line } else { l })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_profile_names_a_valid_edition() {
        let edition = |text: &str| {
            let spec = parse_profile(text).unwrap();
            spec.env.iter().find(|(k, _)| k == "LAUNCHER_EDITION").map(|(_, v)| v.clone())
        };
        assert_eq!(edition(STANDARD).as_deref(), Some("standard"));
        assert_eq!(edition(FULL).as_deref(), Some("tensa"));
        assert_eq!(edition(CORE).as_deref(), Some("core"));
        assert_eq!(edition(MOCK).as_deref(), Some("standard"));
        let with =
            |value: &str| STANDARD.replace("edition = \"standard\"", &format!("edition = \"{value}\""));
        for bad in ["Tensa", "tensa lite", "", "tensa-2"] {
            assert!(parse_profile(&with(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn mock_profile_points_at_the_local_server() {
        let spec = parse_profile(MOCK).unwrap();
        assert!(spec.env.contains(&("LAUNCHER_UPDATE_API".into(), "http://127.0.0.1:1430".into())));
        assert!(spec.env.contains(&("LAUNCHER_UPDATE_REPO".into(), "mock/launcher".into())));
    }

    #[test]
    fn standard_profile_does_not_override_the_update_api() {
        assert!(!parse_profile(STANDARD).unwrap().env.iter().any(|(k, _)| k == "LAUNCHER_UPDATE_API"));
    }

    #[test]
    fn update_api_must_be_https_or_loopback() {
        let with_api =
            |api: &str| with_repo_line(STANDARD, &format!("update_repo = \"\"\nupdate_api = \"{api}\""));
        for bad in ["http://example.com", "ftp://127.0.0.1", "api.github.com", "https://"] {
            assert!(parse_profile(&with_api(bad)).is_err(), "{bad}");
        }
        for good in ["https://ghe.example.com/api/v3", "http://localhost:1430", "http://127.0.0.1:1430"] {
            assert!(parse_profile(&with_api(good)).is_ok(), "{good}");
        }
    }

    #[test]
    fn standard_profile_features_and_env() {
        let spec = parse_profile(STANDARD).unwrap();
        assert_eq!(spec.features_arg(), "mod-modrinth,mod-curseforge,mod-backups,mod-reports");
        assert!(spec.env.contains(&("LAUNCHER_PROFILE".into(), "standard".into())));
        let issues = spec.env.iter().find(|(k, _)| k == "LAUNCHER_ISSUES_URL").map(|(_, v)| v.as_str());
        assert!(issues.is_some_and(|url| url.starts_with("https://github.com/") && url.ends_with("/issues")));
        assert!(spec.env.contains(&(
            "LAUNCHER_MOD_REPORTS_ENDPOINT".into(),
            "https://gigabait.uk/api/mods/launcher/logs".into()
        )));
    }

    #[test]
    fn full_and_core_profiles() {
        assert!(parse_profile(FULL).unwrap().modules.contains(&"tensa".to_string()));
        let core = parse_profile(CORE).unwrap();
        assert!(core.modules.is_empty());
        assert_eq!(core.features_arg(), "");
    }

    #[test]
    fn rejects_unknown_duplicate_modules_and_bad_repo() {
        let unknown = STANDARD.replace("\"reports\"]", "\"reports\", \"forge\"]");
        assert!(parse_profile(&unknown).is_err());
        let dup = STANDARD.replace("\"reports\"]", "\"reports\", \"reports\"]");
        assert!(parse_profile(&dup).is_err());
        let bad_repo = with_repo_line(STANDARD, "update_repo = \"not-a-repo\"");
        assert!(parse_profile(&bad_repo).is_err());
        let good_repo = with_repo_line(STANDARD, "update_repo = \"gigabait/launcher\"");
        assert!(parse_profile(&good_repo).is_ok());
    }

    #[test]
    fn modules_override_keeps_branding() {
        let root = crate::cmd::root();
        let base = parse_profile(STANDARD).unwrap();
        let spec = with_modules(base, &root, "tensa, backups").unwrap();
        assert_eq!(spec.features_arg(), "mod-tensa,mod-backups");
        assert!(spec.env.iter().any(|(k, _)| k == "LAUNCHER_APP_NAME"));
        let none = with_modules(parse_profile(STANDARD).unwrap(), &root, "").unwrap();
        assert!(none.modules.is_empty());
        assert!(with_modules(parse_profile(STANDARD).unwrap(), &root, "nope").is_err());
    }

    #[test]
    fn modules_all_leaves_out_postponed_modules() {
        let root = crate::cmd::root();
        let all = with_modules(parse_profile(STANDARD).unwrap(), &root, "all").unwrap();
        assert!(!all.modules.contains(&"diagnostics".to_string()), "{:?}", all.modules);
        assert!(!all.features().contains(&"mod-diagnostics".to_string()));
        let named = with_modules(parse_profile(STANDARD).unwrap(), &root, "diagnostics, backups").unwrap();
        assert_eq!(named.modules, ["diagnostics", "backups"], "named, it is built in");
    }

    #[test]
    fn modules_all_expands_to_every_discovered_module() {
        let root = crate::cmd::root();
        let mut expected: Vec<String> =
            discover_modules(&root).unwrap().into_iter().filter(|m| m != "diagnostics").collect();
        expected.sort();
        let spec = with_modules(parse_profile(STANDARD).unwrap(), &root, "all").unwrap();
        assert_eq!(spec.modules, expected);
        // and it must still be a set validate_modules is happy with
        assert!(!spec.modules.is_empty());
    }

    #[test]
    fn modules_none_clears_the_list() {
        let root = crate::cmd::root();
        let spec = with_modules(parse_profile(STANDARD).unwrap(), &root, "none").unwrap();
        assert!(spec.modules.is_empty());
        assert_eq!(spec.features_arg(), "");
    }

    #[test]
    fn modules_unknown_still_errors_alongside_all_and_none() {
        let root = crate::cmd::root();
        assert!(with_modules(parse_profile(STANDARD).unwrap(), &root, "nope").is_err());
        assert!(with_modules(parse_profile(STANDARD).unwrap(), &root, "tensa, nope").is_err());
    }

    #[test]
    fn discovered_modules_match_known_modules() {
        // Keeps validate_modules's static allow-list in sync with what's actually on disk:
        // if this fails, a module directory was added/removed without updating KNOWN_MODULES.
        let root = crate::cmd::root();
        let mut discovered = discover_modules(&root).unwrap();
        discovered.sort();
        let mut known: Vec<String> = KNOWN_MODULES.iter().map(|s| s.to_string()).collect();
        known.sort();
        assert_eq!(discovered, known);
    }
}
