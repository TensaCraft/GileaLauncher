//! The server's whitelist: the mods it allows a player to add to a server build
//! (`{api_base}/{pack}/whitelist`), as the launcher keeps them.

use launcher_shared::ContentKind;
use serde_json::Value;

use super::manifest::safe_relative;

/// A mod the server allows: where it goes, where it comes from and what it must hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The server's id of the entry, the same across its versions.
    pub id: String,
    pub title: String,
    pub kind: ContentKind,
    /// The file's name.
    pub name: String,
    /// `mods/<name>`.
    pub relative: String,
    pub url: String,
    pub size: Option<u64>,
    /// Lower-case hex.
    pub sha256: String,
    /// Its page on Modrinth or CurseForge, when the server names its project there.
    pub page: Option<String>,
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The page the item's catalog entry names.
fn page(catalog: Option<&Value>) -> Option<String> {
    let catalog = catalog?;
    let project = text(catalog.get("project_id"))?;
    match text(catalog.get("provider"))?.to_lowercase().as_str() {
        "modrinth" => Some(format!("https://modrinth.com/mod/{project}")),
        "curseforge" => Some(format!("https://www.curseforge.com/projects/{project}")),
        _ => None,
    }
}

/// One entry of the answer; `None` for one the launcher cannot use (not a mod, no safe
/// `mods/<name>` path, no address, no sha256).
fn item(entry: &Value) -> Option<Item> {
    let kind = match text(entry.get("type"))?.to_lowercase().as_str() {
        "mod" => ContentKind::Mods,
        _ => return None,
    };
    let relative = safe_relative(&text(entry.get("relative_path"))?)?;
    let name = relative.strip_prefix("mods/").filter(|n| !n.is_empty() && !n.contains('/'))?.to_string();
    let sha256 = text(entry.get("sha256"))
        .or_else(|| text(entry.get("hashes").and_then(|h| h.get("sha256"))))?
        .to_lowercase();
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(Item {
        id: text(entry.get("id"))?,
        title: text(entry.get("title")).unwrap_or_else(|| name.clone()),
        kind,
        url: text(entry.get("download_url"))?,
        size: entry.get("size").and_then(Value::as_u64),
        sha256,
        page: page(entry.get("catalog")),
        name,
        relative,
    })
}

/// The items of a whitelist answer the launcher can use, in its order.
pub fn items(answer: &Value) -> Vec<Item> {
    let entries = answer.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            let kept = item(entry);
            if kept.is_none() {
                tracing::warn!("A whitelist entry the launcher cannot use: {entry}");
            }
            kept
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const HASH: &str = "55c1a0ba3cb0f5cc0e33494f44c2d46d89291de730241cff370f3a4f27c8e638";

    fn one(item: Value) -> Vec<Item> {
        items(&json!({"modpack": {"id": 1, "slug": "aero"}, "items": [item]}))
    }

    fn good() -> Value {
        json!({
            "id": "i1", "title": "Goggles", "type": "mod", "name": "goggles-1.jar", "path": "mods/",
            "relative_path": "mods/goggles-1.jar", "download_url": "https://cdn.example/g.jar", "size": 3,
            "sha256": HASH, "catalog": {"type": "mod", "provider": null, "project_id": null}
        })
    }

    fn with(key: &str, value: Value) -> Value {
        let mut item = good();
        item[key] = value;
        item
    }

    #[test]
    fn the_live_whitelist_reads_as_its_fourteen_mods() {
        let raw: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/whitelist-aeronautics_dev.json"))
                .unwrap();
        let all = items(&raw);
        assert_eq!(all.len(), 14);
        assert_eq!(
            (all[0].title.as_str(), all[0].relative.as_str()),
            ("Create: Cyber Goggles", "mods/CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar")
        );
        assert_eq!((all[0].size, all[0].kind, all[0].page.clone()), (Some(874811), ContentKind::Mods, None));
        assert_eq!(all[0].sha256, HASH);
        assert_eq!(all[0].id, "8c05e2ff-c986-46a4-9b9e-330b276c66b8");
        assert!(all[0].url.starts_with("https://cdn.modrinth.com/"));
    }

    #[test]
    fn items_without_a_safe_mods_path_an_address_or_a_hash_are_dropped() {
        assert_eq!(one(good()).len(), 1);
        for bad in [
            with("relative_path", json!("../x.jar")),
            with("relative_path", json!("config/a.jar")),
            with("relative_path", json!("mods/sub/a.jar")),
            with("relative_path", json!("mods/")),
            with("download_url", json!(" ")),
            with("download_url", Value::Null),
            with("sha256", json!("zz")),
            with("sha256", Value::Null),
            with("type", json!("resourcepack")),
            with("id", json!("")),
        ] {
            assert!(one(bad.clone()).is_empty(), "{bad}");
        }
        let untitled = one(with("title", Value::Null)).remove(0);
        assert_eq!(untitled.title, "goggles-1.jar", "titled by its file");
        let upper = one(with("sha256", json!(HASH.to_uppercase()))).remove(0);
        assert_eq!(upper.sha256, HASH, "a hash is kept in lower case");
        assert!(items(&json!({"items": "no"})).is_empty() && items(&json!([1])).is_empty());
    }

    #[test]
    fn a_catalog_entry_names_the_mod_page() {
        let page = |provider: &str, project: Value| {
            one(with("catalog", json!({"type": "mod", "provider": provider, "project_id": project})))
                .remove(0)
                .page
        };
        assert_eq!(page("modrinth", json!("P")).as_deref(), Some("https://modrinth.com/mod/P"));
        assert_eq!(page("curseforge", json!(7)).as_deref(), Some("https://www.curseforge.com/projects/7"));
        assert_eq!(page("other", json!("P")), None);
        assert_eq!(page("modrinth", Value::Null), None);
    }
}
