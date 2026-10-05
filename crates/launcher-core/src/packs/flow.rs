//! A provider's modpacks from end to end: the provider says what a version of a pack is
//! (`PackSource`); these do the rest through the engine — a new build from a version, the builds
//! with their packs' newest versions, an update — and tell the user how it went.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use futures_util::stream::{self, StreamExt};
use launcher_shared::provider::{
    HeldFile, ModpackBuild, PackInstallArgs, PackInstalled, PackUpdateArgs, PackUpdated, PackVersion,
    newer_pack_version,
};
use launcher_shared::{AppError, AppResult, ErrorCode, Text};

use super::engine::{self, PackDeps, PackKind, PackVersionMeta, Swapped};
use super::{Pack, PackRecord};
use crate::feedback::{OperationHandle, OperationSpec};
use crate::launch::options::game_dir;
use crate::storage::versions::Build;

/// A provider's answer.
pub type PackFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// A version of a pack as its provider gives it: read from its archive, and what a build records
/// of it.
pub struct Fetched {
    pub pack: Pack,
    pub meta: PackVersionMeta,
    /// Held files left out, as `HeldChoice::skip` asked.
    pub skipped: Vec<HeldFile>,
}

/// What to do with the files a provider holds back (`ErrorCode::ProviderFilesHeld`) that are
/// not found: take some from their alternative elsewhere, leave the rest out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldChoice {
    /// Held files (by SHA-1) taken from their alternative.
    pub replace: Vec<String>,
    /// The ones still missing are left out rather than asked for.
    pub skip: bool,
}

impl HeldChoice {
    pub fn of_install(args: &PackInstallArgs) -> HeldChoice {
        HeldChoice { replace: args.replace_held.clone(), skip: args.skip_held }
    }

    pub fn of_update(args: &PackUpdateArgs) -> HeldChoice {
        HeldChoice { replace: args.replace_held.clone(), skip: args.skip_held }
    }
}

/// What the engine asks of a provider's modpacks.
pub trait PackSource: Send + Sync {
    /// Version `version` of pack `project` for the build folder `game` (a new, empty one for an
    /// install): its archive downloaded to `archive` (with progress on `op`) and read. A version of
    /// another pack is refused.
    fn fetch<'a>(
        &'a self,
        project: &'a str,
        version: &'a str,
        game: &'a Path,
        archive: &'a Path,
        op: &'a OperationHandle,
        held: &'a HeldChoice,
    ) -> PackFuture<'a, Fetched>;

    /// The pack's versions, newest first.
    fn versions<'a>(&'a self, project: &'a str) -> PackFuture<'a, Vec<PackVersion>>;

    /// The provider's projects of the enabled mods the player put in `game` themselves (none of
    /// the pack's files, `managed`): an update adds no second copy of them.
    fn players<'a>(&'a self, game: &'a Path, managed: &'a [String]) -> PackFuture<'a, HashSet<String>>;
}

fn busy() -> AppError {
    AppError::new(ErrorCode::Busy, "another operation runs")
}

/// A new build from a modpack version: its folder is claimed first, then filled — the pack, its
/// Minecraft or loader, its files — and registered last; any failure takes the folder away.
pub async fn install_pack(
    deps: &PackDeps,
    kind: &PackKind,
    source: &dyn PackSource,
    args: &PackInstallArgs,
) -> AppResult<PackInstalled> {
    if deps.feedback.is_busy() {
        return Err(busy());
    }
    let name = args.name.trim().to_string();
    let id = deps.versions.claim_folder(&name)?;
    let game = deps.versions.games_dir().join(&id);
    let spec = OperationSpec::new(Text::key("installation_started"), "install")
        .status(Text::key("modpack_installing").param("name", &name));
    let op = deps.feedback.begin(spec);
    match fill_new(deps, kind, source, &game, &id, &name, args, &op).await {
        Ok((build, skipped)) => {
            op.finish();
            deps.feedback.success(Text::key("version_install_success").param("version", &name));
            Ok(PackInstalled { key: build.key, name, skipped })
        }
        Err(e) => {
            if let Err(gone) = fs::remove_dir_all(&game) {
                tracing::warn!("the failed modpack's folder {} stays: {gone}", game.display());
            }
            op.fail(
                Text::key("version_install_error")
                    .param("client", kind.provider)
                    .param("version", &name)
                    .param("error", e.detail.clone()),
            );
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn fill_new(
    deps: &PackDeps,
    kind: &PackKind,
    source: &dyn PackSource,
    game: &Path,
    id: &str,
    name: &str,
    args: &PackInstallArgs,
    op: &OperationHandle,
) -> AppResult<(Build, Vec<HeldFile>)> {
    let _lease = deps.instances.try_acquire(game, kind.lease)?;
    let archive = game.join(kind.archive);
    let held = HeldChoice::of_install(args);
    let result = match source.fetch(&args.project_id, &args.version_id, game, &archive, op, &held).await {
        Ok(Fetched { pack, meta, skipped }) => {
            engine::fill(deps, kind, game, id, name, &meta, args.icon_url.clone(), &archive, &pack, op)
                .await
                .map(|build| (build, skipped))
        }
        Err(e) => Err(e),
    };
    let _ = fs::remove_file(&archive);
    result
}

/// How many packs' versions are asked at once.
const ASKED_AT_ONCE: usize = 4;

/// The versions of every pack of `projects`, asked a few at a time (one list a pack, never one
/// after another); the last failure, when some pack got no answer.
async fn versions_of(
    source: &dyn PackSource,
    projects: BTreeSet<String>,
) -> (HashMap<String, Vec<PackVersion>>, Option<AppError>) {
    let answers: Vec<(String, AppResult<Vec<PackVersion>>)> = stream::iter(projects)
        .map(|project| async move {
            let answer = source.versions(&project).await;
            (project, answer)
        })
        .buffer_unordered(ASKED_AT_ONCE)
        .collect()
        .await;
    let (mut lists, mut last_error) = (HashMap::new(), None);
    for (project, answer) in answers {
        match answer {
            Ok(list) => {
                lists.insert(project, list);
            }
            Err(e) => last_error = Some(e),
        }
    }
    (lists, last_error)
}

/// The builds installed from this kind of pack, each with its pack's newest version when that is
/// another: one versions list per pack; a pack the provider does not answer for is left out.
pub async fn pack_builds(
    deps: &PackDeps,
    kind: &PackKind,
    source: &dyn PackSource,
) -> AppResult<Vec<ModpackBuild>> {
    let owned = engine::owned(deps, kind).await?;
    let projects: BTreeSet<String> = owned.iter().map(|(_, r)| r.project_id.clone()).collect();
    let (lists, last_error) = versions_of(source, projects).await;
    if lists.is_empty()
        && let Some(e) = last_error
    {
        return Err(e);
    }
    Ok(owned
        .into_iter()
        .filter_map(|(build, record)| {
            let list = lists.get(&record.project_id)?;
            let number = if record.version_number.is_empty() {
                list.iter()
                    .find(|v| v.id == record.version_id)
                    .map_or_else(|| record.version_id.clone(), |v| v.version_number.clone())
            } else {
                record.version_number.clone()
            };
            // A first record knows no Minecraft: the version it came from tells.
            let games = match &record.minecraft {
                Some(game) => vec![game.clone()],
                None => list
                    .iter()
                    .find(|v| v.id == record.version_id)
                    .map(|v| v.game_versions.clone())
                    .unwrap_or_default(),
            };
            let newest =
                newer_pack_version(list, &record.version_id, &games, record.client.as_deref().unwrap_or(""));
            Some(ModpackBuild {
                key: build.key,
                name: build.name,
                newest,
                project_id: record.project_id,
                version_id: record.version_id,
                version_number: number,
            })
        })
        .collect())
}

/// Updates build `args.key`, installed from this kind of pack, to another version of it — as the
/// original installs a version into a build: the pack's files and overrides replace theirs, the
/// files only the old version had go, the loader is installed first, and the build takes the new
/// version in the transaction's commit step. A failure leaves all as it was.
pub async fn update_pack(
    deps: &PackDeps,
    kind: &PackKind,
    source: &dyn PackSource,
    args: &PackUpdateArgs,
) -> AppResult<PackUpdated> {
    if deps.feedback.is_busy() {
        return Err(busy());
    }
    let build = deps
        .versions
        .get(&args.key)
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "no such build").with_param("key", &args.key))?;
    let game = game_dir(&build, deps.versions.minecraft_dir());
    let _lease = deps.instances.try_acquire(&game, kind.lease)?;
    // Before the record is read: an update a crash cut short may have changed it.
    let (versions, key, root, pack_kind) = (deps.versions.clone(), build.key.clone(), game.clone(), *kind);
    tokio::task::spawn_blocking(move || engine::recover(&versions, &pack_kind, &key, &root))
        .await
        .map_err(|e| AppError::internal(e.to_string()))??;
    let record = PackRecord::read(&game, kind.record).ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput, format!("the build is not a {} modpack's", kind.provider))
    })?;
    let spec = OperationSpec::new(Text::key("installation_started"), "install")
        .status(Text::key("modpack_updating").param("name", &build.name));
    let op = deps.feedback.begin(spec);
    match refill(deps, kind, source, &build, &game, &record, args, &op).await {
        Ok((Swapped { number, backups, folder }, skipped)) => {
            op.finish();
            let said = if backups.is_empty() {
                Text::key("modpack_updated")
            } else {
                Text::key("modpack_updated_backups")
                    .param("count", backups.len().to_string())
                    .param("folder", folder)
            };
            deps.feedback.success(said.param("name", &build.name).param("version", &number));
            Ok(PackUpdated { key: build.key.clone(), version_number: number, backups, skipped })
        }
        Err(e) => {
            op.fail(
                Text::key("version_install_error")
                    .param("client", kind.provider)
                    .param("version", &build.name)
                    .param("error", e.detail.clone()),
            );
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn refill(
    deps: &PackDeps,
    kind: &PackKind,
    source: &dyn PackSource,
    build: &Build,
    game: &Path,
    record: &PackRecord,
    args: &PackUpdateArgs,
    op: &OperationHandle,
) -> AppResult<(Swapped, Vec<HeldFile>)> {
    let archive = game.join(kind.archive);
    let held = HeldChoice::of_update(args);
    let result = async {
        let Fetched { pack, meta, skipped } =
            source.fetch(&record.project_id, &args.version_id, game, &archive, op, &held).await?;
        let players = source.players(game, &record.managed_files).await?;
        engine::update(deps, kind, build, game, record, &meta, &archive, &pack, players, op)
            .await
            .map(|swapped| (swapped, skipped))
    }
    .await;
    let _ = fs::remove_file(&archive);
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// Answers versions after a pause, counting how many are asked at once; `bad` gets no answer.
    #[derive(Default)]
    struct Slow {
        now: AtomicUsize,
        most: AtomicUsize,
    }

    impl PackSource for Slow {
        fn fetch<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: &'a Path,
            _: &'a Path,
            _: &'a OperationHandle,
            _: &'a HeldChoice,
        ) -> PackFuture<'a, Fetched> {
            unimplemented!()
        }

        fn versions<'a>(&'a self, project: &'a str) -> PackFuture<'a, Vec<PackVersion>> {
            Box::pin(async move {
                let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.now.fetch_sub(1, Ordering::SeqCst);
                if project == "bad" {
                    return Err(AppError::new(ErrorCode::Network, "no answer"));
                }
                Ok(vec![PackVersion {
                    id: format!("{project}-2"),
                    version_number: "2".into(),
                    game_versions: Vec::new(),
                    loaders: Vec::new(),
                }])
            })
        }

        fn players<'a>(&'a self, _: &'a Path, _: &'a [String]) -> PackFuture<'a, HashSet<String>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn the_packs_versions_are_asked_side_by_side() {
        // The Builds page waited for one pack's versions after another.
        let source = Slow::default();
        let projects: BTreeSet<String> = ["a", "b", "bad", "c", "d", "e"].map(String::from).into();
        let (lists, failure) = versions_of(&source, projects).await;
        assert_eq!(lists.len(), 5);
        assert_eq!(lists["c"][0].id, "c-2");
        assert_eq!(failure.map(|e| e.code), Some(ErrorCode::Network));
        let most = source.most.load(Ordering::SeqCst);
        assert!((2..=ASKED_AT_ONCE).contains(&most), "{most} at once");
    }
}
