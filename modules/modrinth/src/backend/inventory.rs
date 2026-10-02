//! The files of one kind in a build and what is known of their Modrinth origin:
//! the provenance's claims, checked against each file's hash, and Modrinth's own answer for each
//! file's SHA-512 — so a mod put there by hand is known as well as one the launcher installed.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use launcher_core::net::downloader::HashKind;
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode};
use serde_json::Value;

use super::api::ModrinthApi;
use super::catalog::text;
use super::provenance::{DigestCache, read, record_hash};

/// A file's size and modification time: one that changes while it is identified fails the plan.
type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    fs::symlink_metadata(path).ok().filter(|m| m.is_file()).map(|m| (m.len(), m.modified().ok()))
}

fn field(value: &Value, key: &str) -> Option<String> {
    Some(text(value, key)).filter(|s| !s.is_empty())
}

/// One file of the kind's folder.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledItem {
    /// `mods/x.jar` (or `mods/x.jar.disabled`).
    pub relative: String,
    /// Its name without `.disabled`.
    pub filename: String,
    pub enabled: bool,
    pub sha512: Option<String>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub version_number: Option<String>,
    /// The provenance's project slug and title (gone when Modrinth names another project).
    pub slug: Option<String>,
    pub title: Option<String>,
    /// The file is known to be the project's: its provenance hash matches, or Modrinth named it.
    pub owned: bool,
    /// The launcher installed it from Modrinth: its provenance hash matches.
    pub recorded: bool,
    /// The mod id and name its jar declares (mods only).
    pub mod_id: Option<String>,
    pub mod_name: Option<String>,
    /// The version Modrinth answered with when identifying the file.
    pub version: Option<Value>,
    /// The provenance's SHA-512 matches (kept when Modrinth does not know the file).
    sha512_claim: bool,
    stamp: Stamp,
}

/// Modrinth's answers by SHA-512 for the session: a file's bytes name one version for good.
#[derive(Default)]
pub struct Identities(Mutex<HashMap<String, Value>>);

impl Identities {
    fn missing(&self, hashes: &[String]) -> Vec<String> {
        let known = self.0.lock().unwrap_or_else(|e| e.into_inner());
        hashes.iter().filter(|h| !known.contains_key(*h)).cloned().collect()
    }

    fn learn(&self, answers: serde_json::Map<String, Value>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend(answers);
    }

    fn get(&self, hash: &str) -> Option<Value> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(hash).cloned()
    }
}

/// What an inventory says about a project.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Found<'a> {
    None,
    Owned(&'a InstalledItem),
    /// A file no project is known to own that looks like this one's (the original's HINT): an
    /// unverified provenance, or the jar's mod id or name is the project's slug or title. Its
    /// version is unknown.
    Hinted(&'a InstalledItem),
    /// More than one file is the project's: none can be safely replaced.
    Ambiguous,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    pub items: Vec<InstalledItem>,
}

fn extension(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => ".jar",
        ContentKind::ResourcePacks | ContentKind::ShaderPacks => ".zip",
    }
}

/// The files of `kind` in build folder `game` — `.jar` for mods, `.zip` for packs, switched off
/// with `.disabled`; no folders or links — with what the provenance claims of each.
pub fn scan(game: &Path, kind: ContentKind, cache: &DigestCache) -> Inventory {
    let document = read(game);
    let records = document.as_ref().and_then(|d| d.get("files")).and_then(Value::as_object);
    let ext = extension(kind);
    let Ok(entries) = fs::read_dir(game.join(kind.folder())) else { return Inventory::default() };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let enabled = lower.ends_with(ext);
        if !enabled && !lower.ends_with(&format!("{ext}.disabled")) {
            continue;
        }
        let path = entry.path();
        let Some(stamp) = stamp(&path) else { continue };
        let filename =
            if enabled { name.clone() } else { name[..name.len() - ".disabled".len()].to_string() };
        let relative = format!("{}/{name}", kind.folder());
        let base = format!("{}/{filename}", kind.folder());
        let sha512 = cache.digest(&path, "sha512", HashKind::Sha512);
        let (mod_id, mod_name) = match kind {
            ContentKind::Mods => cache.mod_meta(&path).map_or((None, None), |(id, name)| (Some(id), name)),
            _ => (None, None),
        };
        let mut item = InstalledItem {
            relative: relative.clone(),
            filename,
            enabled,
            sha512,
            project_id: None,
            version_id: None,
            version_number: None,
            slug: None,
            title: None,
            owned: false,
            recorded: false,
            mod_id,
            mod_name,
            version: None,
            sha512_claim: false,
            stamp,
        };
        if let Some(record) = records.and_then(|r| r.get(&relative).or_else(|| r.get(&base))) {
            item.project_id = field(record, "project_id");
            item.version_id = field(record, "version_id");
            item.version_number = field(record, "version_number");
            item.slug = field(record, "project_slug");
            item.title = field(record, "project_title");
            if let Some((algorithm, hash, expected)) = record_hash(record) {
                let actual = if algorithm == "sha512" {
                    item.sha512.clone()
                } else {
                    cache.digest(&path, algorithm, hash)
                };
                item.owned = item.project_id.is_some() && actual.as_deref() == Some(expected.as_str());
                item.recorded = item.owned;
                item.sha512_claim = item.owned && algorithm == "sha512";
            }
        }
        items.push(item);
    }
    items.sort_by(|a, b| a.relative.cmp(&b.relative));
    Inventory { items }
}

/// Letters and digits in lower case: how names compare for a hint ("Fabric API" = "fabric-api").
pub fn normalized(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

impl InstalledItem {
    /// A file no project is known to own that looks like the project's: its unverified provenance
    /// names it, or its mod id is the project's slug, or its mod name the project's title. (Never
    /// the file name: `sodium-extra.jar` is not Sodium.)
    fn hints(&self, project_id: &str, slug: &str, title: &str) -> bool {
        if self.owned {
            return false;
        }
        let same = |declared: Option<&str>, name: &str| {
            let name = normalized(name);
            !name.is_empty() && declared.is_some_and(|d| normalized(d) == name)
        };
        self.project_id.as_deref() == Some(project_id)
            || same(self.mod_id.as_deref(), slug)
            || same(self.mod_name.as_deref(), title)
    }
}

impl Inventory {
    /// Asks Modrinth which version each file is by its SHA-512 (`POST /version_files`): a file it
    /// knows becomes that version's project's; one it does not keeps only a SHA-512 provenance
    /// claim. A file that changed since the scan fails it all.
    pub async fn identify(&mut self, api: &ModrinthApi, game: &Path, known: &Identities) -> AppResult<()> {
        let hashes: Vec<String> = self.items.iter().filter_map(|i| i.sha512.clone()).collect();
        if hashes.is_empty() {
            return Ok(());
        }
        let missing = known.missing(&hashes);
        if !missing.is_empty() {
            known.learn(api.versions_by_hashes(&missing).await?);
        }
        for item in &mut self.items {
            if stamp(&game.join(&item.relative)) != Some(item.stamp) {
                return Err(AppError::new(
                    ErrorCode::Io,
                    format!("{} changed while it was identified", item.relative),
                )
                .with_param("name", item.filename.clone()));
            }
            let Some(digest) = item.sha512.clone() else {
                item.owned = false;
                continue;
            };
            let Some(version) = known.get(&digest) else {
                item.owned = item.sha512_claim;
                continue;
            };
            let same_file = version.get("files").and_then(Value::as_array).into_iter().flatten().any(|f| {
                f.get("hashes").and_then(|h| h.get("sha512")).and_then(Value::as_str) == Some(digest.as_str())
                    && f.get("size").and_then(Value::as_u64) == Some(item.stamp.0)
            });
            let (project, id) = (text(&version, "project_id"), text(&version, "id"));
            if !same_file || project.is_empty() || id.is_empty() {
                return Err(AppError::new(
                    ErrorCode::Network,
                    format!("Modrinth named {} with another file", item.relative),
                )
                .with_param("name", item.filename.clone()));
            }
            if item.project_id.as_deref() != Some(project.as_str()) {
                (item.slug, item.title) = (None, None);
            }
            item.project_id = Some(project);
            item.version_id = Some(id);
            item.version_number = field(&version, "version_number");
            item.owned = true;
            item.version = Some(version);
        }
        Ok(())
    }

    /// The project's copy here — `slug` and `title` also find the files no project is known to
    /// own — its one enabled copy, else its one copy; several are ambiguous.
    pub fn find(&self, project_id: &str, slug: &str, title: &str) -> Found<'_> {
        let copies = self.copies(project_id, slug, title);
        let enabled: Vec<&InstalledItem> = copies.iter().copied().filter(|i| i.enabled).collect();
        let one = match (enabled.as_slice(), copies.as_slice()) {
            ([one], _) => *one,
            (_, []) => return Found::None,
            (_, [one]) => *one,
            _ => return Found::Ambiguous,
        };
        if one.owned { Found::Owned(one) } else { Found::Hinted(one) }
    }

    /// Every file here that is the project's or looks like it ([`Self::find`]'s candidates).
    pub fn copies(&self, project_id: &str, slug: &str, title: &str) -> Vec<&InstalledItem> {
        self.items
            .iter()
            .filter(|i| {
                (i.owned && i.project_id.as_deref() == Some(project_id)) || i.hints(project_id, slug, title)
            })
            .collect()
    }

    /// The projects that own a file here.
    pub fn owned_projects(&self) -> Vec<String> {
        let owned: BTreeSet<String> =
            self.items.iter().filter(|i| i.owned).filter_map(|i| i.project_id.clone()).collect();
        owned.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use launcher_core::net::downloader::{HashKind, hash_file};
    use serde_json::json;

    use super::*;
    use crate::backend::provenance::DigestCache;

    #[test]
    fn a_build_s_files_carry_the_claims_their_hashes_bear_out() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path();
        let put = |relative: &str, body: &[u8]| {
            let path = game.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        };
        put("mods/a.jar", b"a");
        put("mods/b.jar.disabled", b"b");
        put("mods/c.jar", b"c");
        put("mods/notes.txt", b"n");
        fs::create_dir_all(game.join("mods/folder.jar")).unwrap();
        let hash = |relative: &str, kind| hash_file(&game.join(relative), kind).unwrap();
        let doc = json!({"schema_version": 2, "files": {
            "mods/a.jar": {"project_id": "pa", "version_id": "va", "version_number": "1.0",
                           "hash_algorithm": "sha512", "file_hash": hash("mods/a.jar", HashKind::Sha512)},
            "mods/b.jar": {"project_id": "pb", "hash_algorithm": "sha1", "file_hash": hash("mods/b.jar.disabled", HashKind::Sha1)},
            "mods/c.jar": {"project_id": "pc", "hash_algorithm": "sha512", "file_hash": "00".repeat(64)}}});
        put(".launcher/modrinth-content.json", doc.to_string().as_bytes());
        let cache = DigestCache::default();
        let inventory = scan(game, ContentKind::Mods, &cache);
        let files: Vec<(&str, bool, bool)> =
            inventory.items.iter().map(|i| (i.relative.as_str(), i.enabled, i.owned)).collect();
        assert_eq!(
            files,
            [("mods/a.jar", true, true), ("mods/b.jar.disabled", false, true), ("mods/c.jar", true, false)]
        );
        assert_eq!(inventory.items[1].filename, "b.jar");
        assert_eq!(inventory.items[0].version_id.as_deref(), Some("va"));
        assert!(inventory.items.iter().all(|i| i.sha512.as_deref().is_some_and(|d| d.len() == 128)));
        assert!(matches!(inventory.find("pa", "", ""), Found::Owned(item) if item.relative == "mods/a.jar"));
        assert!(matches!(inventory.find("pb", "", ""), Found::Owned(_)));
        // A claim its hash does not bear out is only a hint: the file may be the project's.
        assert!(matches!(inventory.find("pc", "", ""), Found::Hinted(item) if item.relative == "mods/c.jar"));
        assert_eq!(inventory.find("pd", "", ""), Found::None);
        assert_eq!(inventory.owned_projects(), ["pa", "pb"]);
        // A second file of the same project: the enabled one wins while it is the only enabled one.
        put("mods/a2.jar.disabled", b"a");
        let mut doc = doc;
        doc["files"]["mods/a2.jar"] = json!({"project_id": "pa", "hash_algorithm": "sha512", "file_hash": hash("mods/a.jar", HashKind::Sha512)});
        put(".launcher/modrinth-content.json", doc.to_string().as_bytes());
        assert!(
            matches!(scan(game, ContentKind::Mods, &cache).find("pa", "", ""), Found::Owned(item) if item.enabled)
        );
        fs::rename(game.join("mods/a2.jar.disabled"), game.join("mods/a2.jar")).unwrap();
        assert_eq!(scan(game, ContentKind::Mods, &cache).find("pa", "", ""), Found::Ambiguous);
        assert!(scan(game, ContentKind::ResourcePacks, &cache).items.is_empty());
    }
}
