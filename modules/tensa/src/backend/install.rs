//! A server build installed as a new build: the server's settings, its loader,
//! every file it lists — straight into the new build's folder — and its identity. A failed install
//! leaves no build and no folder behind.

use std::fs;
use std::path::Path;

use launcher_core::feedback::{OperationHandle, OperationSpec};
use launcher_core::loaders::ComponentSpec;
use launcher_core::minecraft::InstallProgress;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask};
use launcher_core::storage::journal::contained;
use launcher_core::storage::versions::Build;
use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind, Text};
use serde_json::{Value, json};

use super::identity::{CLIENT, mark};
use super::manifest::{expected_hash, relative_path, size};
use super::pack::find;
use super::profile::apply_install;
use super::service::Deps;

const LEASE: &str = "tensacraft_install";

/// Installs the server build `pack_id` as a new build named `name`.
pub async fn install(deps: &Deps, pack_id: &str, name: &str) -> AppResult<Build> {
    if deps.feedback.is_busy() {
        return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
    }
    let name = name.trim().to_string();
    let id = deps.versions.claim_folder(&name)?;
    let game = deps.versions.games_dir().join(&id);
    let op = deps.feedback.begin(
        OperationSpec::new(Text::key("installation_started"), "install")
            .status(Text::key("modpack_installing").param("name", &name)),
    );
    match fill(deps, &game, &id, &name, pack_id, &op).await {
        Ok(build) => {
            op.finish();
            deps.feedback.success(Text::key("version_install_success").param("version", &name));
            Ok(build)
        }
        Err(e) => {
            if let Err(why) = fs::remove_dir_all(&game) {
                tracing::warn!("Unable to remove the unfinished build {}: {why}", game.display());
            }
            op.fail(
                Text::key("version_install_error")
                    .param("client", CLIENT)
                    .param("version", &name)
                    .param("error", &e.detail),
            );
            Err(e)
        }
    }
}

/// A listed file's download into `game`: none for a file without an address or a safe path.
fn task(game: &Path, file: &Value) -> Option<DownloadTask> {
    let url = file.get("download_url").and_then(Value::as_str).map(str::trim).filter(|u| !u.is_empty())?;
    let relative = relative_path(file)?;
    let dest = match contained(game, &relative) {
        Ok(dest) => dest,
        Err(e) => {
            tracing::warn!("Leaving {relative} out: {}", e.detail);
            return None;
        }
    };
    let mut task = DownloadTask::new(url, dest);
    if let Some(size) = size(file) {
        task = task.size(size);
    }
    if let Some(hash) = expected_hash(file) {
        task = task.hash(hash);
    }
    Some(task)
}

async fn fill(
    deps: &Deps,
    game: &Path,
    id: &str,
    name: &str,
    pack_id: &str,
    op: &OperationHandle,
) -> AppResult<Build> {
    let _lease = deps.instances.try_acquire(game, LEASE)?;
    let packs = deps.api.packs().await?;
    let pack = find(&packs, pack_id).ok_or_else(|| {
        AppError::new(ErrorCode::NotFound, format!("no server build {pack_id}")).with_param("pack", pack_id)
    })?;
    let mut build = Build::new(name);
    build.options.insert("gpuMode".into(), json!((deps.gpu_mode)()));
    let kind = apply_install(&mut build, &pack)?;
    let minecraft = build.version.clone().unwrap_or_default();
    let spec = match kind {
        LoaderKind::Minecraft => ComponentSpec::vanilla(&minecraft),
        kind => {
            let version = build.loader_version.clone().ok_or_else(|| {
                AppError::new(
                    ErrorCode::InvalidInput,
                    format!("the server build {} names no loader version", pack.id),
                )
                .with_param("pack", &pack.id)
            })?;
            ComponentSpec::loader(kind, &minecraft, &version)
        }
    };
    let progress = |p: InstallProgress| {
        let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
        op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
    };
    let installed = deps.components.install(&spec, &progress).await?;
    build.loader = Some(installed.id.clone());
    if let Some(java) = &installed.java {
        build.options.insert("executablePath".into(), json!(java.to_string_lossy()));
    }
    op.update(Some(Text::key("modpack_installing").param("name", name)), Some(0.0), Some(1.0));
    let files = deps.api.files(&pack.id, pack.files_endpoint.as_deref()).await?;
    let tasks: Vec<DownloadTask> = files.iter().filter_map(|file| task(game, file)).collect();
    let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
    deps.downloader.download_all(tasks, false, &progress).await?.into_result()?;
    mark(&mut build, &pack.id);
    build.description = pack.description.clone().unwrap_or_default();
    deps.versions.create_in(&mut build, id)?;
    Ok(build)
}
