//! What the "Create build" page offers: Minecraft releases, and snapshots when
//! the user asks for them, in the order of Mojang's manifest (newest first).

use launcher_shared::CatalogVersion;

use super::manifest::VersionManifest;

pub fn catalog_entries(manifest: &VersionManifest, snapshots: bool) -> Vec<CatalogVersion> {
    manifest
        .versions
        .iter()
        .filter(|v| v.kind == "release" || (snapshots && v.kind == "snapshot"))
        .map(|v| CatalogVersion {
            id: v.id.clone(),
            kind: v.kind.clone(),
            release_time: v.release_time.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> VersionManifest {
        let entry = |id: &str, kind: &str| json!({"id": id, "type": kind, "url": format!("https://x/{id}.json"), "releaseTime": "2024-08-08T12:24:45+00:00"});
        serde_json::from_value(json!({
            "latest": {"release": "1.21.1", "snapshot": "24w33a"},
            "versions": [entry("24w33a", "snapshot"), entry("1.21.1", "release"), entry("b1.7.3", "old_beta"),
                         entry("a1.0.4", "old_alpha"), entry("1.20.1", "release")]
        }))
        .unwrap()
    }

    #[test]
    fn releases_come_in_manifest_order_and_snapshots_on_request() {
        let ids =
            |snapshots| catalog_entries(&manifest(), snapshots).into_iter().map(|v| v.id).collect::<Vec<_>>();
        assert_eq!(ids(false), ["1.21.1", "1.20.1"]);
        assert_eq!(ids(true), ["24w33a", "1.21.1", "1.20.1"]);
        let first = &catalog_entries(&manifest(), true)[0];
        assert_eq!(
            (first.kind.as_str(), first.release_time.as_deref()),
            ("snapshot", Some("2024-08-08T12:24:45+00:00"))
        );
    }
}
