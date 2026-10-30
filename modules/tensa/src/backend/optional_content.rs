//! The «Необов'язкові» source of a server build's Mods tab: the server's whitelist searched,
//! planned, installed (one transaction: the new file in, its older versions out, the note) and
//! told apart in the build's installed mods.

use std::fs;
use std::path::{Path, PathBuf};

use launcher_core::content::jar::inspect_mod_jar;
use launcher_core::feedback::OperationSpec;
use launcher_core::launch::options::game_dir;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, ExpectedHash, HashKind};
use launcher_core::storage::transaction::{ApplyHooks, FileTransaction, TransactionPlan};
use launcher_core::storage::versions::Build;
use launcher_shared::provider::{
    Action, Change, FileNote, InstallAnswer, InstallArgs, InstallOutcome, NewerVersion, Overview,
    OverviewArgs, PlanDto, PlanItem, ProjectHit, SearchArgs, SearchPage, UpdateSummary, UpdatesStatus,
};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode, LoaderKind, Text};

use super::identity::{self, JOURNAL};
use super::manifest::preserve_rules;
use super::optional::{Found, NOTE, Noted, State, hasher, is_disabled, read_note, recognize};
use super::pack::{Pack, find};
use super::service::Deps;
use super::whitelist::Item;

/// The lease an install holds on the build's folder.
const LEASE: &str = "tensacraft_optional";
const METADATA_RESERVE: u64 = 64 * 1024;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

/// A server build, its folder and its catalog entry.
struct Target {
    build: Build,
    game: PathBuf,
    pack: Pack,
}

/// The build `key` when the server manages it; `None` for any other build.
async fn target(deps: &Deps, key: &str) -> AppResult<Option<Target>> {
    let build = deps.versions.get(key).ok_or_else(|| {
        AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
    })?;
    let game = game_dir(&build, deps.versions.minecraft_dir());
    if !identity::is_managed(&build, &game) {
        return Ok(None);
    }
    let Some(pack_id) = identity::pack_id(&build) else { return Ok(None) };
    let Some(pack) = find(&deps.api.packs().await?, &pack_id) else { return Ok(None) };
    Ok(Some(Target { build, game, pack }))
}

/// The whitelist of the target's pack, and what the build has of it.
async fn found(deps: &Deps, target: &Target, fresh: bool) -> AppResult<Vec<Found>> {
    let items = deps.api.whitelist(&target.pack.id, target.pack.whitelist_endpoint.as_deref(), fresh).await?;
    let items: Vec<Item> = items.into_iter().filter(|item| item.kind == ContentKind::Mods).collect();
    let rules = preserve_rules(target.pack.preserve_rules.as_ref());
    let game = target.game.clone();
    blocking(move || {
        let note = read_note(&game);
        Ok(recognize(&game, &items, &rules, &note, &mut hasher(&game, &note)))
    })
    .await
}

fn file_name(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

pub async fn search(deps: &Deps, args: &SearchArgs) -> AppResult<SearchPage> {
    let empty = SearchPage { hits: Vec::new(), total: 0, offset: args.offset, limit: args.page_size() };
    if args.kind != ContentKind::Mods {
        return Ok(empty);
    }
    let Some(target) = target(deps, &args.key).await? else { return Ok(empty) };
    let items = deps.api.whitelist(&target.pack.id, target.pack.whitelist_endpoint.as_deref(), false).await?;
    let query = args.query.trim().to_lowercase();
    let matching: Vec<Item> = items
        .into_iter()
        .filter(|item| item.kind == ContentKind::Mods)
        .filter(|item| {
            query.is_empty()
                || item.title.to_lowercase().contains(&query)
                || item.name.to_lowercase().contains(&query)
        })
        .collect();
    let hits = matching
        .iter()
        .skip(args.offset as usize)
        .take(args.page_size() as usize)
        .map(|item| ProjectHit {
            project_id: item.id.clone(),
            slug: item.id.clone(),
            title: item.title.clone(),
            author: item.name.clone(),
            description: String::new(),
            downloads: 0,
            icon_url: None,
            url: item.page.clone(),
        })
        .collect();
    Ok(SearchPage { hits, total: matching.len() as u32, ..empty })
}

/// The plan of installing `found`; `occupied`: a file not its own already has its name.
fn plan_of(found: &Found, occupied: bool) -> PlanDto {
    let item = &found.item;
    let action = match found.state {
        State::Missing if occupied => Action::Replace,
        State::Missing => Action::Install,
        State::Outdated => Action::Replace,
        State::Current => Action::Satisfied,
    };
    let main = PlanItem {
        project_id: item.id.clone(),
        version_id: item.sha256.clone(),
        title: item.title.clone(),
        version_number: item.name.clone(),
        filename: item.name.clone(),
        action,
        current: (action == Action::Replace)
            .then(|| found.files.first().map_or_else(|| item.name.clone(), |f| file_name(f).to_string())),
        url: item.page.clone(),
        unrecognized: action == Action::Replace && (found.by_name_only || occupied),
    };
    // The item is the whole plan: no dependencies, nothing optional, nothing in the way.
    PlanDto {
        main: Some(main),
        install: Vec::new(),
        replace: Vec::new(),
        satisfied: Vec::new(),
        optional: Vec::new(),
        optional_issues: Vec::new(),
        embedded: Vec::new(),
        blocking: Vec::new(),
    }
}

fn gone(project_id: &str) -> AppError {
    AppError::new(ErrorCode::NotFound, format!("the server no longer allows {project_id}"))
        .with_param("name", project_id)
}

fn not_a_server_build(key: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("{key} is not a server build")).with_param("version", key)
}

pub async fn plan(deps: &Deps, args: &InstallArgs) -> AppResult<PlanDto> {
    let target = target(deps, &args.key).await?.ok_or_else(|| not_a_server_build(&args.key))?;
    let all = found(deps, &target, false).await?;
    let mine = all.iter().find(|f| f.item.id == args.project_id).ok_or_else(|| gone(&args.project_id))?;
    Ok(plan_of(mine, occupied(&target.game, mine)))
}

pub async fn install(deps: &Deps, args: &InstallArgs) -> AppResult<InstallAnswer> {
    let target = target(deps, &args.key).await?.ok_or_else(|| not_a_server_build(&args.key))?;
    let all = found(deps, &target, true).await?;
    let mine =
        all.into_iter().find(|f| f.item.id == args.project_id).ok_or_else(|| gone(&args.project_id))?;
    let plan = plan_of(&mine, occupied(&target.game, &mine));
    let main = plan.main.clone().ok_or_else(|| gone(&args.project_id))?;
    let outcome = InstallOutcome {
        project_id: main.project_id.clone(),
        filename: main.filename.clone(),
        version_number: main.version_number.clone(),
    };
    if main.action == Action::Satisfied {
        return Ok(InstallAnswer::Installed(outcome));
    }
    let change = Change {
        project_id: main.project_id.clone(),
        version_id: main.version_id.clone(),
        action: main.action,
    };
    if !args.approved.contains(&change) {
        return Ok(InstallAnswer::Replanned(Box::new(plan)));
    }
    let _lease = deps
        .instances
        .try_acquire(&target.game, LEASE)
        .map_err(|e| e.with_param("version", target.build.name.clone()))?;
    let name = mine.item.title.clone();
    let op = deps.feedback.begin(
        OperationSpec::new(Text::key("tensa_optional_installing").param("name", &name), "tensa")
            .on_build(&target.build.key),
    );
    let loader = target.pack.loader.as_deref().and_then(LoaderKind::from_client);
    match put_in(deps, &target.game, &mine, loader, &op).await {
        Ok(()) => {
            op.finish();
            let done = if main.action == Action::Replace {
                "tensa_optional_updated"
            } else {
                "tensa_optional_installed"
            };
            deps.feedback.success(Text::key(done).param("name", &name));
            Ok(InstallAnswer::Installed(outcome))
        }
        Err(e) => {
            op.fail(Text::key("installation_failed"));
            Err(e)
        }
    }
}

/// A file at `found`'s destination that is not its own (a mod put in by hand under the same
/// name): installing replaces it, so the player confirms first.
fn occupied(game: &Path, found: &Found) -> bool {
    let dest = destination(found);
    game.join(&dest).is_file() && !found.files.iter().any(|f| f.eq_ignore_ascii_case(&dest))
}

/// Where the item goes: its own name, turned off when the copy it replaces was.
fn destination(found: &Found) -> String {
    if found.disabled { format!("{}.disabled", found.item.relative) } else { found.item.relative.clone() }
}

/// The enabled jar of `game`'s `mods/`, not one of `leaving`, that declares mod `id`.
fn declaring(game: &Path, id: &str, leaving: &[String], loader: Option<LoaderKind>) -> Option<String> {
    let id = id.to_lowercase();
    let mut jars: Vec<String> = fs::read_dir(game.join("mods"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.to_lowercase().ends_with(".jar"))
        .collect();
    jars.sort();
    jars.into_iter().find(|name| {
        let relative = format!("mods/{name}");
        !leaving.iter().any(|gone| gone.eq_ignore_ascii_case(&relative))
            && inspect_mod_jar(&game.join(&relative), loader)
                .is_ok_and(|meta| meta.mod_id.to_lowercase() == id)
    })
}

/// Downloads `found`'s item into `game` in place of its other files, and notes it — one
/// transaction; a jar left in `mods/` that declares the same mod stops it all.
async fn put_in(
    deps: &Deps,
    game: &Path,
    found: &Found,
    loader: Option<LoaderKind>,
    op: &launcher_core::feedback::OperationHandle,
) -> AppResult<()> {
    let item = &found.item;
    let dest = destination(found);
    let stale: Vec<String> = found.files.iter().filter(|f| !f.eq_ignore_ascii_case(&dest)).cloned().collect();
    let plan = TransactionPlan {
        replacements: vec![dest.clone(), NOTE.to_string()],
        stale: stale.clone(),
        staged_bytes: item.size.unwrap_or_default().saturating_add(METADATA_RESERVE),
        ..TransactionPlan::new("tensacraft_optional")
    };
    let root = game.to_path_buf();
    let tx = blocking(move || FileTransaction::begin(&root, JOURNAL, plan, None)).await?;
    let staged = async {
        let staged_jar = tx.stage_path(&dest)?;
        let mut task = DownloadTask::new(item.url.clone(), staged_jar.clone())
            .hash(ExpectedHash { kind: HashKind::Sha256, hex: item.sha256.clone() });
        if let Some(size) = item.size {
            task = task.size(size);
        }
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        deps.downloader.download_all(vec![task], false, &progress).await?.into_result()?;
        if !is_disabled(&dest)
            && let Ok(meta) = inspect_mod_jar(&staged_jar, loader)
        {
            let leaving: Vec<String> = stale.iter().cloned().chain([dest.clone()]).collect();
            if let Some(taken) = declaring(game, &meta.mod_id, &leaving, loader) {
                return Err(AppError::new(
                    ErrorCode::ContentConflict,
                    format!("{} already has mod {}: {taken}", game.display(), meta.mod_id),
                )
                .with_param("name", taken));
            }
        }
        let mut note = read_note(game);
        note.retain(|id, noted| id != &item.id && !stale.iter().any(|s| file_name(s) == noted.file));
        let file = file_name(&dest).to_string();
        note.insert(
            item.id.clone(),
            Noted { file, sha256: item.sha256.clone(), size: item.size.unwrap_or(0), modified_ms: 0 },
        );
        let bytes = serde_json::to_vec_pretty(&note).map_err(|e| AppError::internal(e.to_string()))?;
        let staged_note = tx.stage_path(NOTE)?;
        if let Some(parent) = staged_note.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
        }
        fs::write(&staged_note, bytes).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
    }
    .await;
    blocking(move || match staged {
        Ok(()) => tx.apply(ApplyHooks::default()),
        Err(e) => Err(tx.abort(e)),
    })
    .await
}

pub async fn overview(deps: &Deps, args: &OverviewArgs) -> AppResult<Overview> {
    if args.kind != ContentKind::Mods {
        return Ok(Overview::default());
    }
    let Some(target) = target(deps, &args.key).await? else { return Ok(Overview::default()) };
    let all = found(deps, &target, false).await?;
    let mut notes = Vec::new();
    for found in &all {
        let newer = (args.check_updates && found.state == State::Outdated).then(|| NewerVersion {
            version_id: found.item.sha256.clone(),
            version_number: found.item.name.clone(),
        });
        for file in &found.files {
            notes.push(FileNote {
                file: file_name(file).to_string(),
                project_id: found.item.id.clone(),
                slug: found.item.id.clone(),
                title: found.item.title.clone(),
                version_number: file_name(file).to_string(),
                update: newer.clone(),
                url: found.item.page.clone(),
                icon_url: None,
                // Its own only when the launcher put it in: Modrinth keeps what it installed.
                installed: found.noted.as_deref() == Some(file.as_str()),
            });
        }
    }
    let updates = args.check_updates.then(|| {
        let available = all.iter().filter(|f| f.state == State::Outdated).count();
        let status = if available > 0 { UpdatesStatus::Available } else { UpdatesStatus::Current };
        UpdateSummary { status, available, unchecked: 0 }
    });
    Ok(Overview { notes, updates })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing() -> Found {
        Found {
            item: Item {
                id: "e4mc".into(),
                title: "e4mc".into(),
                kind: ContentKind::Mods,
                name: "e4mc-6.2.3.jar".into(),
                relative: "mods/e4mc-6.2.3.jar".into(),
                url: "https://cdn.example/e4mc-6.2.3.jar".into(),
                size: Some(10),
                sha256: "ab".into(),
                page: None,
            },
            files: Vec::new(),
            state: State::Missing,
            current: None,
            disabled: false,
            by_name_only: false,
            noted: None,
            by_name: Vec::new(),
            pattern: None,
        }
    }

    #[test]
    fn a_foreign_file_of_the_same_name_is_replaced_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let found = missing();
        let free = plan_of(&found, occupied(dir.path(), &found));
        assert_eq!(free.main.unwrap().action, Action::Install);
        fs::create_dir_all(dir.path().join("mods")).unwrap();
        fs::write(dir.path().join("mods/e4mc-6.2.3.jar"), b"someone else's").unwrap();
        let main = plan_of(&found, occupied(dir.path(), &found)).main.unwrap();
        assert_eq!(
            (main.action, main.unrecognized, main.current.as_deref()),
            (Action::Replace, true, Some("e4mc-6.2.3.jar")),
            "the player is asked before their file goes"
        );
    }
}
