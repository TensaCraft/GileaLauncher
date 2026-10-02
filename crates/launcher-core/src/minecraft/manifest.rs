//! Mojang's version list and the addresses installs use.

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Deserialize;

use crate::net::meta::MetaClient;

/// Where Mojang metadata and files come from; tests point them at a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MojangEndpoints {
    pub version_manifest: String,
    /// `<resources>/<hash[..2]>/<hash>` — asset objects.
    pub resources: String,
    /// Maven repository for libraries that name no `url`.
    pub libraries: String,
    pub java_runtimes: String,
}

impl Default for MojangEndpoints {
    fn default() -> Self {
        MojangEndpoints {
            version_manifest: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into(),
            resources: "https://resources.download.minecraft.net".into(),
            libraries: "https://libraries.minecraft.net".into(),
            java_runtimes: "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json"
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub release_time: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Latest {
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default)]
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionManifest {
    #[serde(default)]
    pub latest: Latest,
    pub versions: Vec<ManifestVersion>,
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Option<&ManifestVersion> {
        self.versions.iter().find(|v| v.id == id)
    }
}

pub async fn fetch_manifest(meta: &MetaClient, endpoints: &MojangEndpoints) -> AppResult<VersionManifest> {
    let value = meta.get_json(&endpoints.version_manifest).await?;
    serde_json::from_value(value).map_err(|e| {
        AppError::new(ErrorCode::Network, format!("version manifest: {e}")).with_param("error", e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_manifest_parses_and_finds_versions() {
        let raw = json!({
            "latest": {"release": "1.21.1", "snapshot": "24w33a"},
            "versions": [
                {"id": "24w33a", "type": "snapshot", "url": "https://piston-meta.mojang.com/v1/packages/aa/24w33a.json",
                 "time": "2024-08-15T12:44:25+00:00", "releaseTime": "2024-08-15T12:39:34+00:00", "sha1": "aa", "complianceLevel": 1},
                {"id": "1.21.1", "type": "release", "url": "https://x.example/1.21.1.json", "sha1": "bb",
                 "releaseTime": "2024-08-08T12:24:45+00:00"}
            ]
        });
        let manifest: VersionManifest = serde_json::from_value(raw).unwrap();
        assert_eq!(manifest.latest.release.as_deref(), Some("1.21.1"));
        let found = manifest.find("1.21.1").unwrap();
        assert_eq!((found.kind.as_str(), found.sha1.as_deref()), ("release", Some("bb")));
        assert!(manifest.find("0.0-missing").is_none());
    }

    #[test]
    fn endpoints_point_at_mojang() {
        let e = MojangEndpoints::default();
        assert!(e.version_manifest.ends_with("/mc/game/version_manifest_v2.json"));
        assert_eq!(e.resources, "https://resources.download.minecraft.net");
        assert_eq!(e.libraries, "https://libraries.minecraft.net");
        assert!(e.java_runtimes.ends_with("/all.json"));
    }
}
