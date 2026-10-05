//! Mod loaders on top of the Minecraft installer: Fabric and Quilt
//! through their meta profile, Forge and NeoForge through their installer's own steps.

pub mod fabric;
pub mod forge;
pub mod installer;
pub mod processors;

mod forge_install;
pub mod versions;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderKind, LoaderOption, Text};
use serde_json::Value;

use crate::minecraft::install::{InstalledVersion, MinecraftInstaller};
use crate::minecraft::integrity::VersionCheck;
use crate::minecraft::version::{read_version_json, version_json_path};
use crate::minecraft::{InstallProgress, InstallProgressFn};
use crate::net::meta::MetaClient;
use crate::storage::json::write_json_file;
use crate::storage::versions::Build;
use fabric::FabricMeta;
use forge::ForgeMeta;
use processors::{JavaProcessorRunner, ProcessorRunner};
use versions::{game_builds, offered_builds};

/// Where loader metadata and installers come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderEndpoints {
    pub fabric: String,
    pub quilt: String,
    /// Forge's Maven: `maven-metadata.xml` and the installers.
    pub forge: String,
    /// Forge's recommended builds (`promotions_slim.json`).
    pub forge_promotions: String,
    /// NeoForge's Maven: its version list and the installers.
    pub neoforge: String,
}

impl Default for LoaderEndpoints {
    fn default() -> Self {
        LoaderEndpoints {
            fabric: "https://meta.fabricmc.net/v2".into(),
            quilt: "https://meta.quiltmc.org/v3".into(),
            forge: "https://maven.minecraftforge.net".into(),
            forge_promotions:
                "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json".into(),
            neoforge: "https://maven.neoforged.net".into(),
        }
    }
}

/// What a build runs: Minecraft `mc`, maybe with a loader build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSpec {
    pub kind: LoaderKind,
    pub mc: String,
    pub loader_version: Option<String>,
}

/// The loader build an installed id names (`fabric-loader-0.16.9-1.21.1` → `0.16.9`,
/// `1.20.1-forge-47.4.10` → `47.4.10`); `None` for plain Minecraft or another shape.
pub fn loader_version_of(kind: LoaderKind, mc: &str, id: &str) -> Option<String> {
    let lower = id.to_lowercase();
    let rest = match kind {
        LoaderKind::Fabric => lower.strip_prefix("fabric-loader-")?.strip_suffix(&format!("-{mc}"))?,
        LoaderKind::Quilt => lower.strip_prefix("quilt-loader-")?.strip_suffix(&format!("-{mc}"))?,
        LoaderKind::Forge => lower.strip_prefix(&format!("{mc}-forge-"))?,
        LoaderKind::NeoForge => lower.strip_prefix("neoforge-")?,
        LoaderKind::Minecraft => return None,
    };
    Some(rest.to_string())
}

impl ComponentSpec {
    pub fn vanilla(mc: &str) -> ComponentSpec {
        ComponentSpec { kind: LoaderKind::Minecraft, mc: mc.to_string(), loader_version: None }
    }

    pub fn loader(kind: LoaderKind, mc: &str, loader_version: &str) -> ComponentSpec {
        ComponentSpec { kind, mc: mc.to_string(), loader_version: Some(loader_version.to_string()) }
    }

    pub fn component_id(&self) -> String {
        self.kind.component_id(&self.mc, self.loader_version.as_deref().unwrap_or_default())
    }

    /// What `build` runs; `None` when it names no Minecraft version. A missing loader build is read
    /// from the loader's id.
    pub fn from_build(build: &Build) -> Option<ComponentSpec> {
        let mc = build.version.as_deref().map(str::trim).filter(|v| !v.is_empty())?;
        let loader_id = build.loader.as_deref().unwrap_or_default();
        let kind = LoaderKind::of_component(loader_id)
            .or_else(|| build.client.as_deref().and_then(LoaderKind::from_client))
            .unwrap_or(LoaderKind::Minecraft);
        if kind == LoaderKind::Minecraft {
            return Some(ComponentSpec::vanilla(mc));
        }
        let lv = build
            .loader_version
            .clone()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| loader_version_of(kind, mc, loader_id))?;
        Some(ComponentSpec::loader(kind, mc, &lv))
    }
}

fn unsupported(kind: LoaderKind) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("{} builds are not supported yet", kind.display_name()))
        .with_param("loader", kind.display_name())
}

/// Installs what builds run: plain Minecraft through `MinecraftInstaller`, Fabric and Quilt by
/// writing their profile JSON first.
pub struct ComponentInstaller {
    minecraft: Arc<MinecraftInstaller>,
    fabric: FabricMeta,
    quilt: FabricMeta,
    forge: ForgeMeta,
    neoforge: ForgeMeta,
    meta: Arc<MetaClient>,
    endpoints: LoaderEndpoints,
    runner: Arc<dyn ProcessorRunner>,
    mc_dir: PathBuf,
}

impl ComponentInstaller {
    pub fn new(
        minecraft: Arc<MinecraftInstaller>,
        meta: Arc<MetaClient>,
        endpoints: LoaderEndpoints,
        mc_dir: &Path,
    ) -> ComponentInstaller {
        ComponentInstaller {
            minecraft,
            fabric: FabricMeta::new(LoaderKind::Fabric, &endpoints.fabric, meta.clone()),
            quilt: FabricMeta::new(LoaderKind::Quilt, &endpoints.quilt, meta.clone()),
            forge: ForgeMeta::new(LoaderKind::Forge, endpoints.clone(), meta.clone()),
            neoforge: ForgeMeta::new(LoaderKind::NeoForge, endpoints.clone(), meta.clone()),
            meta,
            endpoints,
            runner: Arc::new(JavaProcessorRunner),
            mc_dir: mc_dir.to_path_buf(),
        }
    }

    /// Runs installer processors with `runner` instead of Java (tests).
    pub fn with_runner(mut self, runner: Arc<dyn ProcessorRunner>) -> ComponentInstaller {
        self.runner = runner;
        self
    }

    pub fn minecraft(&self) -> &MinecraftInstaller {
        &self.minecraft
    }

    fn meta_of(&self, kind: LoaderKind) -> AppResult<&FabricMeta> {
        match kind {
            LoaderKind::Fabric => Ok(&self.fabric),
            LoaderKind::Quilt => Ok(&self.quilt),
            other => Err(unsupported(other)),
        }
    }

    fn installer_meta(&self, kind: LoaderKind) -> AppResult<&ForgeMeta> {
        match kind {
            LoaderKind::Forge => Ok(&self.forge),
            LoaderKind::NeoForge => Ok(&self.neoforge),
            other => Err(unsupported(other)),
        }
    }

    /// Minecraft versions the loader supports and Mojang lists (in Mojang's order), each with the
    /// loader builds on offer.
    pub async fn catalog(&self, kind: LoaderKind, unstable: bool) -> AppResult<Vec<LoaderOption>> {
        match kind {
            LoaderKind::Forge | LoaderKind::NeoForge => self.installer_catalog(kind, unstable).await,
            _ => self.meta_catalog(kind, unstable).await,
        }
    }

    /// Forge and NeoForge: every Minecraft version has builds of its own.
    async fn installer_catalog(&self, kind: LoaderKind, unstable: bool) -> AppResult<Vec<LoaderOption>> {
        // Asked side by side: each is a request of its own (often to another host).
        let (games, vanilla) =
            tokio::try_join!(self.installer_meta(kind)?.games(), self.minecraft.catalog(true))?;
        Ok(vanilla
            .into_iter()
            .filter(|v| unstable || v.kind != "snapshot")
            .filter_map(|v| {
                let game = games.get(&v.id)?;
                let (builds, default_version) =
                    game_builds(&game.builds, game.recommended.as_deref(), unstable)?;
                Some(LoaderOption { snapshot: v.kind == "snapshot", mc: v.id, builds, default_version })
            })
            .collect())
    }

    async fn meta_catalog(&self, kind: LoaderKind, unstable: bool) -> AppResult<Vec<LoaderOption>> {
        let meta = self.meta_of(kind)?;
        let (games, loaders, vanilla) =
            tokio::try_join!(meta.games(), meta.loaders(), self.minecraft.catalog(true))?;
        let builds = offered_builds(&loaders, unstable);
        let Some(default) = builds.first().map(|b| b.version.clone()) else { return Ok(Vec::new()) };
        Ok(vanilla
            .into_iter()
            .filter(|v| games.iter().any(|(id, stable)| id == &v.id && (*stable || unstable)))
            .filter(|v| unstable || v.kind != "snapshot")
            .map(|v| LoaderOption {
                snapshot: v.kind == "snapshot",
                mc: v.id,
                builds: builds.clone(),
                default_version: default.clone(),
            })
            .collect())
    }

    async fn write_profile(&self, spec: &ComponentSpec) -> AppResult<()> {
        let lv = spec.loader_version.as_deref().ok_or_else(|| {
            AppError::new(ErrorCode::InvalidInput, "no loader build").with_param("version", &spec.mc)
        })?;
        let profile = self.meta_of(spec.kind)?.profile(&spec.mc, lv).await?;
        let path = version_json_path(&self.mc_dir, &spec.component_id());
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(&dir)
            .and_then(|()| write_json_file(&path, &Value::Object(profile), 2))
            .map_err(|e| {
                AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string())
                    .with_param("path", dir.to_string_lossy())
            })
    }

    /// Installs `spec` (its base, libraries and Java come through the Minecraft installer).
    pub async fn install(
        &self,
        spec: &ComponentSpec,
        verify: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        match spec.kind {
            LoaderKind::Minecraft => self.minecraft.install(&spec.mc, verify, progress).await,
            LoaderKind::Fabric | LoaderKind::Quilt => {
                progress(InstallProgress::status(
                    Text::key("version_component_installing")
                        .param("loader", spec.kind.display_name())
                        .param("version", &spec.mc),
                ));
                // Under the shared lock from the start: busy, nothing is fetched or rewritten in
                // versions/.
                let lease = self.minecraft.lock("minecraft_install")?;
                self.write_profile(spec).await?;
                self.minecraft.install_with(&spec.component_id(), verify, &lease, progress).await
            }
            LoaderKind::Forge | LoaderKind::NeoForge => {
                self.install_with_installer(spec, verify, progress).await
            }
        }
    }

    /// The Components page's Verify: every file against its hash, Java included,
    /// and for Forge and NeoForge also what their installer shipped and made (by the SHA-1s in its
    /// marker).
    pub async fn verify(&self, id: &str, spec: Option<&ComponentSpec>) -> VersionCheck {
        let mut check = self.minecraft.check_deep(id).await;
        let installer = spec.is_some_and(|s| matches!(s.kind, LoaderKind::Forge | LoaderKind::NeoForge));
        if installer && !processors::marker_ready(&self.mc_dir, id, true) {
            check.valid = false;
            check.issues.push("installer: its steps are not finished".into());
        }
        check
    }

    /// Repairs `id` with every file checked again: through its loader when `spec` names it, else
    /// as a plain version with the Minecraft installer.
    pub async fn repair(
        &self,
        id: &str,
        spec: Option<&ComponentSpec>,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        match spec.filter(|spec| spec.component_id() == id) {
            Some(spec) => self.install(spec, true, progress).await,
            None => self.minecraft.install(id, true, progress).await,
        }
    }

    /// Before a launch: a Fabric or Quilt profile that is missing or unreadable
    /// is fetched again; a Forge or NeoForge build that is not ready runs its installer steps
    /// again; then the usual check-and-repair runs.
    pub async fn ensure(
        &self,
        component: &str,
        spec: Option<&ComponentSpec>,
        force: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<InstalledVersion> {
        if let Some(spec) = spec.filter(|spec| spec.component_id() == component) {
            match spec.kind {
                LoaderKind::Fabric | LoaderKind::Quilt => {
                    if force || read_version_json(&self.mc_dir, component).is_err() {
                        tracing::warn!(
                            "Fetching the {} profile of {component} again",
                            spec.kind.display_name()
                        );
                        let _lease = self.minecraft.lock("minecraft_install")?;
                        self.write_profile(spec).await?;
                    }
                }
                LoaderKind::Forge | LoaderKind::NeoForge => {
                    let ready = if force { None } else { self.installer_ready(component).await };
                    if let Some(ready) = ready {
                        return Ok(ready);
                    }
                    tracing::warn!("Running the {} installer of {component} again", spec.kind.display_name());
                    return self.install_with_installer(spec, force, progress).await;
                }
                LoaderKind::Minecraft => {}
            }
        }
        self.minecraft.ensure_installed(component, force, progress).await
    }
}

pub type ComponentFuture<'a> = Pin<Box<dyn Future<Output = AppResult<InstalledVersion>> + Send + 'a>>;

/// How a module gets a Minecraft version or a loader installed: the launcher's
/// `ComponentInstaller` in the app, a fake in its tests.
pub trait ComponentSource: Send + Sync {
    /// Installs `spec` (the base Minecraft version with it), telling `progress` along the way.
    fn install<'a>(&'a self, spec: &'a ComponentSpec, progress: InstallProgressFn<'a>)
    -> ComponentFuture<'a>;
}

impl ComponentSource for ComponentInstaller {
    fn install<'a>(
        &'a self,
        spec: &'a ComponentSpec,
        progress: InstallProgressFn<'a>,
    ) -> ComponentFuture<'a> {
        Box::pin(ComponentInstaller::install(self, spec, false, progress))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_name_their_component() {
        let mut b = Build::new("Aero");
        b.version = Some("1.21.1".into());
        b.loader = Some("fabric-loader-0.16.9-1.21.1".into());
        b.client = Some("Fabric".into());
        b.loader_version = Some("0.16.9".into());
        let fabric = ComponentSpec::loader(LoaderKind::Fabric, "1.21.1", "0.16.9");
        assert_eq!(ComponentSpec::from_build(&b), Some(fabric.clone()));
        assert_eq!(fabric.component_id(), "fabric-loader-0.16.9-1.21.1");
        b.loader_version = None;
        assert_eq!(ComponentSpec::from_build(&b), Some(fabric), "the build is read from the id");
        b.loader = Some("quilt-loader-0.27.0-beta.1-1.21.1".into());
        b.client = Some("Quilt".into());
        assert_eq!(
            ComponentSpec::from_build(&b).and_then(|s| s.loader_version),
            Some("0.27.0-beta.1".into()),
            "dashes inside the loader version survive"
        );
        b.loader = Some("1.21.1".into());
        b.client = Some("Minecraft".into());
        assert_eq!(ComponentSpec::from_build(&b), Some(ComponentSpec::vanilla("1.21.1")));
        b.version = None;
        assert_eq!(ComponentSpec::from_build(&b), None);
    }
}
