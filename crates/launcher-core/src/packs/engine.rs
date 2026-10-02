//! Installs a modpack as a new build and updates such a build to another version of its pack —
//! for any provider: the provider downloads the pack's archive and reads it into a `Pack`, the
//! engine puts it in place. An update keeps what the player made their own (see `choose_update`)
//! and runs as one transaction whose commit step points the build at the new version; a crash
//! midway is finished or undone the next time the build is met (`recover`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind, Text};
use serde_json::{Value, json};

use super::{Override, Pack, PackFile, PackRecord, extract, override_sha1s};
use crate::content::held::take_copy;
use crate::content::inventory::{MetadataCache, scan_mods};
use crate::content::jar::inspect_mod_jar;
use crate::feedback::{FeedbackService, OperationHandle};
use crate::launch::options::game_dir;
use crate::loaders::{ComponentSource, ComponentSpec};
use crate::lock::Coordinator;
use crate::minecraft::InstallProgress;
use crate::minecraft::install::InstalledVersion;
use crate::net::downloader::{DownloadProgress, DownloadTask, Downloader, HashKind, hash_file};
use crate::net::preflight::{SpaceRequest, preflight};
use crate::storage::journal::SyncJournal;
use crate::storage::json::write_json_file;
use crate::storage::transaction::{ApplyHooks, FileTransaction, TransactionPlan};
use crate::storage::versions::{Build, VersionStore};

/// Room for the record beside the files.
const METADATA_RESERVE: u64 = 4 * 1024 * 1024;

/// What tells one provider's modpack builds from another's.
#[derive(Debug, Clone, Copy)]
pub struct PackKind {
    /// The provider's name, as messages name the source.
    pub provider: &'static str,
    /// The build's record of its pack (`.launcher/<provider>-pack.json`).
    pub record: &'static str,
    /// The update's journal (reserved in a build: packs cannot write it).
    pub journal: &'static str,
    /// Where the pack's archive is downloaded to in the build while it is read (reserved too).
    pub archive: &'static str,
    /// Held on the build's folder while a pack fills or updates it.
    pub lease: &'static str,
    pub install_operation: &'static str,
    pub update_operation: &'static str,
    /// What identifies an update's commit step when it resumes after a crash.
    pub commit_prefix: &'static str,
    /// The build's options that name the pack's project and version.
    pub project_option: &'static str,
    pub version_option: &'static str,
}

/// The launcher services a pack needs.
pub struct PackDeps {
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub downloader: Arc<Downloader>,
    /// Installs a pack's Minecraft version or loader.
    pub components: Arc<dyn ComponentSource>,
    /// The default GPU mode of new builds.
    pub gpu_mode: Arc<dyn Fn() -> String + Send + Sync>,
    /// Whether build `key`'s game runs.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

/// The pack version a build gets.
#[derive(Debug, Clone)]
pub struct PackVersionMeta {
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
}

/// What an update did: the version it went to, and the copies of the player's files it saved (in
/// `folder`).
#[derive(Debug, Clone)]
pub struct Swapped {
    pub number: String,
    pub backups: Vec<String>,
    pub folder: String,
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

fn spec_of(pack: &Pack) -> ComponentSpec {
    match &pack.loader {
        Some((kind, version)) => ComponentSpec::loader(*kind, &pack.minecraft, version),
        None => ComponentSpec::vanilla(&pack.minecraft),
    }
}

fn task_of(file: &PackFile, dest: PathBuf) -> DownloadTask {
    let task = DownloadTask::new(file.url.clone(), dest).size(file.size).hash(file.hash.clone());
    match &file.credential {
        Some(credential) => task.credential(credential.clone()),
        None => task,
    }
}

/// Puts each of `files` at its destination, with progress: a copy the user has is taken, the
/// rest downloaded.
async fn fetch_files(
    deps: &PackDeps,
    files: Vec<(PackFile, PathBuf)>,
    op: &OperationHandle,
) -> AppResult<()> {
    let (copies, downloads): (Vec<_>, Vec<_>) = files.into_iter().partition(|(f, _)| f.local.is_some());
    blocking(move || {
        copies.iter().try_for_each(|(file, dest)| match &file.local {
            Some(from) => take_copy(from, dest, file.size, &file.hash, &file.path),
            None => Ok(()),
        })
    })
    .await?;
    let tasks = downloads.iter().map(|(file, dest)| task_of(file, dest.clone())).collect();
    let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
    deps.downloader.download_all(tasks, false, &progress).await?.into_result()?;
    Ok(())
}

/// Fills the new build folder `game` (claimed as `id`) with `pack` read from `archive` — its
/// Minecraft or loader, then its files — and registers the build `name` last.
#[allow(clippy::too_many_arguments)]
pub async fn fill(
    deps: &PackDeps,
    kind: &PackKind,
    game: &Path,
    id: &str,
    name: &str,
    meta: &PackVersionMeta,
    icon_url: Option<String>,
    archive: &Path,
    pack: &Pack,
    op: &OperationHandle,
) -> AppResult<Build> {
    let spec = spec_of(pack);
    let progress = |p: InstallProgress| {
        let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
        op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
    };
    let installed = deps.components.install(&spec, &progress).await?;
    op.update(Some(Text::key("modpack_installing").param("name", name)), Some(0.0), Some(1.0));
    let record = record_of(pack, meta, &installed, &spec);
    place_pack(deps, kind, game, archive, pack, &record, op).await?;
    let mut build = Build::new(name);
    build.version = Some(pack.minecraft.clone());
    build.loader = Some(installed.id.clone());
    build.client = Some(spec.kind.display_name().to_string());
    build.loader_version = spec.loader_version.clone();
    build.image = icon_url;
    build.description = pack.summary.clone();
    build.options.insert("gpuMode".into(), json!((deps.gpu_mode)()));
    if let Some(java) = &installed.java {
        build.options.insert("executablePath".into(), json!(java.to_string_lossy()));
    }
    build.options.insert(kind.project_option.into(), json!(meta.project_id));
    build.options.insert(kind.version_option.into(), json!(meta.version_id));
    deps.versions.create_in(&mut build, id)?;
    Ok(build)
}

/// Puts the pack's files straight into its new build folder, with progress: the folder holds
/// nothing to keep and a failure takes all of it away, so no per-file journal is needed (its
/// cost grows with the square of the file count).
async fn place_pack(
    deps: &PackDeps,
    kind: &PackKind,
    game: &Path,
    archive: &Path,
    pack: &Pack,
    record: &PackRecord,
    op: &OperationHandle,
) -> AppResult<()> {
    let overrides = pack.overrides.iter().map(|o| o.size).fold(0, u64::saturating_add);
    let bytes = pack.files.iter().map(|f| f.size).fold(overrides, u64::saturating_add);
    preflight(&[SpaceRequest {
        dir: game.to_path_buf(),
        bytes: bytes.saturating_add(METADATA_RESERVE),
        label: kind.install_operation.into(),
    }])?;
    let targets: Vec<(Override, PathBuf)> =
        pack.overrides.iter().map(|o| (o.clone(), game.join(&o.path))).collect();
    let (source, report, total) = (archive.to_path_buf(), op.progress_reporter(), overrides.max(1) as f64);
    blocking(move || extract(&source, &targets, &|done| report(done as f64, total))).await?;
    fetch_files(deps, pack.files.iter().map(|f| (f.clone(), game.join(&f.path))).collect(), op).await?;
    let mut record = record.clone();
    let (root, paths) = (game.to_path_buf(), record.managed_files.clone());
    record.hashes = blocking(move || Ok(sha1_of_each(&root, &paths))).await?;
    let record = serde_json::to_value(&record).map_err(|e| AppError::internal(e.to_string()))?;
    let path = game.join(kind.record);
    let io_err = |e: std::io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    write_json_file(&path, &record, 2).map_err(io_err)
}

/// The builds installed from this kind of pack, each with its record. An update a crash cut short
/// is finished or undone first (unless the build's game runs or the build is busy): listing the
/// builds is the next operation that meets it, and its record may name a version the build does
/// not run.
pub async fn owned(deps: &PackDeps, kind: &PackKind) -> AppResult<Vec<(Build, PackRecord)>> {
    let (versions, instances, kind) = (deps.versions.clone(), deps.instances.clone(), *kind);
    let running = deps.running.clone();
    blocking(move || {
        Ok(versions
            .list()
            .into_iter()
            .filter_map(|b| {
                let game = game_dir(&b, versions.minecraft_dir());
                if !running(&b.key)
                    && let Ok(_lease) = instances.try_acquire(&game, kind.lease)
                    && let Err(e) = recover(&versions, &kind, &b.key, &game)
                {
                    tracing::warn!("the interrupted modpack update of {} stays: {}", b.key, e.detail);
                }
                PackRecord::read(&game, kind.record).map(|r| (b, r))
            })
            .collect())
    })
    .await
}

/// Updates the build `build` (its folder `game`, its record `old`) to `pack` read from `archive`:
/// the pack's files and overrides replace theirs, the files only the old version had go, the
/// loader is installed first, and the build takes the new version in the transaction's commit
/// step. A failure leaves all as it was. `players`: the provider's projects of the mods the player
/// put there themselves (a pack mod of one of them is not added again).
#[allow(clippy::too_many_arguments)]
pub async fn update(
    deps: &PackDeps,
    kind: &PackKind,
    build: &Build,
    game: &Path,
    old: &PackRecord,
    meta: &PackVersionMeta,
    archive: &Path,
    pack: &Pack,
    players: HashSet<String>,
    op: &OperationHandle,
) -> AppResult<Swapped> {
    let spec = spec_of(pack);
    let progress = |p: InstallProgress| {
        let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
        op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
    };
    let installed = deps.components.install(&spec, &progress).await?;
    op.update(Some(Text::key("modpack_updating").param("name", &build.name)), Some(0.0), Some(1.0));
    let mut new_record = record_of(pack, meta, &installed, &spec);
    let (root, old_record, new_pack, source) =
        (game.to_path_buf(), old.clone(), pack.clone(), archive.to_path_buf());
    let choice = blocking(move || {
        // What the new version brings, by SHA-1: the pack's word for its files, the archive's
        // bytes for its overrides.
        let mut incoming: HashMap<String, String> =
            override_sha1s(&source, &new_pack.overrides)?.into_iter().map(|(p, h)| (lower(&p), h)).collect();
        incoming.extend(new_pack.files.iter().filter_map(|f| Some((lower(&f.path), f.sha1.clone()?))));
        Ok(choose_update(&root, &old_record, &new_pack, &players, &incoming))
    })
    .await?;
    new_record.managed_files = choice.managed.clone();
    // The player's copies of what the update replaces go to a folder of their own first.
    let folder = backup_folder(game);
    let backups: Vec<(String, String)> =
        choice.backups.iter().map(|p| (p.clone(), format!("{folder}/{p}"))).collect();
    let backup_bytes: u64 =
        backups.iter().filter_map(|(p, _)| fs::metadata(game.join(p)).ok()).map(|m| m.len()).sum();
    let plan = TransactionPlan {
        replacements: choice
            .files
            .iter()
            .map(|f| f.path.clone())
            .chain(choice.overrides.iter().map(|o| o.path.clone()))
            .chain(backups.iter().map(|(_, saved)| saved.clone()))
            .chain([kind.record.to_string()])
            .collect(),
        stale: choice.stale.clone(),
        staged_bytes: choice
            .files
            .iter()
            .map(|f| f.size)
            .chain(choice.overrides.iter().map(|o| o.size))
            .chain([backup_bytes])
            .fold(METADATA_RESERVE, u64::saturating_add),
        commit_key: Some(commit_key(kind, &build.key)),
        ..TransactionPlan::new(kind.update_operation)
    };
    let commit = committer(&deps.versions, kind, &build.key, game);
    let (root, recover_with, journal) = (game.to_path_buf(), commit.clone(), kind.journal);
    let tx = blocking(move || FileTransaction::begin(&root, journal, plan, Some(&*recover_with))).await?;
    let staged =
        stage_update(deps, kind, &tx, game, archive, &choice, &backups, new_record.clone(), op).await;
    let loader = pack.loader.as_ref().map(|(kind, _)| *kind);
    let (root, mods, stale) = (game.to_path_buf(), plan_mods(&choice), choice.stale.clone());
    blocking(move || {
        // A mod the player has that no record names: its jar's id tells, as for content (every
        // jar there is read, so not on the async workers).
        match staged.and_then(|()| one_copy_each(&tx, &root, loader, &mods, &stale)) {
            Ok(()) => tx.apply(ApplyHooks { commit: Some(&*commit), ..ApplyHooks::default() }),
            Err(e) => Err(tx.abort(e)),
        }
    })
    .await?;
    for (path, saved) in &backups {
        tracing::info!("The update replaced {path} of {}; the copy there is in {saved}", build.name);
    }
    Ok(Swapped {
        number: new_record.version_number,
        backups: backups.into_iter().map(|(_, saved)| saved).collect(),
        folder,
    })
}

/// Stages the copies of the player's files in `backups` (path, where it is saved), the
/// overrides and files `choice` puts, and the record with each file's hash, with progress.
#[allow(clippy::too_many_arguments)]
async fn stage_update(
    deps: &PackDeps,
    kind: &PackKind,
    tx: &FileTransaction,
    game: &Path,
    archive: &Path,
    choice: &UpdateChoice,
    backups: &[(String, String)],
    mut record: PackRecord,
    op: &OperationHandle,
) -> AppResult<()> {
    let copies: Vec<(PathBuf, PathBuf)> = backups
        .iter()
        .map(|(path, saved)| Ok((game.join(path), tx.stage_path(saved)?)))
        .collect::<AppResult<_>>()?;
    blocking(move || {
        for (from, to) in &copies {
            fs::copy(from, to)
                .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", from.display())))?;
        }
        Ok(())
    })
    .await?;
    let targets: Vec<(Override, PathBuf)> = choice
        .overrides
        .iter()
        .map(|o| Ok((o.clone(), tx.stage_path(&o.path)?)))
        .collect::<AppResult<_>>()?;
    let total = choice.overrides.iter().map(|o| o.size).fold(0, u64::saturating_add).max(1) as f64;
    let (source, report) = (archive.to_path_buf(), op.progress_reporter());
    blocking(move || extract(&source, &targets, &|done| report(done as f64, total))).await?;
    let files = choice
        .files
        .iter()
        .map(|f| Ok((f.clone(), tx.stage_path(&f.path)?)))
        .collect::<AppResult<Vec<_>>>()?;
    fetch_files(deps, files, op).await?;
    let staged: Vec<(String, PathBuf)> = choice
        .files
        .iter()
        .map(|f| f.path.clone())
        .chain(choice.overrides.iter().map(|o| o.path.clone()))
        .map(|path| Ok((path.clone(), tx.stage_path(&path)?)))
        .collect::<AppResult<_>>()?;
    let mut hashes = choice.unchanged.clone();
    hashes.extend(
        blocking(move || {
            Ok(staged
                .into_iter()
                .filter_map(|(path, file)| hash_file(&file, HashKind::Sha1).ok().map(|h| (path, h)))
                .collect::<Vec<_>>())
        })
        .await?,
    );
    record.hashes = hashes;
    let staged = tx.stage_path(kind.record)?;
    let value = serde_json::to_value(&record).map_err(|e| AppError::internal(e.to_string()))?;
    write_json_file(&staged, &value, 2)
        .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", staged.display())))
}

/// A mod's id as two copies of one mod share it.
fn mod_key(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('-', "_")
}

/// The safety net under an update: no new jar declares a mod id an enabled jar here that stays
/// already declares (two copies of a mod stop the game).
fn one_copy_each(
    tx: &FileTransaction,
    game: &Path,
    loader: Option<LoaderKind>,
    mods: &[String],
    stale: &[String],
) -> AppResult<()> {
    let here = scan_mods(game, loader, &MetadataCache::default(), &[]);
    let leaving = |file: &str| {
        let relative = format!("mods/{file}");
        stale.iter().chain(mods).any(|gone| gone.eq_ignore_ascii_case(&relative))
    };
    for relative in mods {
        let Ok(descriptor) = inspect_mod_jar(&tx.stage_path(relative)?, loader) else { continue };
        let id = mod_key(&descriptor.mod_id);
        let taken = here.iter().find(|entry| {
            entry.item.enabled
                && !leaving(&entry.item.file)
                && entry.item.mod_id.as_deref().is_some_and(|other| mod_key(other) == id)
        });
        if let Some(entry) = taken {
            return Err(AppError::new(
                ErrorCode::ContentConflict,
                format!("{} already has mod {id}: {}", game.display(), entry.item.file),
            )
            .with_param("name", entry.item.filename.clone()));
        }
    }
    Ok(())
}

/// Files the game keeps the player's choices in: once there, an update never replaces or deletes
/// them (the game rewrites them itself, so a hash cannot tell).
const PLAYER_SETTINGS: [&str; 4] = ["options.txt", "optionsof.txt", "optionsshaders.txt", "servers.dat"];

/// Where an update saves the player's copies of the files it replaces (in the build).
const PACK_BACKUPS: &str = ".launcher/pack-backups";

/// A new folder of `PACK_BACKUPS` for this update: its time, and a number when it is taken.
fn backup_folder(game: &Path) -> String {
    let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    (1..)
        .map(
            |n| {
                if n == 1 { format!("{PACK_BACKUPS}/{stamp}") } else { format!("{PACK_BACKUPS}/{stamp}-{n}") }
            },
        )
        .find(|folder| !game.join(folder).exists())
        .unwrap_or_default()
}

/// A path as the record compares paths: `/`-separated, lower case.
fn lower(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// What a modpack update puts and takes away.
#[derive(Debug, Clone, Default)]
struct UpdateChoice {
    /// The files to download and the overrides to take from the archive.
    files: Vec<PackFile>,
    overrides: Vec<Override>,
    /// The pack's files once the update is done.
    managed: Vec<String>,
    /// Pack files that stay as they are, by the SHA-1 the record keeps for them.
    unchanged: BTreeMap<String, String>,
    /// Files there that the update replaces: their copies are saved first.
    backups: Vec<String>,
    /// The old version's files to remove.
    stale: Vec<String>,
}

/// How a file of the new version meets what is at its place (`choose_update`).
#[derive(Debug, PartialEq, Eq)]
enum Meet {
    /// Not the pack's to touch: a world there, a settings file there, a file turned off.
    Theirs,
    /// Nothing there: it goes in.
    Absent,
    /// There already as the new version has it; the record keeps this SHA-1.
    Same(String),
    /// Untouched since the old version put it: the new version replaces it.
    Untouched,
    /// Changed since, while the new version brings it as the old one did: it stays, and the record
    /// keeps the old version's SHA-1 so the change still shows next time.
    Kept(String),
    /// Changed since (or not the old version's), and the new version brings another: it goes in
    /// after the copy there is saved.
    Conflict,
}

/// The world folder `path` is in (`saves/<world>`, spelled as in `path`: names on Linux keep
/// their case).
fn world_dir(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let (saves, world) = (parts.next()?, parts.next()?);
    saves.eq_ignore_ascii_case("saves").then(|| format!("{saves}/{world}"))
}

/// The files `choice` puts in `mods/`.
fn plan_mods(choice: &UpdateChoice) -> Vec<String> {
    let files = choice.files.iter().map(|f| &f.path).chain(choice.overrides.iter().map(|o| &o.path));
    files.filter(|p| p.replace('\\', "/").to_lowercase().starts_with("mods/")).cloned().collect()
}

/// The SHA-1 of each of `paths` under `game` that is a file.
fn sha1_of_each(game: &Path, paths: &[String]) -> BTreeMap<String, String> {
    paths
        .iter()
        .filter_map(|p| hash_file(&game.join(p), HashKind::Sha1).ok().map(|h| (p.clone(), h)))
        .collect()
}

/// What updating from `old` to `pack` changes in `game`. A file there is weighed against what the
/// old version put (`old.hashes`) and what the new one brings (`incoming`, SHA-1 by `lower` path) —
/// see `Meet`: the player's changes to a file the pack leaves as it was stay; a file the pack
/// changes goes in, after the copy there is saved when it is not the old version's. Never touched:
/// a world already there (`saves/<world>`), the game's settings files once there, a file the player
/// turned off (`<path>.disabled`). A pack mod whose project the player already has as another file
/// is left out. The old version's files the new one lacks go unless they changed since.
fn choose_update(
    game: &Path,
    old: &PackRecord,
    pack: &Pack,
    players: &HashSet<String>,
    incoming: &HashMap<String, String>,
) -> UpdateChoice {
    let old_managed: HashSet<String> = old.managed_files.iter().map(|p| lower(p)).collect();
    let recorded: HashMap<String, String> =
        old.hashes.iter().map(|(p, h)| (lower(p), h.to_ascii_lowercase())).collect();
    // Before schema 3 a record keeps no hashes: nothing tells the player's changes apart.
    let hashed = old.schema_version >= 3;
    let sha1 = |p: &str| hash_file(&game.join(p), HashKind::Sha1).ok();
    let settings = |p: &str| PLAYER_SETTINGS.contains(&lower(p).as_str());
    let meet = |path: &str, expected: Option<&crate::net::downloader::ExpectedHash>| -> Meet {
        let there = game.join(path);
        let world = world_dir(path).is_some_and(|w| game.join(w).is_dir());
        if world || (there.is_file() && settings(path)) {
            return Meet::Theirs;
        }
        if !there.is_file() {
            let off = game.join(format!("{path}.disabled")).is_file();
            return if off { Meet::Theirs } else { Meet::Absent };
        }
        let Some(now) = sha1(path) else { return Meet::Conflict };
        let key = lower(path);
        let new = incoming.get(&key);
        let same = match (new, expected) {
            (Some(new), _) => *new == now,
            (None, Some(e)) => hash_file(&there, e.kind).is_ok_and(|h| h.eq_ignore_ascii_case(&e.hex)),
            (None, None) => false,
        };
        if same {
            return Meet::Same(now);
        }
        match recorded.get(&key).filter(|_| hashed && old_managed.contains(&key)) {
            Some(before) if *before == now => Meet::Untouched,
            Some(before) if new == Some(before) => Meet::Kept(before.clone()),
            _ => Meet::Conflict,
        }
    };
    let mut choice = UpdateChoice::default();
    for file in &pack.files {
        let met = meet(&file.path, Some(&file.hash));
        match met {
            Meet::Theirs => continue,
            Meet::Same(sha1) | Meet::Kept(sha1) => {
                choice.unchanged.insert(file.path.clone(), sha1);
            }
            Meet::Absent => {
                let a_mod = lower(&file.path).starts_with("mods/");
                if a_mod && file.project.as_ref().is_some_and(|p| players.contains(p)) {
                    continue;
                }
                choice.files.push(file.clone());
            }
            Meet::Untouched | Meet::Conflict => {
                if met == Meet::Conflict {
                    choice.backups.push(file.path.clone());
                }
                choice.files.push(file.clone());
            }
        }
        choice.managed.push(file.path.clone());
    }
    for over in &pack.overrides {
        let met = meet(&over.path, None);
        match met {
            Meet::Theirs => continue,
            Meet::Same(sha1) | Meet::Kept(sha1) => {
                choice.unchanged.insert(over.path.clone(), sha1);
            }
            Meet::Absent | Meet::Untouched | Meet::Conflict => {
                if met == Meet::Conflict {
                    choice.backups.push(over.path.clone());
                }
                choice.overrides.push(over.clone());
            }
        }
        choice.managed.push(over.path.clone());
    }
    let kept: HashSet<String> = choice.managed.iter().map(|p| lower(p)).collect();
    choice.stale = old
        .managed_files
        .iter()
        .filter(|p| !kept.contains(&lower(p)) && game.join(p).is_file())
        .filter(|p| world_dir(p).is_none() && !settings(p))
        .filter(|p| !hashed || recorded.get(&lower(p)).is_some_and(|h| sha1(p).as_ref() == Some(h)))
        .cloned()
        .collect();
    choice
}

/// The record of `pack`'s version `meta` for a build that runs `installed` (schema 3; the files'
/// hashes are added once they are in place).
fn record_of(
    pack: &Pack,
    meta: &PackVersionMeta,
    installed: &InstalledVersion,
    spec: &ComponentSpec,
) -> PackRecord {
    PackRecord {
        schema_version: 3,
        project_id: meta.project_id.clone(),
        version_id: meta.version_id.clone(),
        version_number: meta.version_number.clone(),
        minecraft: Some(pack.minecraft.clone()),
        loader: Some(installed.id.clone()),
        loader_version: spec.loader_version.clone(),
        client: Some(spec.kind.display_name().to_string()),
        java: installed.java.as_ref().map(|j| j.to_string_lossy().into_owned()),
        managed_files: PackRecord::managed(pack),
        hashes: BTreeMap::new(),
    }
}

/// Finishes the commit of an update of build `key` a crash interrupted, or undoes its file swap.
pub fn recover(versions: &Arc<VersionStore>, kind: &PackKind, key: &str, game: &Path) -> AppResult<()> {
    let commit = committer(versions, kind, key, game);
    SyncJournal::new(game, kind.journal).recover(Some(&*commit), Some(&commit_key(kind, key))).map(|_| ())
}

/// What identifies a modpack update's commit step when it resumes after a crash.
fn commit_key(kind: &PackKind, key: &str) -> String {
    format!("{}:{key}", kind.commit_prefix)
}

/// The update's commit step, and its recovery after a crash: build `key` takes what the pack
/// record on disk says its files run on.
fn committer(
    versions: &Arc<VersionStore>,
    kind: &PackKind,
    key: &str,
    game: &Path,
) -> Arc<dyn Fn() -> AppResult<()> + Send + Sync> {
    let (versions, kind, key, game) = (versions.clone(), *kind, key.to_string(), game.to_path_buf());
    Arc::new(move || {
        let record = PackRecord::read(&game, kind.record)
            .ok_or_else(|| AppError::new(ErrorCode::Io, "the modpack's record is missing"))?;
        let mut build = versions
            .get(&key)
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "the build is gone").with_param("key", &key))?;
        // Another Minecraft may need another Java; the same keeps the one the build has.
        let new_minecraft = record.minecraft.is_some() && record.minecraft != build.version;
        let no_java = build.options.get("executablePath").and_then(Value::as_str).is_none_or(str::is_empty);
        if let Some(minecraft) = record.minecraft {
            build.version = Some(minecraft);
        }
        if let Some(loader) = record.loader {
            build.loader = Some(loader);
            build.loader_version = record.loader_version;
        }
        if let Some(client) = record.client {
            build.client = Some(client);
        }
        if let Some(java) = record.java.filter(|_| new_minecraft || no_java) {
            build.options.insert("executablePath".into(), json!(java));
        }
        build.options.insert(kind.version_option.into(), json!(record.version_id));
        versions.save(&mut build)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_folders_keep_their_case() {
        assert_eq!(world_dir("saves/Sky/level.dat").as_deref(), Some("saves/Sky"));
        assert_eq!(world_dir("SAVES\\New World/region/r.0.0.mca").as_deref(), Some("SAVES/New World"));
        assert_eq!(world_dir("config/saves/x.txt"), None);
        assert_eq!(world_dir("saves/"), None);
    }
}
