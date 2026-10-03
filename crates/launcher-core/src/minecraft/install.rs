//! Installs a Minecraft version where MLL puts it, with two fixes over the original:
//! incomplete versions are really repaired, and markers tell an interrupted install apart.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use launcher_shared::{AppError, AppResult, CatalogVersion, ErrorCode, Text};
use serde_json::{Map, Value};

use super::assets::{AssetIndex, index_path};
use super::catalog::catalog_entries;
use super::integrity::{INSTALLED_MARKER, INSTALLING_MARKER, VersionCheck, check_version};
use super::library::{extract_natives, plan_libraries};
use super::manifest::{MojangEndpoints, fetch_manifest};
use super::platform::GamePlatform;
use super::version::{
    FileRef, MAX_INHERITANCE, VersionInfo, load_merged, read_version_json, version_dir, version_json_path,
};
use super::{InstallProgress, InstallProgressFn};
use crate::builds::ids::validate_component_id;
use crate::java::runtime::{JavaRuntimes, LEGACY_COMPONENT};
use crate::lock::{Coordinator, Lease};
use crate::net::downloader::{DownloadTask, Downloader, ExpectedHash};
use crate::net::meta::MetaClient;

pub const INSTALL_ATTEMPTS: u32 = 4;
pub const INSTALL_RETRY_DELAY: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    pub id: String,
    /// Its managed Java; `None` where Mojang has none for this platform.
    pub java: Option<PathBuf>,
}

pub struct MinecraftInstaller {
    mc_dir: PathBuf,
    platform: GamePlatform,
    endpoints: MojangEndpoints,
    meta: Arc<MetaClient>,
    downloader: Arc<Downloader>,
    java: Arc<JavaRuntimes>,
    shared: Arc<Coordinator>,
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
}

fn file_task(file: &FileRef, dest: PathBuf) -> DownloadTask {
    let mut task = DownloadTask::new(file.url.as_str(), dest);
    if let Some(size) = file.size {
        task = task.size(size);
    }
    if let Some(sha1) = &file.sha1 {
        task = task.hash(ExpectedHash::sha1(sha1));
    }
    task
}

/// Worth another whole attempt: the network failed, not the metadata or the disk, and it is not
/// down altogether (another attempt soon would only wait out the same refusals).
fn retryable(error: &AppError) -> bool {
    matches!(error.code, ErrorCode::DownloadFailed | ErrorCode::Network)
        && error.params.get("network").map(String::as_str) != Some("down")
}

impl MinecraftInstaller {
    pub fn new(
        mc_dir: &Path,
        platform: GamePlatform,
        endpoints: MojangEndpoints,
        meta: Arc<MetaClient>,
        downloader: Arc<Downloader>,
        java: Arc<JavaRuntimes>,
        shared: Arc<Coordinator>,
    ) -> MinecraftInstaller {
        MinecraftInstaller {
            mc_dir: mc_dir.to_path_buf(),
            platform,
            endpoints,
            meta,
            downloader,
            java,
            shared,
        }
    }

    /// Checks `id` by presence and size, its Java included. Blocking.
    pub fn check(&self, id: &str) -> VersionCheck {
        check_version(&self.mc_dir, id, &self.platform, Some(&self.java), false)
    }

    /// The shared Minecraft lock for an operation of `kind`. Loader installs hold
    /// it across their own steps and pass it to `install_with`.
    pub fn lock(&self, kind: &str) -> AppResult<Lease> {
        self.shared.try_acquire(&self.mc_dir, kind)
    }

    pub fn downloader(&self) -> &Arc<Downloader> {
        &self.downloader
    }

    pub fn platform(&self) -> &GamePlatform {
        &self.platform
    }

    /// Mojang's library repository, for libraries that name none.
    pub fn libraries_repo(&self) -> &str {
        &self.endpoints.libraries
    }

    /// `check` off the async runtime.
    pub async fn check_async(&self, id: &str) -> VersionCheck {
        let (mc_dir, id, platform, java) =
            (self.mc_dir.clone(), id.to_string(), self.platform.clone(), self.java.clone());
        tokio::task::spawn_blocking(move || check_version(&mc_dir, &id, &platform, Some(&java), false))
            .await
            .unwrap_or_default()
    }

    /// `check` that also compares every file with its known hash (Verify on the Components page).
    pub async fn check_deep(&self, id: &str) -> VersionCheck {
        let (mc_dir, id, platform, java) =
            (self.mc_dir.clone(), id.to_string(), self.platform.clone(), self.java.clone());
        tokio::task::spawn_blocking(move || check_version(&mc_dir, &id, &platform, Some(&java), true))
            .await
            .unwrap_or_default()
    }

    /// The launcher's Java for version `id` (the runtime its JSON asks for), when installed.
    pub fn managed_java(&self, id: &str) -> Option<PathBuf> {
        let info = VersionInfo::from_json(&load_merged(&self.mc_dir, id).ok()?).ok()?;
        let component = info.java_component.unwrap_or_else(|| LEGACY_COMPONENT.to_string());
        self.java.executable(&component)
    }

    /// Minecraft versions for the "Create build" page (the manifest is cached for an hour).
    pub async fn catalog(&self, snapshots: bool) -> AppResult<Vec<CatalogVersion>> {
        let manifest = fetch_manifest(&self.meta, &self.endpoints).await?;
        Ok(catalog_entries(&manifest, snapshots))
    }

    /// Installs `id`, what it inherits from and its Java; `verify` also re-hashes the files
    /// already present. Holds the shared Minecraft lock for the whole run; network failures
    /// retry the whole install (4 attempts, 0.75 s apart).
    pub async fn install(
        &self,
        id: &str,
        verify: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        validate_component_id(id)?;
        let lease = self.lock("minecraft_install")?;
        self.install_with(id, verify, &lease, progress).await
    }

    /// `install` under a lease the caller already holds.
    pub async fn install_with(
        &self,
        id: &str,
        verify: bool,
        lease: &Lease,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        validate_component_id(id)?;
        let mut attempt = 1;
        loop {
            match self.install_locked(id, verify, lease, progress, Vec::new()).await {
                Err(e) if attempt < INSTALL_ATTEMPTS && retryable(&e) => {
                    tracing::warn!(
                        "Minecraft {id} install attempt {attempt}/{INSTALL_ATTEMPTS} failed; retrying: {}",
                        e.detail
                    );
                    attempt += 1;
                    tokio::time::sleep(INSTALL_RETRY_DELAY).await;
                }
                result => return result,
            }
        }
    }

    /// Before a launch: nothing to do when the version checks out, otherwise an install that
    /// fetches what is missing (a folder that merely exists is not enough; files of the right size
    /// are trusted, as the check did). `force_check` hashes every file and repairs anyway.
    pub async fn ensure_installed(
        &self,
        id: &str,
        force_check: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        validate_component_id(id)?;
        let check = self.check_async(id).await;
        if check.valid && !force_check {
            let java = check.java_component.as_deref().and_then(|c| self.java.executable(c));
            return Ok(InstalledVersion { id: id.to_string(), java });
        }
        if !check.valid {
            tracing::warn!("Minecraft {id} needs repair: {}", check.issues.join("; "));
        }
        self.install(id, force_check, progress).await
    }

    /// One attempt under `lease`; the parent version is installed first (recursion needs the
    /// boxed future). `children` are the versions already waiting on this one: a parent among
    /// them is an inheritance loop.
    fn install_locked<'a>(
        &'a self,
        id: &'a str,
        verify: bool,
        lease: &'a Lease,
        progress: InstallProgressFn<'a>,
        children: Vec<String>,
    ) -> BoxFuture<'a, AppResult<InstalledVersion>> {
        Box::pin(async move {
            let key = if verify { "repairing_minecraft_version" } else { "installing_minecraft_version" };
            let status = Text::key(key).param("version", id);
            progress(InstallProgress::status(status.clone()));
            let json = self.version_json(id).await?;
            if let Some(parent) = json.get("inheritsFrom").and_then(Value::as_str) {
                validate_component_id(parent)?;
                if parent == id
                    || children.iter().any(|child| child == parent)
                    || children.len() >= MAX_INHERITANCE
                {
                    return Err(AppError::new(
                        ErrorCode::InvalidInput,
                        format!("version {id}: inheritance loop at {parent}"),
                    )
                    .with_param("version", id));
                }
                let mut chain = children;
                chain.push(id.to_string());
                self.install_locked(parent, verify, lease, progress, chain).await?;
            }
            let dir = version_dir(&self.mc_dir, id);
            fs::write(dir.join(INSTALLING_MARKER), b"").map_err(|e| io_error(&dir, e))?;
            let _ = fs::remove_file(dir.join(INSTALLED_MARKER));
            let info = VersionInfo::from_json(&load_merged(&self.mc_dir, id)?)?;
            let assets_dir = self.mc_dir.join("assets");
            let index = self.asset_index(&info, &assets_dir).await?;
            let libraries = plan_libraries(
                &info.libraries,
                &self.mc_dir.join("libraries"),
                &self.endpoints.libraries,
                &self.platform,
            )?;
            let mut tasks = libraries.tasks;
            if let Some(client) = &info.client {
                tasks.push(file_task(client, dir.join(format!("{id}.jar"))));
            }
            if let Some(log) = &info.log_config
                && let Some(name) = &log.id
            {
                tasks.push(file_task(log, assets_dir.join("log_configs").join(name)));
            }
            if let Some(index) = &index {
                tasks.extend(index.object_tasks(&assets_dir, &self.endpoints.resources));
            }
            self.downloader
                .download_all(tasks, verify, &|p| progress(InstallProgress::download(status.clone(), p)))
                .await?
                .into_result()?;
            let (natives, natives_dir) = (libraries.natives, dir.join("natives"));
            tokio::task::spawn_blocking(move || -> AppResult<()> {
                for jar in &natives {
                    extract_natives(&jar.jar, &natives_dir, &jar.exclude)?;
                }
                if let Some(index) = &index {
                    index.materialize_virtual(&assets_dir)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
            let component = info.java_component.clone().unwrap_or_else(|| LEGACY_COMPONENT.to_string());
            let java = if verify {
                self.java.repair(&component, lease, progress).await?
            } else {
                self.java.ensure(&component, lease, progress).await?
            };
            fs::write(dir.join(INSTALLED_MARKER), b"").map_err(|e| io_error(&dir, e))?;
            let _ = fs::remove_file(dir.join(INSTALLING_MARKER));
            progress(InstallProgress::status(Text::key("installation_complete")));
            Ok(InstalledVersion { id: id.to_string(), java })
        })
    }

    /// The local version JSON, or the one Mojang lists (checked against its SHA-1); a damaged
    /// local copy is replaced.
    async fn version_json(&self, id: &str) -> AppResult<Map<String, Value>> {
        match read_version_json(&self.mc_dir, id) {
            Ok(json) => return Ok(json),
            Err(e) if matches!(e.code, ErrorCode::VersionNotFound | ErrorCode::InvalidInput) => {
                tracing::info!("Fetching the version file of {id}: {}", e.detail);
            }
            Err(e) => return Err(e),
        }
        let manifest = fetch_manifest(&self.meta, &self.endpoints).await?;
        let entry = manifest.find(id).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("Mojang lists no version {id}"))
                .with_param("version", id)
        })?;
        let mut task = DownloadTask::new(entry.url.as_str(), version_json_path(&self.mc_dir, id));
        if let Some(sha1) = &entry.sha1 {
            task = task.hash(ExpectedHash::sha1(sha1));
        }
        self.downloader.download_all(vec![task], true, &|_| {}).await?.into_result()?;
        read_version_json(&self.mc_dir, id)
    }

    /// The asset index, downloaded and always checked by hash (it is small, and Mojang updates
    /// indexes in place).
    async fn asset_index(&self, info: &VersionInfo, assets_dir: &Path) -> AppResult<Option<AssetIndex>> {
        let (Some(name), Some(file)) = (&info.asset_index_name, &info.asset_index) else { return Ok(None) };
        let path = index_path(assets_dir, name);
        self.downloader
            .download_all(vec![file_task(file, path.clone())], true, &|_| {})
            .await?
            .into_result()?;
        tokio::task::spawn_blocking(move || AssetIndex::read(&path))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_network_that_is_down_is_not_tried_again_at_once() {
        use launcher_shared::{AppError, ErrorCode};
        assert!(super::retryable(&AppError::new(ErrorCode::DownloadFailed, "x")));
        assert!(super::retryable(&AppError::new(ErrorCode::Network, "x")));
        let down = AppError::new(ErrorCode::Network, "x").with_param("network", "down");
        assert!(!super::retryable(&down));
    }
}
