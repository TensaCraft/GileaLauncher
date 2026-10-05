//! The module's service: search a build's kind of content on Modrinth, name
//! what was installed from it, and install a project's file with its dependencies as one
//! transaction, backing up the mods it replaces.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use launcher_core::content::backups::back_up;
use launcher_core::content::files::{destination, stage_listing};
use launcher_core::content::packs::{legacy_pack_names, listing_file, lists_any};
use launcher_core::feedback::{FeedbackService, OperationHandle, OperationSpec};
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, Downloader};
use launcher_core::packs::engine::{PackDeps, PackKind, PackVersionMeta};
use launcher_core::packs::flow::{self, Fetched, HeldChoice, PackFuture, PackSource};
use launcher_core::providers::{ContentProvider, ProviderFuture};
use launcher_core::storage::json::write_json_file;
use launcher_core::storage::transaction::{ApplyHooks, FileTransaction, TransactionPlan};
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode, Text, mods_supported};
use serde_json::{Value, json};

use super::api::ModrinthApi;
use super::catalog::{primary_file, search_facets, search_page, text, version_fits};
use super::components::ComponentSource;
use super::inventory::{Identities, InstalledItem, Inventory, normalized, scan};
use super::pack::{self, PACK_RECORD, PackRecord, pack_download};
use super::provenance::{DigestCache, PROVENANCE, Record, read, with_records};
use super::resolver::{self, Candidate, Plan, Project, newer};
use crate::types::{BuildArgs, modrinth_url, project_type};
use launcher_shared::provider::{
    Action, Change, FileNote, InstallAnswer, InstallArgs, InstallOutcome, ModpackBuild, NewerVersion,
    Overview, OverviewArgs, PACKS_LIMIT, PackArgs, PackInstallArgs, PackInstalled, PackUpdateArgs,
    PackUpdated, PackVersion, PacksArgs, PlanDto, SEARCH_LIMIT, SearchArgs, SearchPage, UpdateSummary,
    UpdatesStatus, installed_key, installing_key, loader_name, update_texts,
};

/// The journal of Modrinth installs.
pub const JOURNAL: &str = ".launcher-modrinth-sync.json";
const OPERATION: &str = "modrinth-content-install";
/// Held on the build's folder for the whole install.
const LEASE: &str = "modrinth_content_install";
/// Room for the provenance file beside the download.
const METADATA_RESERVE: u64 = 4 * 1024 * 1024;
const PACK_OPERATION: &str = "modrinth-pack-install";
/// Held on a new build's folder while a modpack fills it.
const PACK_LEASE: &str = "modrinth_pack_install";
/// A modpack update's transaction and its journal (reserved in a build: packs cannot write it).
const PACK_UPDATE_OPERATION: &str = "modrinth-pack-update";
const PACK_JOURNAL: &str = ".launcher-pack-sync.json";
/// Where the downloaded `.mrpack` waits (always removed after).
const PACK_ARCHIVE: &str = ".launcher-pack.mrpack";

/// Modrinth's modpack builds, as the core's pack engine tells them apart.
const PACKS: PackKind = PackKind {
    provider: "Modrinth",
    record: PACK_RECORD,
    journal: PACK_JOURNAL,
    archive: PACK_ARCHIVE,
    lease: PACK_LEASE,
    install_operation: PACK_OPERATION,
    update_operation: PACK_UPDATE_OPERATION,
    commit_prefix: "modrinth-pack",
    project_option: "modrinthProjectId",
    version_option: "modrinthVersionId",
};

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
}

pub struct ModrinthService {
    api: ModrinthApi,
    deps: Deps,
    digests: Arc<DigestCache>,
    /// Modrinth's answers for files already named this session.
    identities: Arc<Identities>,
    /// Projects' icons (`None`: the project has none) learnt this session.
    icons: Mutex<HashMap<String, Option<String>>>,
    /// What the core's pack engine needs of the launcher.
    packs: PackDeps,
}

/// A build as the target of one kind of content.
struct Target {
    build: Build,
    game: PathBuf,
    /// Only for mods.
    loader: Option<&'static str>,
    game_version: Option<String>,
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

fn summary(status: UpdatesStatus, available: usize, unchecked: usize) -> UpdateSummary {
    UpdateSummary { status, available, unchecked }
}

/// `mods/x.jar.disabled` → `x.jar.disabled`.
fn file_name(relative: &str) -> String {
    relative.rsplit('/').next().unwrap_or(relative).to_string()
}

/// `item`'s note; `ours`: the launcher installed it from Modrinth (by itself or with a pack).
fn note(item: &InstalledItem, kind: ContentKind, ours: bool) -> FileNote {
    let (project_id, slug) =
        (item.project_id.clone().unwrap_or_default(), item.slug.clone().unwrap_or_default());
    let page = if slug.is_empty() { &project_id } else { &slug };
    FileNote {
        file: file_name(&item.relative),
        url: (!page.is_empty()).then(|| modrinth_url(project_type(kind), page)),
        project_id,
        slug,
        title: item.title.clone().unwrap_or_default(),
        version_number: item.version_number.clone().unwrap_or_default(),
        update: None,
        icon_url: None,
        installed: ours,
    }
}

/// A project's icon as a row shows it: an https picture only.
fn icon_of(project: &Value) -> Option<String> {
    Some(text(project, "icon_url")).filter(|url| url.starts_with("https://"))
}

impl ModrinthService {
    pub fn new(api: ModrinthApi, deps: Deps) -> ModrinthService {
        let packs = PackDeps {
            versions: deps.versions.clone(),
            instances: deps.instances.clone(),
            feedback: deps.feedback.clone(),
            downloader: deps.downloader.clone(),
            components: deps.components.clone(),
            gpu_mode: deps.gpu_mode.clone(),
            running: deps.running.clone(),
        };
        ModrinthService {
            api,
            deps,
            packs,
            digests: Arc::new(DigestCache::default()),
            identities: Arc::new(Identities::default()),
            icons: Mutex::new(HashMap::new()),
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
        let loader = match kind {
            ContentKind::Mods => loader_name(build.loader.as_deref(), build.client.as_deref()),
            _ => None,
        };
        Ok(Target {
            game: game_dir(&build, self.deps.versions.minecraft_dir()),
            game_version: build.version.clone().filter(|v| !v.trim().is_empty()),
            loader,
            build,
        })
    }

    /// A page of this build's kind of content on Modrinth.
    pub async fn search(&self, args: &SearchArgs) -> AppResult<SearchPage> {
        let target = self.target(&args.key, args.kind)?;
        let facets = search_facets(args.kind, target.loader, target.game_version.as_deref());
        let raw = self.api.search(args.query.trim(), &facets, args.offset, SEARCH_LIMIT).await?;
        Ok(search_page(&raw, args.offset, SEARCH_LIMIT))
    }

    /// Build `game`'s files of `kind`, named by Modrinth too.
    async fn inventory(&self, game: &Path, kind: ContentKind) -> AppResult<Inventory> {
        let (root, cache) = (game.to_path_buf(), self.digests.clone());
        let mut inventory = blocking(move || Ok(scan(&root, kind, &cache))).await?;
        inventory.identify(&self.api, game, &self.identities).await?;
        Ok(inventory)
    }

    /// The plan of `args`' install: the version the user saw when it still fits, with their picks.
    async fn plan_for(&self, target: &Target, args: &InstallArgs) -> AppResult<Plan> {
        let inventory = self.inventory(&target.game, args.kind).await?;
        let project = Project::named(&args.project_id, &args.slug, &args.title, project_type(args.kind));
        let game_version = target.game_version.as_deref();
        let exact = match &args.version_id {
            Some(id) => self.api.version(id).await.ok().filter(|v| {
                text(v, "project_id") == args.project_id && version_fits(v, target.loader, game_version)
            }),
            None => None,
        };
        let scope =
            resolver::Target { kind: args.kind, loader: target.loader, game_version, inventory: &inventory };
        resolver::plan(&self.api, &scope, project, exact, &args.optional, args.alone).await
    }

    /// What installing `args`' project takes.
    pub async fn plan(&self, args: &InstallArgs) -> AppResult<PlanDto> {
        let target = self.target(&args.key, args.kind)?;
        Ok(self.plan_for(&target, args).await?.to_dto())
    }

    /// The projects of the kind whose files in the build are theirs — by the provenance and by
    /// Modrinth's answer for each file; out of Modrinth's reach, the provenance alone answers.
    pub async fn installed(&self, args: &BuildArgs) -> AppResult<Vec<String>> {
        let build = self.build(&args.key)?;
        let game = game_dir(&build, self.deps.versions.minecraft_dir());
        let (root, cache, kind) = (game.clone(), self.digests.clone(), args.kind);
        let mut inventory = blocking(move || Ok(scan(&root, kind, &cache))).await?;
        let mut named = inventory.clone();
        if named.identify(&self.api, &game, &self.identities).await.is_ok() {
            inventory = named;
        }
        Ok(inventory.owned_projects())
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
        let plan = self.plan_for(&target, args).await?;
        let pending: Vec<&Candidate> =
            plan.install_order().into_iter().filter(|c| c.action != Action::Satisfied).collect();
        let approved: HashSet<Change> = args.approved.iter().cloned().collect();
        let Some(main) = plan
            .main
            .as_ref()
            .filter(|_| plan.can_install() && pending.iter().all(|c| approved.contains(&c.change())))
        else {
            return Ok(InstallAnswer::Replanned(Box::new(plan.to_dto())));
        };
        let outcome = InstallOutcome {
            project_id: main.project.id.clone(),
            filename: main.file.filename.clone(),
            version_number: text(&main.version, "version_number"),
        };
        if pending.is_empty() {
            return Ok(InstallAnswer::Installed(outcome));
        }
        let name = main.project.title.clone();
        // Replacing a file with a newer one is its update.
        let done = if main.action == Action::Replace {
            update_texts(args.kind).done
        } else {
            installed_key(args.kind)
        };
        let title =
            if pending.len() > 1 { "installing_modrinth_dependencies" } else { installing_key(args.kind) };
        let op =
            self.deps.feedback.begin(OperationSpec::new(Text::key(title).param("name", &name), "modrinth"));
        match self.apply(&target, args.kind, &pending, &op).await {
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
    /// records them — one transaction. A running game does not stop it: a file the game holds
    /// open undoes it all as `FileInUse`.
    async fn apply(
        &self,
        target: &Target,
        kind: ContentKind,
        pending: &[&Candidate],
        op: &OperationHandle,
    ) -> AppResult<()> {
        let game = &target.game;
        let mut destinations: Vec<String> = Vec::new();
        let mut names: HashSet<String> = HashSet::new();
        for candidate in pending {
            let relative = destination(kind, &candidate.file.filename)?;
            if !names.insert(relative.to_lowercase()) {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("two files of the plan are {relative}"),
                )
                .with_param("name", candidate.file.filename.clone()));
            }
            let own = candidate.replaced().map(|i| i.relative.to_lowercase());
            for taken in [relative.clone(), format!("{relative}.disabled")] {
                if fs::symlink_metadata(game.join(&taken)).is_ok()
                    && own.as_deref() != Some(taken.to_lowercase().as_str())
                {
                    return Err(AppError::new(ErrorCode::ContentConflict, format!("{taken} already exists"))
                        .with_param("name", candidate.file.filename.clone()));
                }
            }
            destinations.push(relative);
        }
        let stale: Vec<String> = pending
            .iter()
            .zip(&destinations)
            .filter_map(|(c, dest)| {
                c.replaced()
                    .map(|i| i.relative.clone())
                    .filter(|old| old.to_lowercase() != dest.to_lowercase())
            })
            .collect();
        if kind == ContentKind::Mods {
            for item in pending.iter().filter_map(|c| c.replaced()) {
                let (root, relative, name) = (game.clone(), item.relative.clone(), item.filename.clone());
                blocking(move || back_up(&root, &relative, &name)).await?;
            }
        }
        let records: Vec<(String, Record)> =
            pending.iter().zip(&destinations).map(|(c, dest)| (dest.clone(), c.record(kind))).collect();
        // An updated pack under a new file name keeps its place in the game's list of packs.
        let renames: Vec<(String, String)> = pending
            .iter()
            .filter_map(|c| c.replaced().map(|i| (i.filename.clone(), c.file.filename.clone())))
            .filter(|(old, new)| old != new)
            .collect();
        let legacy = legacy_pack_names(target.game_version.as_deref());
        let listing = listing_file(kind)
            .filter(|file| !renames.is_empty() && lists_any(&game.join(file), kind, legacy, &renames));
        let plan = TransactionPlan {
            replacements: destinations
                .iter()
                .cloned()
                .chain([PROVENANCE.to_string()])
                .chain(listing.map(str::to_string))
                .collect(),
            stale: stale.clone(),
            staged_bytes: pending.iter().map(|c| c.file.size).sum::<u64>().saturating_add(METADATA_RESERVE),
            ..TransactionPlan::new(OPERATION)
        };
        let root = game.clone();
        let tx = blocking(move || FileTransaction::begin(&root, JOURNAL, plan, None)).await?;
        let mut staged = self.stage(&tx, game, pending, &destinations, &stale, &records, op).await;
        if staged.is_ok() && kind == ContentKind::Mods {
            staged = self.one_copy_each(&tx, game, &destinations, &stale).await;
        }
        if staged.is_ok()
            && let Some(file) = listing
        {
            staged = stage_listing(&tx, game, file, kind, legacy, &renames);
        }
        blocking(move || match staged {
            Ok(()) => tx.apply(ApplyHooks::default()),
            Err(e) => Err(tx.abort(e)),
        })
        .await
    }

    /// The safety net under the plan: no new jar declares a mod id an enabled jar here that stays
    /// already declares (two copies of a mod stop the game).
    async fn one_copy_each(
        &self,
        tx: &FileTransaction,
        game: &Path,
        destinations: &[String],
        stale: &[String],
    ) -> AppResult<()> {
        let (root, digests) = (game.to_path_buf(), self.digests.clone());
        let here = blocking(move || Ok(scan(&root, ContentKind::Mods, &digests))).await?;
        let leaving =
            |relative: &str| stale.iter().chain(destinations).any(|gone| gone.eq_ignore_ascii_case(relative));
        for relative in destinations {
            let Some((id, _)) = self.digests.mod_meta(&tx.stage_path(relative)?) else { continue };
            let id = normalized(&id);
            let taken = here.items.iter().find(|item| {
                item.enabled
                    && !leaving(&item.relative)
                    && item.mod_id.as_deref().is_some_and(|other| normalized(other) == id)
            });
            if let Some(item) = taken {
                return Err(AppError::new(
                    ErrorCode::ContentConflict,
                    format!("{} already has mod {id}: {}", game.display(), item.relative),
                )
                .with_param("name", item.filename.clone()));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn stage(
        &self,
        tx: &FileTransaction,
        game: &Path,
        pending: &[&Candidate],
        destinations: &[String],
        stale: &[String],
        records: &[(String, Record)],
        op: &OperationHandle,
    ) -> AppResult<()> {
        let mut tasks = Vec::new();
        for (candidate, relative) in pending.iter().zip(destinations) {
            let file = &candidate.file;
            tasks.push(
                DownloadTask::new(file.url.clone(), tx.stage_path(relative)?)
                    .size(file.size)
                    .hash(file.hash.clone()),
            );
        }
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        self.deps.downloader.download_all(tasks, false, &progress).await?.into_result()?;
        let staged = tx.stage_path(PROVENANCE)?;
        let document = with_records(read(game), stale, records);
        write_json_file(&staged, &document, 2)
            .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", staged.display())))
    }

    /// The build's files of `args.kind` that are Modrinth projects' — each with its page and, for
    /// enabled mods when asked, the newer version Modrinth offers. Out of
    /// Modrinth's reach the provenance alone names the files and the check fails.
    pub async fn overview(&self, args: &OverviewArgs) -> AppResult<Overview> {
        let target = self.target(&args.key, args.kind)?;
        let (root, cache, kind) = (target.game.clone(), self.digests.clone(), args.kind);
        let scanned = blocking(move || Ok(scan(&root, kind, &cache))).await?;
        let mut named = scanned.clone();
        let identified = named.identify(&self.api, &target.game, &self.identities).await.is_ok();
        let inventory = if identified { named } else { scanned };
        // What the launcher put there through Modrinth: its installs, and its modpack's files.
        let managed: HashSet<String> = PackRecord::read(&target.game, PACK_RECORD)
            .map(|r| r.managed_files.iter().map(|p| p.to_lowercase()).collect())
            .unwrap_or_default();
        let ours = |i: &InstalledItem| {
            i.recorded || managed.contains(&format!("{}/{}", args.kind.folder(), i.filename).to_lowercase())
        };
        let mut notes: Vec<FileNote> =
            inventory.items.iter().filter(|i| i.owned).map(|i| note(i, args.kind, ours(i))).collect();
        self.add_icons(&mut notes).await;
        let updates = match (args.check_updates, identified) {
            (false, _) => None,
            (true, false) => Some(summary(UpdatesStatus::Failed, 0, 0)),
            (true, true) => Some(self.updates(&target, kind, &inventory, &mut notes).await),
        };
        Ok(Overview { notes, updates })
    }

    /// Gives the notes their projects' icons: those not learnt yet are asked for in one go; a
    /// failed ask leaves the notes without (and is asked again next time).
    async fn add_icons(&self, notes: &mut [FileNote]) {
        let missing: Vec<String> = {
            let known = self.icons.lock().unwrap_or_else(|e| e.into_inner());
            let mut ids: Vec<String> = notes
                .iter()
                .map(|n| n.project_id.clone())
                .filter(|id| !id.is_empty() && !known.contains_key(id))
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        if !missing.is_empty() {
            match self.api.projects(&missing).await {
                Ok(projects) => {
                    let mut known = self.icons.lock().unwrap_or_else(|e| e.into_inner());
                    for id in &missing {
                        known.insert(id.clone(), None);
                    }
                    for project in &projects {
                        known.insert(text(project, "id"), icon_of(project));
                    }
                }
                Err(e) => tracing::warn!("No project icons from Modrinth: {}", e.detail),
            }
        }
        let known = self.icons.lock().unwrap_or_else(|e| e.into_inner());
        for note in notes {
            note.icon_url = known.get(&note.project_id).cloned().flatten();
        }
    }

    /// Marks the notes of enabled owned files Modrinth has a newer fitting file for: mods for the
    /// build's loader, resource packs Minecraft's, shaders Iris' (where the launcher selects them).
    async fn updates(
        &self,
        target: &Target,
        kind: ContentKind,
        inventory: &Inventory,
        notes: &mut [FileNote],
    ) -> UpdateSummary {
        let eligible: Vec<&InstalledItem> =
            inventory.items.iter().filter(|i| i.enabled && i.owned && i.sha512.is_some()).collect();
        if eligible.is_empty() {
            return summary(UpdatesStatus::NoEnabled, 0, 0);
        }
        let hashes: Vec<String> = eligible.iter().filter_map(|i| i.sha512.clone()).collect();
        let loaders: Vec<&str> = match kind {
            ContentKind::Mods => target.loader.map(super::catalog::run_by).unwrap_or_default(),
            ContentKind::ResourcePacks => vec!["minecraft"],
            ContentKind::ShaderPacks => vec!["iris"],
        };
        let games: Vec<&str> = target.game_version.as_deref().into_iter().collect();
        let Ok(answer) = self.api.latest_versions(&hashes, &loaders, &games).await else {
            return summary(UpdatesStatus::Failed, 0, 0);
        };
        let game_version = target.game_version.as_deref();
        let (mut found, mut unchecked) = (Vec::new(), 0);
        for item in eligible {
            let Some(candidate) = item.sha512.as_ref().and_then(|h| answer.get(h)) else {
                unchecked += 1;
                continue;
            };
            if text(candidate, "project_id") != item.project_id.clone().unwrap_or_default() {
                return summary(UpdatesStatus::Failed, 0, 0);
            }
            let id = text(candidate, "id");
            if item.version_id.as_deref() == Some(id.as_str())
                || !version_fits(candidate, target.loader, game_version)
                || primary_file(candidate).is_none()
            {
                continue;
            }
            let current = match (&item.version, &item.version_id) {
                (Some(v), _) => Some(v.clone()),
                (None, Some(id)) => self.api.version(id).await.ok(),
                (None, None) => None,
            };
            if current.as_ref().is_none_or(|c| newer(candidate, c)) {
                let number = text(candidate, "version_number");
                found.push((
                    file_name(&item.relative),
                    NewerVersion { version_id: id, version_number: number },
                ));
            }
        }
        for (file, newer) in &found {
            if let Some(note) = notes.iter_mut().find(|n| &n.file == file) {
                note.update = Some(newer.clone());
            }
        }
        let status = if !found.is_empty() {
            UpdatesStatus::Available
        } else if unchecked > 0 {
            UpdatesStatus::Unchecked
        } else {
            UpdatesStatus::Current
        };
        summary(status, found.len(), unchecked)
    }

    /// A page of Modrinth's modpacks.
    pub async fn packs(&self, args: &PacksArgs) -> AppResult<SearchPage> {
        let facets = json!([["project_type:modpack"]]).to_string();
        let raw = self.api.search(args.query.trim(), &facets, args.offset, PACKS_LIMIT).await?;
        Ok(search_page(&raw, args.offset, PACKS_LIMIT))
    }

    /// The versions of a modpack that carry a `.mrpack`, newest first.
    pub async fn pack_versions(&self, args: &PackArgs) -> AppResult<Vec<PackVersion>> {
        let raw = self.api.project_versions(&args.project_id, None, None).await?;
        let strings = |v: &Value, key: &str| -> Vec<String> {
            v.get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        };
        Ok(raw
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| pack_download(v).is_some())
            .map(|v| PackVersion {
                id: text(v, "id"),
                version_number: text(v, "version_number"),
                game_versions: strings(v, "game_versions"),
                loaders: strings(v, "loaders"),
            })
            .collect())
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

    /// Updates build `args.key`, installed from a Modrinth modpack, to another version of it.
    pub async fn update_pack(&self, args: &PackUpdateArgs) -> AppResult<PackUpdated> {
        flow::update_pack(&self.packs, &PACKS, self, args).await
    }

    /// Version `version_id` of pack `project_id`: its `.mrpack` downloaded to `archive` and read.
    async fn fetch_pack(
        &self,
        project_id: &str,
        version_id: &str,
        archive: &Path,
        op: &OperationHandle,
    ) -> AppResult<Fetched> {
        let version = self.api.version(version_id).await?;
        if text(&version, "project_id") != project_id {
            return Err(AppError::new(ErrorCode::InvalidInput, "the version is another project's"));
        }
        let file = pack_download(&version).ok_or_else(|| {
            AppError::new(ErrorCode::NoFileFound, "the version has no .mrpack")
                .with_param("name", text(&version, "name"))
        })?;
        let task = DownloadTask::new(file.url.clone(), archive.to_path_buf())
            .size(file.size)
            .hash(file.hash.clone());
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        self.deps.downloader.download_all(vec![task], false, &progress).await?.into_result()?;
        let path = archive.to_path_buf();
        let pack = blocking(move || {
            let file = fs::File::open(&path).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
            pack::read(file)
        })
        .await?;
        let meta = PackVersionMeta {
            project_id: project_id.to_string(),
            version_id: text(&version, "id"),
            version_number: text(&version, "version_number"),
        };
        Ok(Fetched { pack, meta, skipped: Vec::new() })
    }
}

impl PackSource for ModrinthService {
    fn fetch<'a>(
        &'a self,
        project: &'a str,
        version: &'a str,
        _game: &'a Path,
        archive: &'a Path,
        op: &'a OperationHandle,
        // Modrinth holds nothing back.
        _held: &'a HeldChoice,
    ) -> PackFuture<'a, Fetched> {
        Box::pin(self.fetch_pack(project, version, archive, op))
    }

    fn versions<'a>(&'a self, project: &'a str) -> PackFuture<'a, Vec<PackVersion>> {
        Box::pin(async move { self.pack_versions(&PackArgs { project_id: project.to_string() }).await })
    }

    fn players<'a>(&'a self, game: &'a Path, managed: &'a [String]) -> PackFuture<'a, HashSet<String>> {
        let (root, managed, digests) = (game.to_path_buf(), managed.to_vec(), self.digests.clone());
        Box::pin(blocking(move || Ok(player_projects(&root, &managed, &digests))))
    }
}

/// The Modrinth projects of the enabled mods the player put in `game` (not the pack's `managed`
/// files): a pack update brings no second copy of them.
fn player_projects(game: &Path, managed: &[String], digests: &DigestCache) -> HashSet<String> {
    let managed: HashSet<String> = managed.iter().map(|p| p.to_lowercase()).collect();
    scan(game, ContentKind::Mods, digests)
        .items
        .into_iter()
        .filter(|i| i.owned && i.enabled && !managed.contains(&i.relative.to_lowercase()))
        .filter_map(|i| i.project_id)
        .collect()
}

impl ContentProvider for ModrinthService {
    fn search(&self, args: SearchArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { ModrinthService::search(self, &args).await })
    }

    fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
        Box::pin(async move { ModrinthService::plan(self, &args).await })
    }

    fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
        Box::pin(async move { ModrinthService::install(self, &args).await })
    }

    fn overview(&self, args: OverviewArgs) -> ProviderFuture<'_, Overview> {
        Box::pin(async move { ModrinthService::overview(self, &args).await })
    }

    fn modpacks(&self, args: PacksArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { ModrinthService::packs(self, &args).await })
    }

    fn modpack_versions(&self, args: PackArgs) -> ProviderFuture<'_, Vec<PackVersion>> {
        Box::pin(async move { ModrinthService::pack_versions(self, &args).await })
    }

    fn install_modpack(&self, args: PackInstallArgs) -> ProviderFuture<'_, PackInstalled> {
        Box::pin(async move { ModrinthService::install_pack(self, &args).await })
    }

    fn modpack_builds(&self) -> ProviderFuture<'_, Vec<ModpackBuild>> {
        Box::pin(async move { ModrinthService::modpack_builds(self).await })
    }

    fn update_modpack(&self, args: PackUpdateArgs) -> ProviderFuture<'_, PackUpdated> {
        Box::pin(async move { ModrinthService::update_pack(self, &args).await })
    }
}
