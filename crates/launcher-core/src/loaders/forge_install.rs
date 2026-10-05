//! Forge and NeoForge through their installer: the installer's version JSON
//! under our id, the jars it ships and needs, the base through the Minecraft installer, then the
//! client processors with the base's Java. One shared lease covers every step; the marker is
//! written only when all of them are done.

use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind, Text};
use serde_json::Value;

use super::fabric::version_token_ok;
use super::forge::{installer_coords, installer_url};
use super::installer::{InstallProfile, InstallerJar, ModernProfile, Processor};
use super::processors::{
    ProcessorContext, ProcessorPlan, clear_marker, marker_ready, outputs_ok, plan_processors, write_marker,
};
use super::{ComponentInstaller, ComponentSpec};
use crate::builds::ids::validate_component_id;
use crate::minecraft::install::InstalledVersion;
use crate::minecraft::library::{library_path, plan_libraries};
use crate::minecraft::version::{version_dir, version_json_path};
use crate::minecraft::{InstallProgress, InstallProgressFn};
use crate::net::downloader::{DownloadTask, ExpectedHash};
use crate::storage::json::write_json_file;

/// A checksum file next to a download: its extension, hex length and hash kind.
type Sidecar = (&'static str, usize, fn(&str) -> ExpectedHash);

/// Where installer files are unpacked for the processors (removed after a success).
fn work_dir(mc_dir: &Path, id: &str) -> PathBuf {
    version_dir(mc_dir, id).join(".installer")
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::internal(e.to_string())
}

fn loader_failed(why: impl Into<String>) -> AppError {
    let why = why.into();
    AppError::new(ErrorCode::LoaderInstallFailed, why.clone()).with_param("error", why)
}

fn wrong_minecraft(claimed: &str, wanted: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("the installer is for Minecraft {claimed}, not {wanted}"))
        .with_param("version", wanted)
}

impl ComponentInstaller {
    /// Installs a Forge or NeoForge `spec`.
    pub(super) async fn install_with_installer(
        &self,
        spec: &ComponentSpec,
        verify: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        let lv = spec.loader_version.as_deref().unwrap_or_default();
        if !version_token_ok(&spec.mc) || !version_token_ok(lv) {
            return Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("unusable versions {:?} / {lv:?}", spec.mc),
            )
            .with_param("version", &spec.mc));
        }
        let id = spec.component_id();
        validate_component_id(&id)?;
        let loader = spec.kind.display_name();
        let status =
            Text::key("version_component_installing").param("loader", loader).param("version", &spec.mc);
        progress(InstallProgress::status(status.clone()));
        let lease = self.minecraft.lock(&format!("{}_install", loader.to_lowercase()))?;
        clear_marker(&self.mc_dir, &id);
        let installer = self.fetch_installer(spec.kind, &spec.mc, lv, verify).await?;
        let (profile, shipped) = self.unpack_installer(&spec.mc, &id, &installer).await?;
        if let InstallProfile::Modern(modern) = &profile {
            let plan = plan_libraries(
                &modern.libraries,
                &self.mc_dir.join("libraries"),
                self.minecraft.libraries_repo(),
                self.minecraft.platform(),
            )?;
            self.minecraft
                .downloader()
                .download_all(plan.tasks, verify, &|p| progress(InstallProgress::download(status.clone(), p)))
                .await?
                .into_result()?;
        }
        let installed = self.minecraft.install_with(&id, verify, &lease, progress).await?;
        let made = match &profile {
            InstallProfile::Modern(modern) if modern.processors.iter().any(Processor::runs_on_client) => {
                let java = installed
                    .java
                    .clone()
                    .ok_or_else(|| loader_failed("no Java to run the installer with"))?;
                self.run_processors(spec.kind, &spec.mc, &id, modern, &installer, &java, progress).await?
            }
            _ => Vec::new(),
        };
        let artifacts: Vec<PathBuf> = shipped.into_iter().chain(made).collect();
        write_marker(&self.mc_dir, &id, &artifacts)?;
        let _ = fs::remove_dir_all(work_dir(&self.mc_dir, &id));
        Ok(installed)
    }

    /// `id` as it is, when everything the installer made is in place and the version checks
    /// out (checked once: a launch needs nothing more).
    pub(super) async fn installer_ready(&self, id: &str) -> Option<InstalledVersion> {
        if !marker_ready(&self.mc_dir, id, false) {
            return None;
        }
        let check = self.minecraft.check_async(id).await;
        check.valid.then(|| self.minecraft.checked_out(id, &check))
    }

    /// The installer jar, kept at its Maven path in `libraries/` (repairs reuse it) and checked
    /// against the strongest checksum its Maven publishes when it is downloaded or verified.
    async fn fetch_installer(
        &self,
        kind: LoaderKind,
        mc: &str,
        lv: &str,
        verify: bool,
    ) -> AppResult<PathBuf> {
        let unusable = || {
            AppError::new(
                ErrorCode::InvalidInput,
                format!("no installer for {} {mc} {lv}", kind.display_name()),
            )
        };
        let url = installer_url(kind, &self.endpoints, mc, lv).ok_or_else(unusable)?;
        let coords = installer_coords(kind, mc, lv).ok_or_else(unusable)?;
        let dest = self.mc_dir.join("libraries").join(library_path(&coords).ok_or_else(unusable)?);
        let mut task = DownloadTask::new(url.as_str(), dest.clone());
        if verify || !dest.is_file() {
            match self.published_checksum(&url).await {
                Some(hash) => task = task.hash(hash),
                None => tracing::warn!("{url} publishes no checksum"),
            }
        }
        self.minecraft.downloader().download_all(vec![task], verify, &|_| {}).await?.into_result()?;
        Ok(dest)
    }

    /// `.sha512`, else `.sha256`, else `.sha1` next to `url`.
    async fn published_checksum(&self, url: &str) -> Option<ExpectedHash> {
        let kinds: [Sidecar; 3] = [
            ("sha512", 128, ExpectedHash::sha512),
            ("sha256", 64, ExpectedHash::sha256),
            ("sha1", 40, ExpectedHash::sha1),
        ];
        for (ext, len, hash) in kinds {
            let Ok(body) = self.meta.get_bytes(&format!("{url}.{ext}")).await else { continue };
            let text = String::from_utf8_lossy(&body);
            let hex = text.split_whitespace().next().unwrap_or_default();
            if hex.len() == len && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(hash(&hex.to_lowercase()));
            }
        }
        None
    }

    /// Checks the installer is for Minecraft `mc`, writes its version JSON as `id` and puts the jars
    /// it ships into `libraries/` (returned, for the marker). A jar that does not open is deleted,
    /// to be downloaded again.
    async fn unpack_installer(
        &self,
        mc: &str,
        id: &str,
        installer: &Path,
    ) -> AppResult<(InstallProfile, Vec<PathBuf>)> {
        let (mc_dir, mc, id, installer) =
            (self.mc_dir.clone(), mc.to_string(), id.to_string(), installer.to_path_buf());
        tokio::task::spawn_blocking(move || -> AppResult<(InstallProfile, Vec<PathBuf>)> {
            let mut jar = InstallerJar::open(&installer).inspect_err(|_| {
                let _ = fs::remove_file(&installer);
            })?;
            let profile = jar.profile()?;
            if profile.minecraft() != mc {
                return Err(wrong_minecraft(profile.minecraft(), &mc));
            }
            let mut version = jar.version_json(&profile)?;
            let parent = version.get("inheritsFrom").and_then(Value::as_str).map(str::to_string);
            match parent.as_deref() {
                Some(parent) if parent != mc => return Err(wrong_minecraft(parent, &mc)),
                Some(_) => {}
                None => {
                    version.insert("inheritsFrom".into(), Value::String(mc.clone()));
                }
            }
            version.insert("id".into(), Value::String(id.clone()));
            let path = version_json_path(&mc_dir, &id);
            fs::create_dir_all(version_dir(&mc_dir, &id))
                .and_then(|()| write_json_file(&path, &Value::Object(version), 2))
                .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", path.display())))?;
            let libraries = mc_dir.join("libraries");
            let mut shipped = jar.extract_maven(&libraries)?;
            if let InstallProfile::Legacy(legacy) = &profile {
                let relative = library_path(&legacy.coords).ok_or_else(|| {
                    AppError::new(ErrorCode::InvalidInput, format!("unusable coordinates {}", legacy.coords))
                })?;
                let dest = libraries.join(relative);
                jar.extract(&legacy.file_path, &dest)?;
                shipped.push(dest);
            }
            Ok((profile, shipped))
        })
        .await
        .map_err(internal)?
    }

    /// Runs the client processors in order with `java`; one whose outputs already check out is
    /// skipped, as the official installer does. Returns the files to remember in the marker.
    #[allow(clippy::too_many_arguments)]
    async fn run_processors(
        &self,
        kind: LoaderKind,
        mc: &str,
        id: &str,
        profile: &ModernProfile,
        installer: &Path,
        java: &Path,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Vec<PathBuf>> {
        let work = work_dir(&self.mc_dir, id);
        let (mc_dir, minecraft, profile, installer, work_owned) =
            (self.mc_dir.clone(), mc.to_string(), profile.clone(), installer.to_path_buf(), work.clone());
        let plan = tokio::task::spawn_blocking(move || -> AppResult<ProcessorPlan> {
            let _ = fs::remove_dir_all(&work_owned);
            fs::create_dir_all(&work_owned)
                .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", work_owned.display())))?;
            let ctx = ProcessorContext {
                mc_dir: &mc_dir,
                minecraft: &minecraft,
                installer: &installer,
                work_dir: &work_owned,
            };
            let plan = plan_processors(&profile, &ctx)?;
            let mut jar = InstallerJar::open(&installer)?;
            for (entry, dest) in &plan.extracts {
                jar.extract(entry, dest)?;
            }
            Ok(plan)
        })
        .await
        .map_err(internal)??;
        let total = plan.calls.len();
        for (index, call) in plan.calls.iter().enumerate() {
            progress(InstallProgress::status(
                Text::key("loader_installer_step")
                    .param("loader", kind.display_name())
                    .param("step", (index + 1).to_string())
                    .param("total", total.to_string()),
            ));
            let (runner, java, call, work) =
                (self.runner.clone(), java.to_path_buf(), call.clone(), work.clone());
            tokio::task::spawn_blocking(move || -> AppResult<()> {
                if !call.outputs.is_empty() && outputs_ok(&call.outputs) {
                    tracing::info!("Processor {} is already done", call.jar);
                    return Ok(());
                }
                tracing::info!("Running processor {} ({})", call.jar, call.main_class);
                runner.run(&java, &call, &work).map_err(loader_failed)?;
                if !outputs_ok(&call.outputs) {
                    return Err(loader_failed(format!("{} made files with the wrong checksum", call.jar)));
                }
                Ok(())
            })
            .await
            .map_err(internal)??;
        }
        Ok(plan.artifacts)
    }
}
