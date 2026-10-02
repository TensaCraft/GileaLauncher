//! CurseForge's modpacks through the core's pack flow: a page of packs, a pack's versions, and a
//! version as the flow needs it — its zip downloaded and its files as CurseForge names them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use launcher_core::feedback::OperationHandle;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, HashKind, hash_file};
use launcher_core::packs::Limits;
use launcher_core::packs::engine::{PackKind, PackVersionMeta};
use launcher_core::packs::flow::{self, Fetched, PackFuture, PackSource};
use launcher_shared::provider::{
    ModpackBuild, PACKS_LIMIT, PackArgs, PackInstallArgs, PackInstalled, PackUpdateArgs, PackUpdated,
    PackVersion, PacksArgs, SearchPage, held_error,
};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode};
use serde_json::Value;

use super::{CurseForgeService, blocking, id_of};
use crate::backend::api::SearchQuery;
use crate::backend::catalog::{InstallFile, SEARCH_CAP, Source, install_file, search_page, text};
use crate::backend::held::Held;
use crate::backend::pack::{self, PACK_RECORD, PackHeld};
use crate::backend::provenance::{installed, read};
use crate::backend::resolver::version_label;
use crate::types::MODPACKS_CLASS;

/// The journal of a CurseForge pack's update.
pub const PACK_JOURNAL: &str = ".launcher-curseforge-pack-sync.json";
/// Where a pack's zip is downloaded to while it is read.
const PACK_ARCHIVE: &str = ".launcher-curseforge-pack.zip";

/// CurseForge's modpack builds, as the core's pack engine tells them apart.
pub const PACKS: PackKind = PackKind {
    provider: "CurseForge",
    record: PACK_RECORD,
    journal: PACK_JOURNAL,
    archive: PACK_ARCHIVE,
    lease: "curseforge_pack_install",
    install_operation: "curseforge-pack-install",
    update_operation: "curseforge-pack-update",
    commit_prefix: "curseforge-pack",
    project_option: "curseforgeProjectId",
    version_option: "curseforgeVersionId",
};

/// The loaders CurseForge lists among a file's game versions, and the names builds give them.
const LOADERS: [(&str, &str); 4] =
    [("Forge", "forge"), ("NeoForge", "neoforge"), ("Fabric", "fabric"), ("Quilt", "quilt")];

/// A pack's file the launcher can install: the client's zip, with an address and a SHA-1.
fn installable(file: &Value) -> bool {
    file["isServerPack"].as_bool() != Some(true)
        && file["isAvailable"].as_bool() != Some(false)
        && install_file(file).is_some()
}

/// Pack file `file` of pack `name` as a version to choose.
fn version_of(name: &str, file: &Value) -> PackVersion {
    let tags: Vec<&str> =
        file["gameVersions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    PackVersion {
        id: file["id"].as_u64().unwrap_or_default().to_string(),
        version_number: version_label(name, &text(file, "displayName")),
        game_versions: tags
            .iter()
            .filter(|t| t.starts_with(|c: char| c.is_ascii_digit()))
            .map(|t| t.to_string())
            .collect(),
        loaders: tags
            .iter()
            .filter_map(|t| LOADERS.iter().find(|(tag, _)| tag.eq_ignore_ascii_case(t)))
            .map(|(_, loader)| loader.to_string())
            .collect(),
    }
}

/// The held files `game` has already, as the pack puts them (the build's own copies, by size and
/// SHA-1, or switched off by the player): an update needs no copy of them.
fn already_there(game: &Path, held: &[PackHeld]) -> HashMap<String, InstallFile> {
    held.iter()
        .filter_map(|h| {
            let there = game.join(&h.path);
            let off = game.join(format!("{}.disabled", h.path));
            let same = |path: &Path| {
                std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == h.held.size)
                    && hash_file(path, HashKind::Sha1).is_ok_and(|s| s.eq_ignore_ascii_case(&h.held.sha1))
            };
            let copy = [there, off].into_iter().find(|path| same(path))?;
            Some((h.held.sha1.clone(), h.held.install_file(Source::Copy(copy))))
        })
        .collect()
}

/// The answers of a list call by their `id`.
fn by_id(items: Vec<Value>) -> HashMap<u64, Value> {
    items.into_iter().filter_map(|item| Some((item["id"].as_u64()?, item))).collect()
}

impl CurseForgeService {
    /// A page of CurseForge's modpacks.
    pub async fn packs(&self, args: &PacksArgs) -> AppResult<SearchPage> {
        if args.offset >= SEARCH_CAP {
            return Ok(SearchPage {
                hits: Vec::new(),
                total: SEARCH_CAP,
                offset: args.offset,
                limit: PACKS_LIMIT,
            });
        }
        let query = SearchQuery {
            class_id: MODPACKS_CLASS,
            text: &args.query,
            game_version: None,
            loader: None,
            index: args.offset,
            page_size: PACKS_LIMIT.min(SEARCH_CAP - args.offset),
        };
        Ok(search_page(&self.api.search(&query).await?, args.offset, PACKS_LIMIT))
    }

    /// The versions of a modpack the launcher can install (not its server packs), newest first.
    pub async fn pack_versions(&self, args: &PackArgs) -> AppResult<Vec<PackVersion>> {
        let id = id_of(&args.project_id)?;
        let (project, files) = tokio::join!(self.api.mod_info(id), self.api.files(id, None, None));
        let (project, mut files) = (project?, files?);
        if project["classId"].as_u64() != Some(u64::from(MODPACKS_CLASS)) {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("{id} is not a modpack")));
        }
        files.retain(installable);
        files.sort_by(|a, b| {
            text(b, "fileDate").cmp(&text(a, "fileDate")).then(b["id"].as_u64().cmp(&a["id"].as_u64()))
        });
        let name = text(&project, "name");
        Ok(files.iter().map(|f| version_of(&name, f)).collect())
    }

    /// A new build from a modpack version (the core's pack flow).
    pub async fn install_pack(&self, args: &PackInstallArgs) -> AppResult<PackInstalled> {
        flow::install_pack(&self.packs, &PACKS, self, args).await
    }

    /// The builds this module installed from modpacks, each with its pack's newest version when
    /// that is another.
    pub async fn modpack_builds(&self) -> AppResult<Vec<ModpackBuild>> {
        flow::pack_builds(&self.packs, &PACKS, self).await
    }

    /// Updates build `args.key`, installed from a CurseForge modpack, to another version of it.
    pub async fn update_pack(&self, args: &PackUpdateArgs) -> AppResult<PackUpdated> {
        flow::update_pack(&self.packs, &PACKS, self, args).await
    }

    /// Pack file `version_id` of pack `project_id` for the build folder `game`: its zip downloaded
    /// to `archive` and read, its files named by CurseForge.
    async fn fetch_pack(
        &self,
        project_id: &str,
        version_id: &str,
        game: &Path,
        archive: &Path,
        op: &OperationHandle,
    ) -> AppResult<Fetched> {
        let (project, file_id) = (id_of(project_id)?, id_of(version_id)?);
        let wanted = [file_id];
        let (info, found) = tokio::join!(self.api.mod_info(project), self.api.files_by_ids(&wanted));
        let (info, found) = (info?, found?);
        let file = found.into_iter().find(|f| f["id"].as_u64() == Some(file_id)).ok_or_else(|| {
            AppError::new(ErrorCode::NoFileFound, format!("CurseForge has no file {file_id}"))
                .with_param("name", text(&info, "name"))
        })?;
        if file["modId"].as_u64() != Some(project)
            || info["classId"].as_u64() != Some(u64::from(MODPACKS_CLASS))
        {
            return Err(AppError::new(ErrorCode::InvalidInput, "the version is another project's"));
        }
        let download = install_file(&file).filter(|_| installable(&file)).ok_or_else(|| {
            AppError::new(ErrorCode::NoFileFound, "the pack's zip cannot be downloaded")
                .with_param("name", text(&info, "name"))
        })?;
        let task = DownloadTask::new(download.url, archive.to_path_buf())
            .size(download.size)
            .hash(download.hash)
            .credential(self.credential.clone());
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        self.deps.downloader.download_all(vec![task], false, &progress).await?.into_result()?;
        let path = archive.to_path_buf();
        let (manifest, overrides) = blocking(move || {
            let zip = std::fs::File::open(&path).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
            pack::read(zip, Limits::default())
        })
        .await?;
        let file_ids: Vec<u64> = manifest.files.iter().map(|w| w.file).collect();
        let project_ids: Vec<u64> =
            manifest.files.iter().map(|w| w.project).collect::<BTreeSet<u64>>().into_iter().collect();
        let (files, projects) = tokio::join!(self.api.files_by_ids(&file_ids), self.api.mods(&project_ids));
        let (files, projects) = (by_id(files?), by_id(projects?));
        let (mut pack, held) = pack::pack_of(&manifest, overrides, &files, &projects, &self.credential)?;
        // The files CurseForge keeps from other apps: in the build already (an update), found
        // elsewhere, else downloaded by hand.
        let (root, wanted) = (game.to_path_buf(), held.clone());
        let mut found = blocking(move || Ok(already_there(&root, &wanted))).await?;
        let rest: Vec<Held> =
            held.iter().filter(|h| !found.contains_key(&h.held.sha1)).map(|h| h.held.clone()).collect();
        if let Some(finder) = &self.finder {
            found.extend(finder.find(&rest).await);
        }
        let mut by_hand = Vec::new();
        for h in &held {
            match found.get(&h.held.sha1) {
                Some(file) => pack.files.push(h.pack_file(file)),
                None => by_hand.push(h.held.to_dto()),
            }
        }
        if !by_hand.is_empty() {
            return Err(held_error(&by_hand));
        }
        pack.summary = text(&info, "summary");
        let meta = PackVersionMeta {
            project_id: project.to_string(),
            version_id: file_id.to_string(),
            version_number: version_label(&text(&info, "name"), &text(&file, "displayName")),
        };
        Ok(Fetched { pack, meta })
    }

    /// The CurseForge projects of the enabled mods the player put in `game` (none of the pack's
    /// `managed` files): by fingerprint when CurseForge answers, else by the launcher's records.
    async fn player_projects(&self, game: &Path, managed: &[String]) -> AppResult<HashSet<String>> {
        let managed: HashSet<String> = managed.iter().map(|p| p.to_lowercase()).collect();
        let have = match self.installed(game, ContentKind::Mods, None, false).await {
            Ok(have) => have,
            Err(e) => {
                tracing::warn!("CurseForge cannot name the build's mods now: {}", e.detail);
                installed(game, ContentKind::Mods, &read(game))
            }
        };
        Ok(have
            .by_project
            .iter()
            .filter(|(_, f)| {
                !f.relative.ends_with(".disabled") && !managed.contains(&f.relative.to_lowercase())
            })
            .map(|(id, _)| id.to_string())
            .collect())
    }
}

impl PackSource for CurseForgeService {
    fn fetch<'a>(
        &'a self,
        project: &'a str,
        version: &'a str,
        game: &'a Path,
        archive: &'a Path,
        op: &'a OperationHandle,
    ) -> PackFuture<'a, Fetched> {
        Box::pin(self.fetch_pack(project, version, game, archive, op))
    }

    fn versions<'a>(&'a self, project: &'a str) -> PackFuture<'a, Vec<PackVersion>> {
        Box::pin(async move { self.pack_versions(&PackArgs { project_id: project.to_string() }).await })
    }

    fn players<'a>(&'a self, game: &'a Path, managed: &'a [String]) -> PackFuture<'a, HashSet<String>> {
        Box::pin(self.player_projects(game, managed))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_pack_file_is_a_version_with_its_minecraft_and_loaders() {
        let file =
            json!({"id": 77, "displayName": "Big Pack 1.4.2", "gameVersions": ["1.20.1", "Forge", "Client"]});
        assert_eq!(
            version_of("Big Pack", &file),
            PackVersion {
                id: "77".into(),
                version_number: "1.4.2".into(),
                game_versions: vec!["1.20.1".into()],
                loaders: vec!["forge".into()],
            }
        );
    }

    #[test]
    fn server_packs_and_held_zips_are_no_versions_to_install() {
        let file = |extra: Value| {
            let mut f = json!({"id": 1, "fileName": "p.zip", "fileLength": 9, "isAvailable": true,
                "downloadUrl": "https://edge.forgecdn.net/files/p.zip",
                "hashes": [{"value": "c".repeat(40), "algo": 1}]});
            f.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            f
        };
        assert!(installable(&file(json!({}))));
        assert!(!installable(&file(json!({"isServerPack": true}))));
        assert!(!installable(&file(json!({"downloadUrl": null}))));
        assert!(!installable(&file(json!({"isAvailable": false}))));
    }
}
