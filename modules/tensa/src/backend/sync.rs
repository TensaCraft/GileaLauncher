//! A server build brought in line with the server: the build the catalog
//! names for it, a new loader when the server changed it, the server's forced settings, and its
//! files — downloaded, swapped in and stale ones deleted in one transaction whose commit saves the
//! build. Anything that fails leaves the files and the build as they were.

use std::fs;
use std::path::Path;

use launcher_core::feedback::OperationHandle;
use launcher_core::launch::options::game_dir;
use launcher_core::loaders::{ComponentSource, ComponentSpec};
use launcher_core::minecraft::InstallProgress;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, Downloader};
use launcher_core::storage::journal::{SyncJournal, contained};
use launcher_core::storage::transaction::{ApplyHooks, FileTransaction, TransactionPlan};
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind, Text};
use serde_json::{Value, json};

use super::api::TensaApi;
use super::identity::{self, CLIENT, JOURNAL};
use super::pack::{Pack, find};
use super::plan::{SyncPlan, prepare};
use super::profile;

/// The journal's name for a server build's sync.
pub const OPERATION: &str = "tensacraft_sync";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Synced {
    /// The server or its build could not be reached; nothing changed.
    Skipped,
    /// Already as the server has it (its settings may have been refreshed).
    Unchanged,
    /// A new loader or new files.
    Updated,
}

pub struct SyncDeps<'a> {
    pub api: &'a TensaApi,
    pub versions: &'a VersionStore,
    pub components: &'a dyn ComponentSource,
    pub downloader: &'a Downloader,
    /// The build's game is running (a second copy is being started).
    pub running: bool,
}

/// The names the build may be found by in the catalog, the pack id first.
fn candidates(build: &Build) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let options =
        ["tensacraftPackId", "tensacraft_pack_id"].map(|k| build.options.get(k).and_then(Value::as_str));
    let pack_id = identity::pack_id(build);
    let all = [
        pack_id.as_deref(),
        options[0],
        options[1],
        Some(build.id.as_str()),
        Some(build.version_id.as_str()),
        Some(build.name.as_str()),
        build.version.as_deref(),
    ];
    for name in all.into_iter().flatten().map(str::trim).filter(|n| !n.is_empty()) {
        if !names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    }
    names
}

/// The server cannot be asked about `build`: skipped, or an error when the sync was asked for.
fn unreachable(build: &Build, pack: &str, force: bool, why: &str) -> AppResult<Synced> {
    tracing::warn!("Skipping the sync of {}: {why}", build.name);
    if force {
        Err(AppError::new(ErrorCode::Network, format!("the server build {pack} is unavailable: {why}"))
            .with_param("pack", pack))
    } else {
        Ok(Synced::Skipped)
    }
}

/// The commit of a sync of `build`: one per build, so any later sync of it can finish one a crash
/// interrupted.
fn commit_key(build: &Build) -> String {
    format!("tensacraft-build:{}", build.key)
}

/// A sync that crashed while saving the build left its journal committing: its files are in place,
/// and its commit only saved the build, which this sync does again — so it is finished as it is.
fn finish_interrupted_commit(root: &Path) -> AppResult<()> {
    let journal = SyncJournal::new(root, JOURNAL);
    if journal.status().as_deref() != Some("committing") {
        return Ok(());
    }
    let key = journal.read().and_then(|j| j.get("commit_key").and_then(Value::as_str).map(str::to_string));
    tracing::warn!("Finishing an interrupted sync commit in {}", root.display());
    journal.recover(Some(&|| Ok(())), key.as_deref()).map(|_| ())
}

/// Removes the empty folders inside the managed folders, the deepest first (the managed folders
/// themselves stay).
fn remove_empty_folders(root: &Path, managed: &[String]) {
    fn folders(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                found.push(entry.path());
                folders(&entry.path(), found);
            }
        }
    }
    for dir in managed {
        let Ok(path) = contained(root, dir) else { continue };
        let mut found = Vec::new();
        folders(&path, &mut found);
        found.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for folder in found {
            let _ = fs::remove_dir(folder);
        }
    }
}

/// The component `pack` runs.
fn spec_of(pack: &Pack) -> AppResult<ComponentSpec> {
    let invalid = |what: &str| {
        AppError::new(ErrorCode::InvalidInput, format!("the server build {} {what}", pack.id))
            .with_param("pack", &pack.id)
    };
    let minecraft = pack.minecraft.clone().ok_or_else(|| invalid("names no Minecraft version"))?;
    match profile::loader_kind(pack)? {
        LoaderKind::Minecraft => Ok(ComponentSpec::vanilla(&minecraft)),
        kind => {
            let version = pack.loader_version.clone().ok_or_else(|| invalid("names no loader version"))?;
            Ok(ComponentSpec::loader(kind, &minecraft, &version))
        }
    }
}

/// Runs blocking file work (hundreds of renames, the journal) so the async workers keep going: on
/// the multi-thread runtime the worker hands its other tasks on first; elsewhere it just runs.
fn off_the_workers<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// Swaps the planned files in, deletes the stale ones and saves `target` — all or nothing.
async fn apply(
    deps: &SyncDeps<'_>,
    root: &Path,
    plan: &SyncPlan,
    target: &Build,
    op: &OperationHandle,
) -> AppResult<()> {
    let transaction_plan = TransactionPlan {
        replacements: plan.downloads.iter().map(|f| f.relative.clone()).collect(),
        stale: plan.stale.clone(),
        staged_bytes: plan.downloads.iter().filter_map(|f| f.size).sum(),
        commit_key: Some(commit_key(target)),
        ..TransactionPlan::new(OPERATION)
    };
    let commit = || -> AppResult<()> {
        let mut saved = target.clone();
        deps.versions.save(&mut saved)?;
        remove_empty_folders(root, &plan.managed_dirs);
        Ok(())
    };
    let transaction =
        off_the_workers(|| FileTransaction::begin(root, JOURNAL, transaction_plan, Some(&commit)))?;
    let tasks: AppResult<Vec<DownloadTask>> =
        plan.downloads.iter().map(|file| Ok(file.task(transaction.stage_path(&file.relative)?))).collect();
    let tasks = match tasks {
        Ok(tasks) => tasks,
        Err(e) => return Err(transaction.abort(e)),
    };
    let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
    let staged =
        deps.downloader.download_all(tasks, false, &progress).await.and_then(|report| report.into_result());
    if let Err(e) = staged {
        return Err(transaction.abort(e));
    }
    off_the_workers(|| transaction.apply(ApplyHooks { commit: Some(&commit), ..ApplyHooks::default() }))
}

/// Brings `build` in line with the server; `force` downloads every managed file again and makes
/// an unreachable server an error.
pub async fn sync(
    deps: &SyncDeps<'_>,
    build: &Build,
    force: bool,
    op: &OperationHandle,
) -> AppResult<Synced> {
    op.update(Some(Text::key("syncing_files_check")), Some(0.0), Some(100.0));
    let names = candidates(build);
    let shown = names.first().cloned().unwrap_or_else(|| build.name.clone());
    let packs = match deps.api.packs().await {
        Ok(packs) => packs,
        Err(e) => return unreachable(build, &shown, force, &e.detail),
    };
    let Some(pack) = names.iter().find_map(|name| find(&packs, name)) else {
        return unreachable(build, &shown, force, "the catalog no longer lists it");
    };
    let root = game_dir(build, deps.versions.minecraft_dir());
    finish_interrupted_commit(&root)?;
    let plan = match prepare(deps.api, &pack, &root, force).await {
        Ok(plan) => plan,
        Err(e) if e.code == ErrorCode::Network => return unreachable(build, &pack.id, force, &e.detail),
        Err(e) => return Err(e),
    };
    let mut target = build.clone();
    let loader_changed = profile::loader_changed(&target, &pack);
    if deps.running && (loader_changed || plan.has_changes()) {
        return Err(AppError::new(ErrorCode::GameRunning, format!("{} is running", build.name))
            .with_param("version", &build.name));
    }
    if loader_changed {
        let label = [pack.loader.as_deref(), pack.loader_version.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        op.update(Some(Text::key("syncing_loader_update").param("loader", label)), Some(15.0), Some(100.0));
        let spec = spec_of(&pack)?;
        let progress = |p: InstallProgress| op.update(Some(p.status), None, None);
        let installed = deps.components.install(&spec, &progress).await?;
        target.client = Some(CLIENT.to_string());
        target.loader = Some(installed.id);
        target.loader_version = pack.loader_version.clone();
        target.version = pack.minecraft.clone();
        if let Some(java) = installed.java {
            target.options.insert("executablePath".into(), json!(java.to_string_lossy()));
        }
    } else {
        target.loader_version = pack.loader_version.clone().or(target.loader_version.take());
        target.version = pack.minecraft.clone().or(target.version.take());
    }
    profile::merge(&mut target, &pack, false);
    identity::mark(&mut target, &pack.id);
    if plan.has_changes() {
        op.update(Some(Text::key("syncing_files")), Some(25.0), Some(100.0));
        apply(deps, &root, &plan, &target, op).await?;
    } else if target != *build {
        deps.versions.save(&mut target)?;
    }
    Ok(if loader_changed || plan.has_changes() { Synced::Updated } else { Synced::Unchanged })
}

/// A sync can run inside a launch step, whose future is sent between threads.
#[allow(dead_code)]
fn a_sync_is_sendable(deps: &SyncDeps<'_>, build: &Build, op: &OperationHandle) {
    fn sendable<T: Send>(_: &T) {}
    sendable(&sync(deps, build, false, op));
}
