//! Forge and NeoForge: their build lists, the Minecraft version each build is
//! for, and where their installers live. Installing runs the installer's own steps
//! (`installer`, `processors`, `forge_install`).

use std::collections::BTreeMap;
use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind};
use serde_json::Value;

use super::LoaderEndpoints;
use super::fabric::version_token_ok;
use super::versions::sort_key;
use crate::net::meta::MetaClient;

/// The oldest Minecraft with a Forge installer (earlier builds shipped only jars to patch in).
pub const FORGE_OLDEST_MINECRAFT: &str = "1.5.2";

/// `(minecraft, build)` of every `<version>` in Forge's `maven-metadata.xml`: the part before the
/// first `-` is the Minecraft version. Versions older than 1.5.2 and unusable names are left out.
pub fn forge_versions(xml: &str) -> Vec<(String, String)> {
    let oldest = sort_key(FORGE_OLDEST_MINECRAFT);
    xml.split("<version>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</version>").map(|(version, _)| version.trim()))
        .filter_map(|version| version.split_once('-'))
        .filter(|(mc, lv)| version_token_ok(mc) && version_token_ok(lv) && sort_key(mc) >= oldest)
        .map(|(mc, lv)| (mc.to_string(), lv.to_string()))
        .collect()
}

/// The Minecraft version a NeoForge build is for: `21.1.77` →
/// `1.21.1`, `21.0.5` → `1.21`; from 2025 on the first three numbers, a `.0` patch dropped
/// (`26.1.2.108` → `26.1.2`, `26.2.0.85` → `26.2`); a snapshot build names its snapshot after `+`
/// (`26.1.0.0-alpha.1+snapshot-1` → `26.1-snapshot-1`). `None` for anything else.
pub fn neoforge_minecraft(version: &str) -> Option<String> {
    let core = version.split(['-', '+']).next()?;
    let numbers: Vec<u64> = core.split('.').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    let base = match numbers[..] {
        [major @ 25..=u64::MAX, minor, 0, _] => format!("{major}.{minor}"),
        [major @ 25..=u64::MAX, minor, patch, _] => format!("{major}.{minor}.{patch}"),
        [major @ 20..=24, 0, _] => format!("1.{major}"),
        [major @ 20..=24, minor, _] => format!("1.{major}.{minor}"),
        _ => return None,
    };
    match version.split_once('+') {
        None => Some(base),
        Some((_, target)) if version_token_ok(target) => Some(format!("{base}-{target}")),
        Some(_) => None,
    }
}

/// The Maven coordinates of the installer of `kind` build `lv` for Minecraft `mc`.
pub fn installer_coords(kind: LoaderKind, mc: &str, lv: &str) -> Option<String> {
    match kind {
        LoaderKind::Forge => Some(format!("net.minecraftforge:forge:{mc}-{lv}:installer")),
        LoaderKind::NeoForge => Some(format!("net.neoforged:neoforge:{lv}:installer")),
        _ => None,
    }
}

/// Where that installer is downloaded from.
pub fn installer_url(kind: LoaderKind, endpoints: &LoaderEndpoints, mc: &str, lv: &str) -> Option<String> {
    match kind {
        LoaderKind::Forge => Some(format!(
            "{}/net/minecraftforge/forge/{mc}-{lv}/forge-{mc}-{lv}-installer.jar",
            endpoints.forge.trim_end_matches('/')
        )),
        LoaderKind::NeoForge => Some(format!(
            "{}/releases/net/neoforged/neoforge/{lv}/neoforge-{lv}-installer.jar",
            endpoints.neoforge.trim_end_matches('/')
        )),
        _ => None,
    }
}

/// One Minecraft version's builds as a loader lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameBuilds {
    pub builds: Vec<String>,
    /// Forge's recommended build for this version.
    pub recommended: Option<String>,
}

/// The build lists of Forge or NeoForge, grouped by Minecraft version.
pub struct ForgeMeta {
    kind: LoaderKind,
    endpoints: LoaderEndpoints,
    meta: Arc<MetaClient>,
}

impl ForgeMeta {
    pub fn new(kind: LoaderKind, endpoints: LoaderEndpoints, meta: Arc<MetaClient>) -> ForgeMeta {
        ForgeMeta { kind, endpoints, meta }
    }

    /// Every Minecraft version the loader has builds for.
    pub async fn games(&self) -> AppResult<BTreeMap<String, GameBuilds>> {
        match self.kind {
            LoaderKind::Forge => self.forge_games().await,
            LoaderKind::NeoForge => self.neoforge_games().await,
            other => Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("{} has no installer builds", other.display_name()),
            )),
        }
    }

    async fn forge_games(&self) -> AppResult<BTreeMap<String, GameBuilds>> {
        let base = self.endpoints.forge.trim_end_matches('/');
        let url = format!("{base}/net/minecraftforge/forge/maven-metadata.xml");
        // The builds and the recommendations live on two hosts: both are asked at once.
        let (xml, promotions) =
            tokio::join!(self.meta.get_bytes(&url), self.meta.get_json(&self.endpoints.forge_promotions));
        let xml = String::from_utf8_lossy(&xml?).into_owned();
        let mut games: BTreeMap<String, GameBuilds> = BTreeMap::new();
        for (mc, lv) in forge_versions(&xml) {
            games.entry(mc).or_default().builds.push(lv);
        }
        // Recommendations are a nicety: without them the newest build is the default.
        match promotions {
            Ok(promotions) => {
                for (mc, game) in games.iter_mut() {
                    game.recommended = promotions
                        .get("promos")
                        .and_then(|promos| promos.get(format!("{mc}-recommended")))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Err(e) => tracing::warn!("Forge recommendations are unavailable: {}", e.detail),
        }
        Ok(games)
    }

    async fn neoforge_games(&self) -> AppResult<BTreeMap<String, GameBuilds>> {
        let base = self.endpoints.neoforge.trim_end_matches('/');
        let url = format!("{base}/api/maven/versions/releases/net/neoforged/neoforge");
        let list = self.meta.get_json(&url).await?;
        let Some(versions) = list.get("versions").and_then(Value::as_array) else {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("{url}: no version list"))
                .with_param("error", "no version list"));
        };
        let mut games: BTreeMap<String, GameBuilds> = BTreeMap::new();
        for lv in versions.iter().filter_map(Value::as_str).filter(|lv| version_token_ok(lv)) {
            if let Some(mc) = neoforge_minecraft(lv) {
                games.entry(mc).or_default().builds.push(lv.to_string());
            }
        }
        Ok(games)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_versions_split_at_the_first_dash() {
        let xml = "<metadata><versioning><latest>1.21.1-52.1.16</latest><versions>\
            <version>1.21.1-52.1.16</version><version>1.7.10-10.13.4.1614-1.7.10</version>\
            <version>1.5.2-7.8.1.738</version><version>1.4.7-6.6.2.534</version>\
            <version>1.20.1-47.4.10</version><version>bad version-1</version></versions></versioning></metadata>";
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
        };
        assert_eq!(
            forge_versions(xml),
            pairs(&[
                ("1.21.1", "52.1.16"),
                ("1.7.10", "10.13.4.1614-1.7.10"),
                ("1.5.2", "7.8.1.738"),
                ("1.20.1", "47.4.10"),
            ]),
            "1.4.7 has no installer; a name with a space is not a version"
        );
    }

    #[test]
    fn neoforge_builds_name_their_minecraft() {
        for (lv, mc) in [
            ("20.2.3-beta", "1.20.2"),
            ("21.0.5", "1.21"),
            ("21.1.77", "1.21.1"),
            ("21.11.42", "1.21.11"),
            ("26.1.2.108", "26.1.2"),
            ("26.2.0.85", "26.2"),
            ("26.1.0.0-alpha.1+snapshot-1", "26.1-snapshot-1"),
            ("26.1.0.0-alpha.15+pre-3", "26.1-pre-3"),
        ] {
            assert_eq!(neoforge_minecraft(lv).as_deref(), Some(mc), "{lv}");
        }
        for lv in ["0.25w14craftmine.3-beta", "21", "19.1.2", "26.1.2", "x.y.z"] {
            assert_eq!(neoforge_minecraft(lv), None, "{lv}");
        }
    }

    #[test]
    fn installers_live_on_each_loader_maven() {
        let e = LoaderEndpoints::default();
        assert_eq!(
            installer_url(LoaderKind::Forge, &e, "1.20.1", "47.4.10").as_deref(),
            Some(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.4.10/forge-1.20.1-47.4.10-installer.jar"
            )
        );
        assert_eq!(
            installer_url(LoaderKind::NeoForge, &e, "1.21.1", "21.1.77").as_deref(),
            Some(
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.77/neoforge-21.1.77-installer.jar"
            )
        );
        assert_eq!(
            installer_coords(LoaderKind::Forge, "1.20.1", "47.4.10").as_deref(),
            Some("net.minecraftforge:forge:1.20.1-47.4.10:installer")
        );
        assert_eq!(
            installer_coords(LoaderKind::NeoForge, "1.21.1", "21.1.77").as_deref(),
            Some("net.neoforged:neoforge:21.1.77:installer")
        );
        assert_eq!(installer_url(LoaderKind::Fabric, &e, "1.21.1", "0.16.9"), None);
        assert_eq!(
            e.forge_promotions,
            "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json"
        );
    }
}
