//! CurseForge's answers as the launcher uses them: search pages, the file that fits a build, and
//! what to download.

use launcher_core::net::downloader::ExpectedHash;
use launcher_shared::provider::{ProjectHit, SearchPage};
use reqwest::Url;
use serde_json::Value;

/// The API pages no further than its 10 000th result.
pub const SEARCH_CAP: u32 = 10_000;

/// A file to download and check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    /// Its address (none for the user's copy).
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub hash: ExpectedHash,
    pub from: Source,
}

/// Where a file's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// CurseForge's file servers, with the launcher's key.
    CurseForge,
    /// Another site with the same bytes (Modrinth).
    Elsewhere,
    /// The copy the user downloaded by hand.
    Copy(std::path::PathBuf),
}

pub fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A page of search results.
pub fn search_page(raw: &Value, offset: u32, limit: u32) -> SearchPage {
    let hits: Vec<ProjectHit> =
        raw.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(hit).collect();
    let reported = raw["pagination"]["totalCount"].as_u64().unwrap_or(0);
    let total = u32::try_from(reported).unwrap_or(u32::MAX).min(SEARCH_CAP).max(hits.len() as u32);
    SearchPage { hits, total, offset, limit }
}

fn hit(project: &Value) -> Option<ProjectHit> {
    let id = project.get("id").and_then(Value::as_u64)?;
    let https = |url: String| Some(url).filter(|u| u.starts_with("https://"));
    Some(ProjectHit {
        project_id: id.to_string(),
        slug: text(project, "slug"),
        title: text(project, "name"),
        author: project["authors"][0]["name"].as_str().unwrap_or_default().to_string(),
        description: text(project, "summary"),
        downloads: project.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        icon_url: https(text(&project["logo"], "thumbnailUrl")),
        url: https(text(&project["links"], "websiteUrl")),
    })
}

/// A file made for `game_version` and, when given, the loader CurseForge tags `loader_tag`.
pub fn file_fits(file: &Value, game_version: Option<&str>, loader_tag: Option<&str>) -> bool {
    let lists = |wanted: &str| {
        file.get("gameVersions")
            .and_then(Value::as_array)
            .is_some_and(|all| all.iter().any(|v| v.as_str() == Some(wanted)))
    };
    file.get("isAvailable").and_then(Value::as_bool) != Some(false)
        && game_version.is_none_or(lists)
        && loader_tag.is_none_or(lists)
}

/// The file to install of those that fit: the newest release, else the newest of any.
pub fn pick_file<'a>(
    files: &'a [Value],
    game_version: Option<&str>,
    loader_tag: Option<&str>,
) -> Option<&'a Value> {
    let fitting = || files.iter().filter(|f| file_fits(f, game_version, loader_tag));
    let newest = |a: &&Value, b: &&Value| text(a, "fileDate").cmp(&text(b, "fileDate"));
    fitting()
        .filter(|f| f["releaseType"].as_u64() == Some(1))
        .max_by(newest)
        .or_else(|| fitting().max_by(newest))
}

/// The author keeps the file from other apps: CurseForge gives no address for it.
pub fn blocked(file: &Value) -> bool {
    file.get("downloadUrl").and_then(Value::as_str).is_none_or(|url| url.trim().is_empty())
}

/// What to download for `file`: a safe address, a name, a size and its SHA-1; none when blocked
/// or incomplete.
pub fn install_file(file: &Value) -> Option<InstallFile> {
    let url = text(file, "downloadUrl");
    let filename = text(file, "fileName");
    if !secure_url(&url) || filename.is_empty() || filename.contains('\0') {
        return None;
    }
    let size = file.get("fileLength").and_then(Value::as_u64).filter(|s| *s > 0)?;
    let sha1 = file.get("hashes").and_then(Value::as_array)?.iter().find_map(|h| {
        let value = text(h, "value").trim().to_ascii_lowercase();
        (h["algo"].as_u64() == Some(1) && value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(value)
    })?;
    Some(InstallFile { url, filename, size, hash: ExpectedHash::sha1(&sha1), from: Source::CurseForge })
}

/// The file's page on curseforge.com (where a blocked file is downloaded by hand).
pub fn file_page(project_url: &str, file_id: u64) -> String {
    format!("{}/files/{file_id}", project_url.trim_end_matches('/'))
}

/// HTTPS with a host and no user info; plain HTTP to this machine only in the module's own tests.
pub fn secure_url(raw: &str) -> bool {
    secure_url_with(raw, cfg!(feature = "test-servers"))
}

fn secure_url_with(raw: &str, trust_loopback: bool) -> bool {
    let Ok(url) = Url::parse(raw) else { return false };
    let local = trust_loopback && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && local))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn file(id: u64, versions: &[&str], release: u64, date: &str, url: Value) -> Value {
        json!({
            "id": id, "fileName": format!("f{id}.jar"), "fileLength": 10, "releaseType": release,
            "fileDate": date, "gameVersions": versions, "downloadUrl": url,
            "hashes": [{"value": "00", "algo": 2}, {"value": "a".repeat(40), "algo": 1}]
        })
    }

    #[test]
    fn a_search_page_names_each_project() {
        let raw = json!({
            "data": [{
                "id": 394468, "name": "Sodium", "slug": "sodium", "summary": "Fast", "downloadCount": 9,
                "authors": [{"name": "JellySquid"}, {"name": "Other"}],
                "logo": {"thumbnailUrl": "https://media.forgecdn.net/a.png"},
                "links": {"websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/sodium"}
            }, {
                "id": 2, "name": "No logo", "slug": "nl", "authors": [], "logo": {"thumbnailUrl": "http://x/a.png"}
            }],
            "pagination": {"index": 16, "pageSize": 16, "resultCount": 2, "totalCount": 25000}
        });
        let page = search_page(&raw, 16, 16);
        assert_eq!((page.offset, page.limit, page.total), (16, 16, SEARCH_CAP), "the API stops at 10 000");
        let hit = &page.hits[0];
        assert_eq!(
            (hit.project_id.as_str(), hit.slug.as_str(), hit.title.as_str(), hit.author.as_str()),
            ("394468", "sodium", "Sodium", "JellySquid")
        );
        assert_eq!((hit.description.as_str(), hit.downloads), ("Fast", 9));
        assert_eq!(hit.icon_url.as_deref(), Some("https://media.forgecdn.net/a.png"));
        assert_eq!(hit.url.as_deref(), Some("https://www.curseforge.com/minecraft/mc-mods/sodium"));
        assert_eq!(
            (page.hits[1].icon_url.as_deref(), page.hits[1].author.as_str()),
            (None, ""),
            "https icons only"
        );
    }

    #[test]
    fn the_newest_release_that_fits_is_picked() {
        let files = vec![
            file(1, &["1.21.1", "Fabric"], 3, "2026-09-03T00:00:00Z", json!("https://edge.forgecdn.net/1")),
            file(2, &["1.21.1", "Fabric"], 1, "2026-09-02T00:00:00Z", json!("https://edge.forgecdn.net/2")),
            file(3, &["1.21.1", "Fabric"], 1, "2026-09-01T00:00:00Z", json!("https://edge.forgecdn.net/3")),
            file(4, &["1.21.1", "Forge"], 1, "2026-09-09T00:00:00Z", json!("https://edge.forgecdn.net/4")),
            file(5, &["1.20.1", "Fabric"], 1, "2026-09-09T00:00:00Z", json!("https://edge.forgecdn.net/5")),
        ];
        let pick = |v, l| pick_file(&files, v, l).map(|f| f["id"].as_u64().unwrap());
        assert_eq!(pick(Some("1.21.1"), Some("Fabric")), Some(2), "a release before a newer alpha");
        assert_eq!(pick(Some("1.21.1"), Some("Forge")), Some(4));
        assert_eq!(pick(Some("1.21.1"), Some("Quilt")), None);
        assert_eq!(pick(Some("1.21.1"), None), Some(4), "no loader: any file of the version");
        let alphas = vec![files[0].clone()];
        assert_eq!(
            pick_file(&alphas, Some("1.21.1"), Some("Fabric")).map(|f| f["id"].clone()),
            Some(json!(1))
        );
        assert!(file_fits(&files[0], Some("1.21.1"), Some("Fabric")));
        assert!(!file_fits(&files[0], Some("1.21"), Some("Fabric")), "a version is matched whole");
    }

    #[test]
    fn a_file_to_download_needs_a_safe_address_and_its_sha1() {
        let ready = file(7, &["1.21.1"], 1, "", json!("https://edge.forgecdn.net/files/7/f7.jar"));
        assert_eq!(
            install_file(&ready),
            Some(InstallFile {
                url: "https://edge.forgecdn.net/files/7/f7.jar".into(),
                filename: "f7.jar".into(),
                size: 10,
                hash: ExpectedHash::sha1(&"a".repeat(40)),
                from: Source::CurseForge,
            })
        );
        let held = file(8, &["1.21.1"], 1, "", Value::Null);
        assert!(blocked(&held) && install_file(&held).is_none());
        assert!(!blocked(&ready));
        let plain = file(9, &["1.21.1"], 1, "", json!("http://edge.forgecdn.net/f9.jar"));
        assert!(install_file(&plain).is_none(), "https only");
        let mut no_sha1 = ready.clone();
        no_sha1["hashes"] = json!([{"value": "00", "algo": 2}]);
        assert!(install_file(&no_sha1).is_none(), "nothing to check it by");
        assert_eq!(
            file_page("https://www.curseforge.com/minecraft/mc-mods/x/", 7),
            "https://www.curseforge.com/minecraft/mc-mods/x/files/7"
        );
    }
}
