//! Modrinth's catalog rules: a build's search facets, the versions it can
//! use and the one file of a version that is safe to install.

use launcher_core::net::downloader::ExpectedHash;
use launcher_shared::ContentKind;
use reqwest::Url;
use serde_json::{Value, json};

use crate::types::{modrinth_url, project_type};
use launcher_shared::provider::{ProjectHit, SearchPage, loaders_run_by};

/// A trimmed string field, or empty.
pub(crate) fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string()
}

/// `[["project_type:<t>"],["categories:<loader>"],["versions:<mc>"]]` (the caller passes a loader
/// only for mods).
pub fn search_facets(kind: ContentKind, loader: Option<&str>, game_version: Option<&str>) -> String {
    let mut facets = vec![vec![format!("project_type:{}", project_type(kind))]];
    if let Some(loader) = loader {
        // One group: any of the loaders the build runs (Quilt runs Fabric's mods too).
        facets.push(run_by(loader).iter().map(|l| format!("categories:{l}")).collect());
    }
    if let Some(version) = game_version {
        facets.push(vec![format!("versions:{version}")]);
    }
    json!(facets).to_string()
}

fn hit(raw: &Value) -> Option<ProjectHit> {
    let project_id = text(raw, "project_id");
    if project_id.is_empty() {
        return None;
    }
    let slug = Some(text(raw, "slug")).filter(|s| !s.is_empty()).unwrap_or_else(|| project_id.clone());
    let title = Some(text(raw, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| slug.clone());
    Some(ProjectHit {
        author: text(raw, "author"),
        description: text(raw, "description"),
        downloads: raw.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        icon_url: Some(text(raw, "icon_url")).filter(|u| u.starts_with("https://")),
        url: Some(modrinth_url(&text(raw, "project_type"), &slug)),
        project_id,
        slug,
        title,
    })
}

/// One page of `/search`: hits without a project id are dropped, and the total is never below
/// what the page shows.
pub fn search_page(raw: &Value, offset: u32, limit: u32) -> SearchPage {
    let hits: Vec<ProjectHit> =
        raw.get("hits").and_then(Value::as_array).into_iter().flatten().filter_map(hit).collect();
    let reported = raw.get("total_hits").and_then(Value::as_u64).unwrap_or(0);
    let total = u32::try_from(reported).unwrap_or(u32::MAX).max(hits.len() as u32);
    SearchPage { hits, total, offset, limit }
}

/// The loaders a build of `loader` runs, its own first; an unknown one runs only its own.
pub(crate) fn run_by(loader: &str) -> Vec<&str> {
    match loaders_run_by(loader) {
        [] => vec![loader],
        known => known.to_vec(),
    }
}

/// `version` lists `wanted` under `key`.
fn lists(version: &Value, key: &str, wanted: &str) -> bool {
    version
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(|all| all.iter().any(|v| v.as_str() == Some(wanted)))
}

/// A version made for `game_version` and, when given, a loader the build of `loader` runs.
pub fn version_fits(version: &Value, loader: Option<&str>, game_version: Option<&str>) -> bool {
    game_version.is_none_or(|gv| lists(version, "game_versions", gv))
        && loader.is_none_or(|l| run_by(l).iter().any(|run| lists(version, "loaders", run)))
}

/// The versions a build can use: made for its Minecraft version and, when given, a loader it runs.
/// Newest first as Modrinth lists them; of a day's versions, those for the build's own loader come
/// first (a Quilt build takes a Quilt file over the Fabric one of the same release).
pub fn compatible<'a>(
    versions: &'a Value,
    loader: Option<&str>,
    game_version: Option<&str>,
) -> Vec<&'a Value> {
    let mut fit: Vec<&Value> =
        versions.as_array().into_iter().flatten().filter(|v| version_fits(v, loader, game_version)).collect();
    if let Some(own) = loader {
        let day = |v: &Value| {
            v.get("date_published").and_then(Value::as_str).and_then(|d| d.get(..10)).map(str::to_string)
        };
        // Stable: only versions of the same day change places.
        fit.sort_by(|a, b| {
            day(b).cmp(&day(a)).then_with(|| lists(b, "loaders", own).cmp(&lists(a, "loaders", own)))
        });
    }
    fit
}

/// A file to download and check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub hash: ExpectedHash,
}

/// HTTPS with a host and no user info; plain HTTP to this machine only in the module's own tests
/// (feature `test-servers`, which only its dev-dependencies turn on).
pub fn secure_url(raw: &str) -> bool {
    secure_url_with(raw, cfg!(feature = "test-servers"))
}

/// `secure_url`, taking plain HTTP to this machine only when `trust_loopback`.
fn secure_url_with(raw: &str, trust_loopback: bool) -> bool {
    let Ok(url) = Url::parse(raw) else { return false };
    let local = trust_loopback && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && local))
}

/// SHA-512 when it is a proper 128-digit hex, else SHA-1 (40 digits).
pub fn strongest_hash(hashes: &Value) -> Option<ExpectedHash> {
    let digest = |key: &str, len: usize| {
        hashes
            .get(key)
            .and_then(Value::as_str)
            .map(|d| d.trim().to_ascii_lowercase())
            .filter(|d| d.len() == len && d.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    digest("sha512", 128)
        .map(|d| ExpectedHash::sha512(&d))
        .or_else(|| digest("sha1", 40).map(|d| ExpectedHash::sha1(&d)))
}

fn install_file(file: &Value) -> Option<InstallFile> {
    let url = text(file, "url");
    let filename = text(file, "filename");
    if !secure_url(&url) || filename.is_empty() || filename.contains('\0') {
        return None;
    }
    let size = file.get("size").and_then(Value::as_u64).filter(|s| *s > 0)?;
    let hash = strongest_hash(file.get("hashes")?)?;
    Some(InstallFile { url, filename, size, hash })
}

/// The version's file to install: the primary one first, then the others in order — the first
/// with a safe address, a name, a size and a hash.
pub fn primary_file(version: &Value) -> Option<InstallFile> {
    let mut files: Vec<&Value> = version
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| f.is_object())
        .collect();
    files.sort_by_key(|f| !f.get("primary").and_then(Value::as_bool).unwrap_or(false));
    files.into_iter().find_map(install_file)
}

#[cfg(test)]
mod tests {
    #[test]
    fn pack_files_from_loopback_are_refused_outside_tests() {
        for local in ["http://127.0.0.1:9/a.jar", "http://localhost/a.jar", "http://[::1]:9/a.jar"] {
            assert!(!super::secure_url_with(local, false), "{local}");
            assert!(super::secure_url_with(local, true), "{local}");
        }
        assert!(super::secure_url_with("https://cdn.modrinth.com/a.jar", false));
        assert!(!super::secure_url_with("http://cdn.modrinth.com/a.jar", true));
    }

    use launcher_core::net::downloader::HashKind;
    use serde_json::json;

    use super::*;

    const SHA512: &str = "ab";

    fn sha512() -> String {
        SHA512.repeat(64)
    }

    fn file(url: &str, name: &str, size: serde_json::Value, primary: bool) -> serde_json::Value {
        json!({"url": url, "filename": name, "size": size, "primary": primary, "hashes": {"sha512": sha512(), "sha1": "c".repeat(40)}})
    }

    #[test]
    fn facets_narrow_by_type_loader_and_version() {
        assert_eq!(
            search_facets(ContentKind::Mods, Some("fabric"), Some("1.21.1")),
            r#"[["project_type:mod"],["categories:fabric"],["versions:1.21.1"]]"#
        );
        assert_eq!(
            search_facets(ContentKind::ResourcePacks, None, Some("1.21.1")),
            r#"[["project_type:resourcepack"],["versions:1.21.1"]]"#
        );
        assert_eq!(search_facets(ContentKind::ShaderPacks, None, None), r#"[["project_type:shader"]]"#);
    }

    #[test]
    fn a_quilt_build_searches_and_takes_fabric_mods_too() {
        // Quilt loads Fabric mods; most of them are tagged Fabric only.
        assert_eq!(
            search_facets(ContentKind::Mods, Some("quilt"), Some("1.21.1")),
            r#"[["project_type:mod"],["categories:quilt","categories:fabric"],["versions:1.21.1"]]"#
        );
        let versions = json!([
            {"id": "fabric-new", "game_versions": ["1.21.1"], "loaders": ["fabric"], "date_published": "2026-09-02T10:00:00Z"},
            {"id": "quilt-same-day", "game_versions": ["1.21.1"], "loaders": ["quilt"], "date_published": "2026-09-02T09:00:00Z"},
            {"id": "quilt-old", "game_versions": ["1.21.1"], "loaders": ["quilt"], "date_published": "2026-05-01T09:00:00Z"},
            {"id": "forge", "game_versions": ["1.21.1"], "loaders": ["forge"], "date_published": "2026-09-03T09:00:00Z"}
        ]);
        let ids: Vec<&str> = compatible(&versions, Some("quilt"), Some("1.21.1"))
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["quilt-same-day", "fabric-new", "quilt-old"],
            "its own loader first of a day's builds"
        );
        let fabric: Vec<&str> = compatible(&versions, Some("fabric"), Some("1.21.1"))
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(fabric, ["fabric-new"], "a Fabric build takes no Quilt mod");
    }

    #[test]
    fn odd_search_hits_still_list() {
        let raw = json!({"total_hits": 1, "hits": [
            {"project_id": "AANobbMI", "slug": "sodium", "title": "Sodium", "author": "jellysquid3",
             "description": " Fast ", "downloads": 61234567, "icon_url": "https://cdn.modrinth.com/i.png", "project_type": "mod"},
            {"project_id": "x1", "slug": null, "title": "", "author": null, "description": null,
             "downloads": 1.5, "icon_url": "http://cdn.example/i.png"},
            {"slug": "no-id", "title": "No id"}
        ]});
        let page = search_page(&raw, 16, 16);
        assert_eq!((page.offset, page.limit, page.total), (16, 16, 2), "never fewer than shown");
        assert_eq!(page.hits.len(), 2);
        let sodium = &page.hits[0];
        assert_eq!(
            (sodium.title.as_str(), sodium.description.as_str(), sodium.downloads),
            ("Sodium", "Fast", 61_234_567)
        );
        assert_eq!(sodium.icon_url.as_deref(), Some("https://cdn.modrinth.com/i.png"));
        assert_eq!(sodium.url.as_deref(), Some("https://modrinth.com/mod/sodium"));
        let odd = &page.hits[1];
        assert_eq!((odd.slug.as_str(), odd.title.as_str(), odd.author.as_str()), ("x1", "x1", ""));
        assert_eq!((odd.downloads, odd.icon_url.clone()), (0, None));
        assert_eq!(odd.url.as_deref(), Some("https://modrinth.com/mod/x1"), "no type: a mod's page");
        assert_eq!(
            search_page(&json!({"oops": true}), 0, 16),
            SearchPage { hits: vec![], total: 0, offset: 0, limit: 16 }
        );
    }

    #[test]
    fn compatible_versions_match_the_version_and_the_loader() {
        let versions = json!([
            {"id": "a", "game_versions": ["1.21.1"], "loaders": ["forge"]},
            {"id": "b", "game_versions": ["1.21", "1.21.1"], "loaders": ["fabric", "quilt"]},
            {"id": "c", "game_versions": ["1.20.1"], "loaders": ["fabric"]},
            {"id": "d"}
        ]);
        let ids = |loader, gv| {
            compatible(&versions, loader, gv).iter().map(|v| v["id"].as_str().unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(ids(Some("fabric"), Some("1.21.1")), ["b"]);
        assert_eq!(ids(None, Some("1.21.1")), ["a", "b"]);
        assert_eq!(ids(Some("fabric"), None), ["b", "c"]);
        assert_eq!(ids(None, None), ["a", "b", "c", "d"]);
        assert!(compatible(&json!({"error": "x"}), None, None).is_empty());
    }

    #[test]
    fn the_primary_file_wins_when_it_is_safe() {
        let version = json!({"files": [
            file("https://cdn.modrinth.com/a-sources.jar", "a-sources.jar", json!(10), false),
            file("https://cdn.modrinth.com/a.jar", "a.jar", json!(20), true)
        ]});
        let chosen = primary_file(&version).unwrap();
        assert_eq!((chosen.filename.as_str(), chosen.size), ("a.jar", 20));
        assert_eq!((chosen.hash.kind, chosen.hash.hex.clone()), (HashKind::Sha512, sha512()));
        let unsafe_primary = json!({"files": [
            file("http://cdn.example/a.jar", "a.jar", json!(20), true),
            file("https://cdn.modrinth.com/b.jar", "b.jar", json!(5), false)
        ]});
        assert_eq!(primary_file(&unsafe_primary).unwrap().filename, "b.jar");
    }

    #[test]
    fn files_without_a_safe_address_name_size_or_hash_are_skipped() {
        for bad in [
            file("https://user:pw@cdn.modrinth.com/a.jar", "a.jar", json!(1), true),
            file("ftp://cdn.modrinth.com/a.jar", "a.jar", json!(1), true),
            file("https://cdn.modrinth.com/a.jar", " ", json!(1), true),
            file("https://cdn.modrinth.com/a.jar", "a\0.jar", json!(1), true),
            file("https://cdn.modrinth.com/a.jar", "a.jar", json!(0), true),
            file("https://cdn.modrinth.com/a.jar", "a.jar", json!(true), true),
            file("https://cdn.modrinth.com/a.jar", "a.jar", json!(1.5), true),
            json!({"url": "https://cdn.modrinth.com/a.jar", "filename": "a.jar", "size": 1, "hashes": {"sha512": "zz"}}),
        ] {
            assert_eq!(primary_file(&json!({"files": [bad.clone()]})), None, "{bad}");
        }
        assert_eq!(primary_file(&json!({})), None);
        let sha1_only = json!({"files": [{"url": "http://127.0.0.1:9/a.jar", "filename": "a.jar", "size": 3,
            "hashes": {"sha512": "short", "sha1": "C".repeat(40)}}]});
        let chosen = primary_file(&sha1_only).unwrap();
        assert_eq!(
            (chosen.hash.kind, chosen.hash.hex.clone()),
            (HashKind::Sha1, "c".repeat(40)),
            "loopback serves the tests"
        );
    }
}
