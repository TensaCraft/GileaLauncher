//! The module's service: search a build's kind of content on CurseForge and install a project's
//! file with its dependencies as one transaction, backing up the mods it replaces; and its
//! modpacks (`packs`).

mod packs;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_core::content::backups::back_up;
use launcher_core::content::files::{destination, stage_listing};
use launcher_core::content::held::take_copy;
use launcher_core::content::inventory::{MetadataCache, scan_mods};
use launcher_core::content::jar::inspect_mod_jar;
use launcher_core::content::packs::{legacy_pack_names, listing_file, lists_any};
use launcher_core::feedback::{FeedbackService, OperationHandle, OperationSpec};
use launcher_core::launch::options::{component_id, game_dir};
use launcher_core::loaders::ComponentSource;
use launcher_core::lock::Coordinator;
use launcher_core::net::downloader::{Credential, DownloadProgress, DownloadTask, Downloader};
use launcher_core::packs::engine::PackDeps;
use launcher_core::providers::{ContentProvider, ProviderFuture};
use launcher_core::storage::json::write_json_file;
use launcher_core::storage::transaction::{ApplyHooks, FileTransaction, TransactionPlan};
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::provider::{
    Action, Change, FileNote, InstallAnswer, InstallArgs, InstallOutcome, ModpackBuild, NewerVersion,
    Overview, OverviewArgs, PackArgs, PackInstallArgs, PackInstalled, PackUpdateArgs, PackUpdated,
    PackVersion, PacksArgs, PlanDto, SEARCH_LIMIT, SearchArgs, SearchPage, UpdateSummary, UpdatesStatus,
    installed_key, installing_key, loader_name, update_texts,
};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode, LoaderKind, Text, mods_supported};
use reqwest::header::HeaderName;

use super::api::{CurseForgeApi, SearchQuery};
use super::catalog::{InstallFile, SEARCH_CAP, Source, install_file, search_page, text};
use super::fingerprint::FingerprintCache;
use super::held::{Elsewhere, Finder};
use super::key::{ApiKey, KEY_HEADER, keyed_host};
use super::pack::PACK_RECORD;
use super::provenance::{PROVENANCE, Record, document, installed, read};
use super::resolver::{self, Candidate, Installed, InstalledFile, Plan, mod_key};
use crate::types::{class_id, loader_type};
use launcher_core::packs::PackRecord;

/// The journal of CurseForge installs.
pub const JOURNAL: &str = ".launcher-curseforge-sync.json";
const OPERATION: &str = "curseforge-content-install";
/// Held on the build's folder for the whole install.
const LEASE: &str = "curseforge_content_install";
/// Room for the provenance file beside the download.
const METADATA_RESERVE: u64 = 4 * 1024 * 1024;

/// The launcher services the module works with.
pub struct Deps {
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub downloader: Arc<Downloader>,
    /// Whether build `key`'s game runs.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Installs a modpack's Minecraft version or loader.
    pub components: Arc<dyn ComponentSource>,
    /// The default GPU mode of new builds.
    pub gpu_mode: Arc<dyn Fn() -> String + Send + Sync>,
    /// Where files CurseForge keeps from other apps are looked for.
    pub elsewhere: Elsewhere,
}

pub use packs::{PACK_JOURNAL, PACKS};

pub struct CurseForgeService {
    api: CurseForgeApi,
    deps: Deps,
    /// The key for CurseForge's file CDN.
    credential: Credential,
    metadata: Arc<MetadataCache>,
    fingerprints: Arc<FingerprintCache>,
    /// The projects the installed lists named, for this session only (never kept on disk).
    projects: std::sync::Mutex<std::collections::HashMap<u64, serde_json::Value>>,
    /// What the core's pack engine needs of the launcher.
    packs: PackDeps,
    /// Finds files CurseForge keeps from other apps elsewhere.
    finder: Option<Finder>,
}

/// A candidate's file and its place in the build.
struct Planned<'a> {
    candidate: &'a Candidate,
    file: InstallFile,
    relative: String,
    /// The file it replaces: its recorded place and where it lies now.
    replaced: Option<(String, String)>,
}

/// A build for content of one kind.
struct Target {
    build: Build,
    game: PathBuf,
    game_version: Option<String>,
    loader: Option<&'static str>,
}

/// The safety net under the plan: no new jar declares a mod id an enabled jar of `game` that stays
/// already declares (two copies of a mod stop the game). Blocking: every jar there is read.
fn one_copy_each(
    tx: &FileTransaction,
    game: &Path,
    loader: Option<LoaderKind>,
    metadata: &MetadataCache,
    destinations: &[String],
    stale: &[String],
) -> AppResult<()> {
    let here = scan_mods(game, loader, metadata, &[]);
    let leaving = |file: &str| {
        stale.iter().chain(destinations).any(|gone| gone.eq_ignore_ascii_case(&format!("mods/{file}")))
    };
    for relative in destinations {
        let Ok(descriptor) = inspect_mod_jar(&tx.stage_path(relative)?, loader) else { continue };
        let id = normalized(&descriptor.mod_id);
        let taken = here.iter().find(|entry| {
            entry.item.enabled
                && !leaving(&entry.item.file)
                && entry.item.mod_id.as_deref().is_some_and(|other| normalized(other) == id)
        });
        if let Some(entry) = taken {
            return Err(AppError::new(
                ErrorCode::ContentConflict,
                format!("{} already has mod {id}: {}", game.display(), entry.item.file),
            )
            .with_param("name", entry.item.file.clone()));
        }
    }
    Ok(())
}

impl Target {
    /// The build's loader as its mod jars are read.
    fn loader_kind(&self) -> Option<LoaderKind> {
        component_id(&self.build).and_then(LoaderKind::of_component)
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

fn id_of(raw: &str) -> AppResult<u64> {
    raw.trim()
        .parse()
        .map_err(|_| AppError::new(ErrorCode::InvalidInput, format!("not a CurseForge id: {raw:?}")))
}

/// The build's files of `kind` (enabled or switched off) with their fingerprints.
fn fingerprinted(game: &std::path::Path, kind: ContentKind, cache: &FingerprintCache) -> Vec<(String, u32)> {
    let folder = kind.folder();
    let Ok(entries) = std::fs::read_dir(game.join(folder)) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let plain = name.trim_end_matches(".disabled").to_ascii_lowercase();
            if !(plain.ends_with(".jar") || plain.ends_with(".zip")) {
                return None;
            }
            let print = cache.of(&e.path()).ok()?;
            Some((format!("{folder}/{name}"), print))
        })
        .collect()
}

/// A mod's id as two copies of one mod share it.
fn normalized(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('-', "_")
}

impl CurseForgeService {
    pub fn new(api: CurseForgeApi, deps: Deps, key: &ApiKey) -> CurseForgeService {
        let credential = Credential::new(HeaderName::from_static(KEY_HEADER), key.header(), keyed_host);
        let packs = PackDeps {
            versions: deps.versions.clone(),
            instances: deps.instances.clone(),
            feedback: deps.feedback.clone(),
            downloader: deps.downloader.clone(),
            components: deps.components.clone(),
            gpu_mode: deps.gpu_mode.clone(),
            running: deps.running.clone(),
        };
        let finder = Finder::new(deps.elsewhere.clone())
            .inspect_err(|e| tracing::warn!("held files cannot be looked for elsewhere: {}", e.detail))
            .ok();
        CurseForgeService {
            api,
            deps,
            packs,
            finder,
            credential,
            metadata: Arc::default(),
            fingerprints: Arc::default(),
            projects: Default::default(),
        }
    }

    fn build(&self, key: &str) -> AppResult<Build> {
        self.deps.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })
    }

    /// Build `key` for content of `kind`: mods and shader packs need a mod loader.
    fn target(&self, key: &str, kind: ContentKind) -> AppResult<Target> {
        let build = self.build(key)?;
        if kind != ContentKind::ResourcePacks && !mods_supported(build.client.as_deref()) {
            return Err(AppError::new(ErrorCode::Unsupported, format!("{} runs no mod loader", build.name))
                .with_param("version", build.name.clone()));
        }
        Ok(Target {
            game: game_dir(&build, self.deps.versions.minecraft_dir()),
            game_version: build.version.clone().filter(|v| !v.trim().is_empty()),
            loader: loader_name(build.loader.as_deref(), build.client.as_deref()),
            build,
        })
    }

    /// A page of this build's kind of content on CurseForge.
    pub async fn search(&self, args: &SearchArgs) -> AppResult<SearchPage> {
        let target = self.target(&args.key, args.kind)?;
        if args.offset >= SEARCH_CAP {
            return Ok(SearchPage {
                hits: Vec::new(),
                total: SEARCH_CAP,
                offset: args.offset,
                limit: SEARCH_LIMIT,
            });
        }
        let query = SearchQuery {
            class_id: class_id(args.kind),
            text: &args.query,
            game_version: target.game_version.as_deref(),
            loader: (args.kind == ContentKind::Mods).then_some(target.loader).flatten().and_then(loader_type),
            index: args.offset,
            page_size: SEARCH_LIMIT.min(SEARCH_CAP - args.offset),
        };
        Ok(search_page(&self.api.search(&query).await?, args.offset, SEARCH_LIMIT))
    }

    /// The CurseForge projects of `kind` in the build: those the launcher recorded, and the
    /// files CurseForge knows by their fingerprints (put there by hand or by another provider);
    /// with `hints`, also the other enabled jars by their mod ids (a plan needs them, a list of
    /// what is installed does not — reading every jar costs).
    async fn installed(
        &self,
        game: &std::path::Path,
        kind: ContentKind,
        loader: Option<LoaderKind>,
        hints: bool,
    ) -> AppResult<Installed> {
        let mut found = installed(game, kind, &read(game));
        let known: HashSet<String> = found.by_project.values().map(|f| f.relative.to_lowercase()).collect();
        let (root, cache) = (game.to_path_buf(), self.fingerprints.clone());
        let unknown: Vec<(String, u32)> = blocking(move || Ok(fingerprinted(&root, kind, &cache)))
            .await?
            .into_iter()
            .filter(|(relative, _)| !known.contains(&relative.to_lowercase()))
            .collect();
        let prints: Vec<u32> = unknown.iter().map(|(_, print)| *print).collect();
        let answer = self.api.fingerprints(&prints).await?;
        for matched in answer["exactMatches"].as_array().into_iter().flatten() {
            let file = &matched["file"];
            let (Some(project), Some(file_id)) = (file["modId"].as_u64(), file["id"].as_u64()) else {
                continue;
            };
            let print = file["fileFingerprint"].as_u64();
            for (relative, _) in unknown.iter().filter(|(_, p)| Some(u64::from(*p)) == print) {
                let name = relative.rsplit('/').next().unwrap_or(relative).trim_end_matches(".disabled");
                found.by_project.entry(project).or_insert_with(|| InstalledFile {
                    file_id,
                    filename: name.to_string(),
                    version: text(file, "displayName"),
                    relative: relative.clone(),
                    date: Some(text(file, "fileDate")).filter(|d| !d.is_empty()),
                    release_type: file["releaseType"].as_u64(),
                });
            }
        }
        if hints && kind == ContentKind::Mods {
            // The enabled jars CurseForge does not know, by their mod ids.
            let known: HashSet<String> =
                found.by_project.values().map(|f| f.relative.to_lowercase()).collect();

            let (root, metadata) = (game.to_path_buf(), self.metadata.clone());
            let jars = blocking(move || Ok(scan_mods(&root, loader, &metadata, &[]))).await?;
            for entry in jars.into_iter().filter(|e| e.item.enabled) {
                let relative = format!("mods/{}", entry.item.file);
                let Some(id) =
                    entry.item.mod_id.as_deref().filter(|_| !known.contains(&relative.to_lowercase()))
                else {
                    continue;
                };
                found.by_mod_id.entry(mod_key(id)).or_insert_with(|| InstalledFile {
                    version: entry.item.version.clone().unwrap_or_else(|| entry.item.filename.clone()),
                    filename: entry.item.filename.clone(),
                    relative,
                    ..InstalledFile::default()
                });
            }
        }
        Ok(found)
    }

    async fn plan_for(&self, target: &Target, args: &InstallArgs, have: &Installed) -> AppResult<Plan> {
        let scope = resolver::Target {
            kind: args.kind,
            game_version: target.game_version.as_deref(),
            loader: target.loader,
            installed: have,
            finder: self.finder.as_ref(),
        };
        let exact = args.version_id.as_deref().and_then(|id| id.parse().ok());
        resolver::plan(&self.api, &scope, id_of(&args.project_id)?, exact, &args.optional).await
    }

    /// What installing `args`' project takes.
    pub async fn plan(&self, args: &InstallArgs) -> AppResult<PlanDto> {
        let target = self.target(&args.key, args.kind)?;
        let have = self.installed(&target.game, args.kind, target.loader_kind(), true).await?;
        Ok(self.plan_for(&target, args, &have).await?.to_dto())
    }

    /// Plans `args` again and installs what the plan needs when that is what the user approved;
    /// otherwise answers with the plan as it is now.
    pub async fn install(&self, args: &InstallArgs) -> AppResult<InstallAnswer> {
        let target = self.target(&args.key, args.kind)?;
        let _lease = self
            .deps
            .instances
            .try_acquire(&target.game, LEASE)
            .map_err(|e| e.with_param("version", target.build.name.clone()))?;
        let have = self.installed(&target.game, args.kind, target.loader_kind(), true).await?;
        let plan = self.plan_for(&target, args, &have).await?;
        let pending = plan.pending();
        let approved: HashSet<Change> = args.approved.iter().cloned().collect();
        let dto = plan.to_dto();
        let change = |c: &Candidate| Change {
            project_id: c.project_id().to_string(),
            version_id: c.file_id().to_string(),
            action: c.action,
        };
        if !dto.can_install() || !pending.iter().all(|c| approved.contains(&change(c))) {
            return Ok(InstallAnswer::Replanned(Box::new(dto)));
        }
        let main = plan.main.item();
        let outcome = InstallOutcome {
            project_id: main.project_id.clone(),
            filename: main.filename.clone(),
            version_number: main.version_number.clone(),
        };
        if pending.is_empty() {
            return Ok(InstallAnswer::Installed(outcome));
        }
        let name = main.title.clone();
        let done = if plan.main.action == Action::Replace {
            update_texts(args.kind).done
        } else {
            installed_key(args.kind)
        };
        let title =
            if pending.len() > 1 { "installing_modrinth_dependencies" } else { installing_key(args.kind) };
        let op =
            self.deps.feedback.begin(OperationSpec::new(Text::key(title).param("name", &name), "curseforge"));
        match self.apply(&target, args.kind, &pending, &plan.found, &op).await {
            Ok(()) => {
                op.finish();
                self.deps.feedback.success(Text::key(done).param("name", &name));
                Ok(InstallAnswer::Installed(outcome))
            }
            Err(e) => {
                op.fail(Text::key("installation_failed"));
                Err(e)
            }
        }
    }

    /// Puts the pending candidates' files in place, backs up and removes what they replace and
    /// records them — one transaction.
    async fn apply(
        &self,
        target: &Target,
        kind: ContentKind,
        pending: &[&Candidate],
        found: &std::collections::HashMap<u64, InstallFile>,
        op: &OperationHandle,
    ) -> AppResult<()> {
        let game = &target.game;
        let mut records = read(game);
        let mut planned = Vec::new();
        let mut names: HashSet<String> = HashSet::new();
        for candidate in pending {
            // A file CurseForge keeps from other apps comes from where it was found.
            let file = install_file(&candidate.file)
                .or_else(|| found.get(&candidate.file_id()).cloned())
                .ok_or_else(|| {
                    AppError::new(ErrorCode::NoFileFound, "the file has no address or hash to check")
                        .with_param("name", candidate.item().title)
                })?;
            let relative = destination(kind, &file.filename)?;
            if !names.insert(relative.to_lowercase()) {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("two files of the plan are {relative}"),
                )
                .with_param("name", file.filename.clone()));
            }
            // The replaced file's record (its enabled place) and where it lies now.
            let replaced = candidate
                .replaces
                .clone()
                .filter(|_| candidate.action == Action::Replace)
                .map(|now| (now.trim_end_matches(".disabled").to_string(), now));
            let own = replaced.as_ref().map(|(_, now)| now.to_lowercase());
            for taken in [relative.clone(), format!("{relative}.disabled")] {
                if game.join(&taken).exists() && own.as_deref() != Some(taken.to_lowercase().as_str()) {
                    return Err(AppError::new(ErrorCode::ContentConflict, format!("{taken} already exists"))
                        .with_param("name", file.filename.clone()));
                }
            }
            planned.push(Planned { candidate, file, relative, replaced });
        }
        let stale: Vec<String> = planned
            .iter()
            .filter_map(|Planned { relative, replaced, .. }| {
                replaced.as_ref().map(|(_, now)| now.clone()).filter(|now| now != relative)
            })
            .collect();
        if kind == ContentKind::Mods {
            for Planned { replaced, .. } in &planned {
                if let Some((_, now)) = replaced {
                    let name =
                        now.rsplit('/').next().unwrap_or(now).trim_end_matches(".disabled").to_string();
                    let (root, relative) = (game.clone(), now.clone());
                    blocking(move || back_up(&root, &relative, &name)).await?;
                }
            }
        }
        for Planned { candidate, file, relative, replaced } in &planned {
            if let Some((old, _)) = replaced {
                records.remove(old);
            }
            let item = candidate.item();
            records.insert(
                relative.clone(),
                Record {
                    kind,
                    project_id: candidate.project_id(),
                    file_id: candidate.file_id(),
                    title: item.title,
                    version: text(&candidate.file, "displayName"),
                    filename: file.filename.clone(),
                    page: item.url,
                    sha1: file.hash.hex.clone(),
                    date: Some(text(&candidate.file, "fileDate")).filter(|d| !d.is_empty()),
                    release_type: candidate.file["releaseType"].as_u64(),
                },
            );
        }
        // An updated pack under a new file name keeps its place in the game's list of packs.
        let renames: Vec<(String, String)> = planned
            .iter()
            .filter_map(|Planned { file, replaced, .. }| {
                let (_, now) = replaced.as_ref()?;
                let old = now.rsplit('/').next()?.trim_end_matches(".disabled").to_string();
                (old != file.filename).then(|| (old, file.filename.clone()))
            })
            .collect();
        let legacy = legacy_pack_names(target.game_version.as_deref());
        let listing = listing_file(kind)
            .filter(|file| !renames.is_empty() && lists_any(&game.join(file), kind, legacy, &renames));
        let transaction = TransactionPlan {
            replacements: planned
                .iter()
                .map(|p| p.relative.clone())
                .chain([PROVENANCE.to_string()])
                .chain(listing.map(str::to_string))
                .collect(),
            stale: stale.clone(),
            staged_bytes: planned.iter().map(|p| p.file.size).sum::<u64>().saturating_add(METADATA_RESERVE),
            ..TransactionPlan::new(OPERATION)
        };
        let root = game.clone();
        let tx = blocking(move || FileTransaction::begin(&root, JOURNAL, transaction, None)).await?;
        let staged = self.stage(&tx, &planned, &records, op).await;
        let destinations: Vec<String> = planned.iter().map(|p| p.relative.clone()).collect();
        let (metadata, game, loader) = (self.metadata.clone(), game.clone(), target.loader_kind());
        // Every jar of the build may be read here, so not on the async workers.
        blocking(move || {
            let staged = staged
                .and_then(|()| match kind {
                    ContentKind::Mods => one_copy_each(&tx, &game, loader, &metadata, &destinations, &stale),
                    _ => Ok(()),
                })
                .and_then(|()| match listing {
                    Some(file) => stage_listing(&tx, &game, file, kind, legacy, &renames),
                    None => Ok(()),
                });
            match staged {
                Ok(()) => tx.apply(ApplyHooks::default()),
                Err(e) => Err(tx.abort(e)),
            }
        })
        .await
    }

    async fn stage(
        &self,
        tx: &FileTransaction,
        planned: &[Planned<'_>],
        records: &BTreeMap<String, Record>,
        op: &OperationHandle,
    ) -> AppResult<()> {
        let (mut tasks, mut copies) = (Vec::new(), Vec::new());
        for Planned { file, relative, .. } in planned {
            let dest = tx.stage_path(relative)?;
            let task = DownloadTask::new(file.url.clone(), &dest).size(file.size).hash(file.hash.clone());
            match &file.from {
                Source::CurseForge => tasks.push(task.credential(self.credential.clone())),
                Source::Elsewhere => tasks.push(task),
                Source::Copy(from) => {
                    copies.push((from.clone(), dest, file.size, file.hash.clone(), file.filename.clone()))
                }
            }
        }
        blocking(move || {
            copies
                .iter()
                .try_for_each(|(from, dest, size, hash, name)| take_copy(from, dest, *size, hash, name))
        })
        .await?;
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        self.deps.downloader.download_all(tasks, false, &progress).await?.into_result()?;
        let staged = tx.stage_path(PROVENANCE)?;
        write_json_file(&staged, &document(records), 2)
            .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", staged.display())))
    }

    /// The build's files of `args.kind` installed from CurseForge, each with its page.
    /// The projects of `ids` (their names, pages and icons): asked of CurseForge once a session.
    async fn projects_of(&self, ids: &[u64]) -> std::collections::HashMap<u64, serde_json::Value> {
        let lock = || self.projects.lock().unwrap_or_else(|e| e.into_inner());
        let missing: Vec<u64> = {
            let known = lock();
            ids.iter().copied().filter(|id| !known.contains_key(id)).collect()
        };
        if missing.is_empty() {
            let known = lock();
            return ids.iter().filter_map(|id| Some((*id, known.get(id)?.clone()))).collect();
        }
        match self.api.mods(&missing).await {
            Ok(found) => {
                let mut known = lock();
                for project in found {
                    if let Some(id) = project["id"].as_u64() {
                        known.insert(id, project);
                    }
                }
            }
            Err(e) => tracing::warn!("CurseForge cannot name the installed projects now: {}", e.detail),
        }
        let known = lock();
        ids.iter().filter_map(|id| Some((*id, known.get(id)?.clone()))).collect()
    }

    /// The projects of `ids` as CurseForge has them now (an update check needs their latest
    /// files); the session's names and icons are refreshed with them.
    async fn fresh_projects(
        &self,
        ids: &[u64],
    ) -> AppResult<std::collections::HashMap<u64, serde_json::Value>> {
        let found = self.api.mods(ids).await?;
        let mut known = self.projects.lock().unwrap_or_else(|e| e.into_inner());
        let mut fresh = std::collections::HashMap::new();
        for project in found {
            if let Some(id) = project["id"].as_u64() {
                known.insert(id, project.clone());
                fresh.insert(id, project);
            }
        }
        Ok(fresh)
    }

    /// Marks the notes of enabled files CurseForge has a newer fitting file for (a release, or
    /// for a beta or alpha also as new a beta or alpha), downloadable by other apps.
    async fn updates(
        &self,
        target: &Target,
        kind: ContentKind,
        have: &Installed,
        notes: &mut [FileNote],
    ) -> UpdateSummary {
        let summary = |status, available, unchecked| UpdateSummary { status, available, unchecked };
        let eligible: Vec<(u64, &InstalledFile)> = have
            .by_project
            .iter()
            .filter(|(_, f)| !f.relative.ends_with(".disabled"))
            .map(|(id, f)| (*id, f))
            .collect();
        if eligible.is_empty() {
            return summary(UpdatesStatus::NoEnabled, 0, 0);
        }
        let Some(game) = target.game_version.as_deref() else {
            return summary(UpdatesStatus::Unchecked, 0, eligible.len());
        };
        let ids: Vec<u64> = eligible.iter().map(|(id, _)| *id).collect();
        let Ok(projects) = self.fresh_projects(&ids).await else {
            return summary(UpdatesStatus::Failed, 0, 0);
        };
        let loader = (kind == ContentKind::Mods).then_some(target.loader).flatten().and_then(loader_type);
        let (mut wanted, mut unchecked) = (Vec::new(), 0);
        for (id, file) in &eligible {
            let Some(project) = projects.get(id) else {
                unchecked += 1;
                continue;
            };
            let allowed = file.release_type.unwrap_or(1).max(1);
            let newest = project["latestFilesIndexes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| i["gameVersion"].as_str() == Some(game))
                .filter(|i| loader.is_none_or(|l| i["modLoader"].as_u64() == Some(u64::from(l))))
                .filter(|i| i["releaseType"].as_u64().is_some_and(|r| r <= allowed))
                .filter_map(|i| i["fileId"].as_u64())
                .max();
            if let Some(newest) = newest.filter(|n| *n > file.file_id) {
                wanted.push((*id, newest));
            }
        }
        let newer_ids: Vec<u64> = wanted.iter().map(|(_, file)| *file).collect();
        let Ok(files) = self.api.files_by_ids(&newer_ids).await else {
            return summary(UpdatesStatus::Failed, 0, 0);
        };
        let mut found = 0;
        for (id, newer) in wanted {
            let Some(file) = files.iter().find(|f| f["id"].as_u64() == Some(newer)) else { continue };
            // A file kept from other apps is no update the launcher can bring.
            if install_file(file).is_none() {
                continue;
            }
            let name = projects.get(&id).map(|p| text(p, "name")).unwrap_or_default();
            let version = super::resolver::version_label(&name, &text(file, "displayName"));
            if let Some(note) = notes.iter_mut().find(|n| n.project_id == id.to_string()) {
                note.update = Some(NewerVersion { version_id: newer.to_string(), version_number: version });
                found += 1;
            }
        }
        let status = if found > 0 {
            UpdatesStatus::Available
        } else if unchecked > 0 {
            UpdatesStatus::Unchecked
        } else {
            UpdatesStatus::Current
        };
        summary(status, found, unchecked)
    }

    pub async fn overview(&self, args: &OverviewArgs) -> AppResult<Overview> {
        let target = self.target(&args.key, args.kind)?;
        let records = read(&target.game);
        // Out of CurseForge's reach, the records alone name the files.
        let (have, identified) =
            match self.installed(&target.game, args.kind, target.loader_kind(), false).await {
                Ok(have) => (have, true),
                Err(e) => {
                    tracing::warn!("CurseForge cannot name the build's files now: {}", e.detail);
                    (installed(&target.game, args.kind, &records), false)
                }
            };
        let recorded: std::collections::HashMap<u64, &Record> =
            records.values().map(|r| (r.project_id, r)).collect();
        // What the launcher put there through CurseForge: its installs, and its modpack's files.
        let ours: HashSet<String> = records
            .keys()
            .cloned()
            .chain(PackRecord::read(&target.game, PACK_RECORD).map(|r| r.managed_files).unwrap_or_default())
            .map(|path| path.to_lowercase())
            .collect();
        let projects = self.projects_of(&have.by_project.keys().copied().collect::<Vec<_>>()).await;
        let mut notes: Vec<FileNote> = have
            .by_project
            .iter()
            .map(|(id, file)| {
                let project = projects.get(id);
                let https = |url: String| Some(url).filter(|u| u.starts_with("https://"));
                FileNote {
                    file: file.relative.rsplit('/').next().unwrap_or(&file.relative).to_string(),
                    project_id: id.to_string(),
                    slug: project.map(|p| text(p, "slug")).unwrap_or_default(),
                    title: recorded
                        .get(id)
                        .map(|r| r.title.clone())
                        .or_else(|| project.map(|p| text(p, "name")))
                        .unwrap_or_default(),
                    version_number: file.version.clone(),
                    update: None,
                    url: recorded
                        .get(id)
                        .and_then(|r| r.page.clone())
                        .or_else(|| project.and_then(|p| https(text(&p["links"], "websiteUrl")))),
                    icon_url: project.and_then(|p| https(text(&p["logo"], "thumbnailUrl"))),
                    installed: ours.contains(&file.relative.trim_end_matches(".disabled").to_lowercase()),
                }
            })
            .collect();
        notes.sort_by(|a, b| a.file.cmp(&b.file));
        let updates = match (args.check_updates, identified) {
            (false, _) => None,
            (true, false) => {
                Some(UpdateSummary { status: UpdatesStatus::Failed, available: 0, unchecked: 0 })
            }
            (true, true) => Some(self.updates(&target, args.kind, &have, &mut notes).await),
        };
        Ok(Overview { notes, updates })
    }
}

impl ContentProvider for CurseForgeService {
    fn search(&self, args: SearchArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { CurseForgeService::search(self, &args).await })
    }

    fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
        Box::pin(async move { CurseForgeService::plan(self, &args).await })
    }

    fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
        Box::pin(async move { CurseForgeService::install(self, &args).await })
    }

    fn overview(&self, args: OverviewArgs) -> ProviderFuture<'_, Overview> {
        Box::pin(async move { CurseForgeService::overview(self, &args).await })
    }

    fn modpacks(&self, args: PacksArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { self.packs(&args).await })
    }

    fn modpack_versions(&self, args: PackArgs) -> ProviderFuture<'_, Vec<PackVersion>> {
        Box::pin(async move { self.pack_versions(&args).await })
    }

    fn install_modpack(&self, args: PackInstallArgs) -> ProviderFuture<'_, PackInstalled> {
        Box::pin(async move { self.install_pack(&args).await })
    }

    fn modpack_builds(&self) -> ProviderFuture<'_, Vec<ModpackBuild>> {
        Box::pin(async move { CurseForgeService::modpack_builds(self).await })
    }

    fn update_modpack(&self, args: PackUpdateArgs) -> ProviderFuture<'_, PackUpdated> {
        Box::pin(async move { self.update_pack(&args).await })
    }
}
