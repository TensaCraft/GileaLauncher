//! Creating builds: a vanilla build is its Minecraft version installed (shared in the Minecraft
//! folder) plus a folder of its own in `games/` with its `version.json`.
//! The record appears only once the files are in place, for installs and copies alike.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use launcher_shared::{
    AppError, AppResult, BuildDto, BuildSettingsDto, BuildSettingsUpdate, BuildsSnapshot, DESCRIPTION_MAX,
    ErrorCode, LoaderKind, Text, mods_supported,
};
use serde_json::{Value, json};

use crate::builds::components::classify;
use crate::builds::ids::validate_component_id;
use crate::builds::settings::{apply_options, read_icon, settings_of};
use crate::content::inventory::count_mods;
use crate::feedback::{FeedbackService, OperationHandle, OperationSpec};
use crate::java::preferences::resolve_java_executable;
use crate::launch::alive::game_open;
use crate::launch::options::{component_id, game_dir};
use crate::loaders::{ComponentInstaller, ComponentSpec};
use crate::lock::Coordinator;
use crate::minecraft::InstallProgress;
use crate::minecraft::version::read_version_json;
use crate::paths::Os;
use crate::storage::config::ConfigStore;
use crate::storage::versions::{Build, RECORD_FILE, VersionStore};

pub const GPU_MODE_DEFAULT_KEY: &str = "gpu_mode_default";

/// The order the user put the builds in (their keys).
pub const BUILD_ORDER_KEY: &str = "build_order";

/// Builds in the user's `order` (build keys), the others after them by name.
pub fn ordered(builds: &mut [BuildDto], order: &[String]) {
    builds.sort_by_key(|b| {
        (order.iter().position(|k| *k == b.key).unwrap_or(usize::MAX), b.name.to_lowercase())
    });
}

fn saved_order(config: &ConfigStore) -> Vec<String> {
    config.get(BUILD_ORDER_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Set once the builds' GPU was brought to the system's default (`migrate_gpu_default`).
pub const GPU_AUTO_MIGRATED_KEY: &str = "gpu_auto_migrated";

/// Once, when Windows began to choose the GPU by itself: there, with no default the user picked,
/// builds that got the old `dgpu` default go to `auto`, so nothing is written to the registry
/// until the user picks a GPU (and is told what that does). Returns how many builds changed.
pub fn migrate_gpu_default(config: &ConfigStore, versions: &VersionStore, os: Os) -> usize {
    // A config that could not be read is never written over (its settings would be lost).
    if !config.is_healthy() || config.get_bool(GPU_AUTO_MIGRATED_KEY, false) {
        return 0;
    }
    let mut changed = 0;
    if os == Os::Windows && config.get_str(GPU_MODE_DEFAULT_KEY).is_none() {
        for mut build in versions.list() {
            if build.options.get("gpuMode").and_then(Value::as_str) != Some("dgpu") {
                continue;
            }
            build.options.insert("gpuMode".into(), json!("auto"));
            match versions.save(&mut build) {
                Ok(()) => changed += 1,
                Err(e) => {
                    tracing::warn!("Unable to let Windows choose the GPU of {}: {}", build.name, e.detail)
                }
            }
        }
    }
    if let Err(e) = config.set_bool(GPU_AUTO_MIGRATED_KEY, true) {
        tracing::warn!("Unable to note the GPU migration: {e}");
    }
    changed
}

/// `gpu_mode_default` from the config: `auto`, `igpu` or `dgpu`; unset, the system's default
/// (`auto` on Windows: nothing goes to the registry until the user picks a GPU).
pub fn default_gpu_mode(config: &ConfigStore) -> &'static str {
    match config.get_str(GPU_MODE_DEFAULT_KEY).as_deref() {
        Some("auto") => "auto",
        Some("igpu") => "igpu",
        Some("dgpu") => "dgpu",
        _ => crate::java::gpu::platform_default(Os::current()).as_str(),
    }
}

/// The mod loader a build had before it went back to vanilla (`LoaderKind`, in lower case).
pub const LAST_LOADER_OPTION: &str = "lastLoader";

/// The mod loader `build` had before it went back to vanilla, if it did.
pub fn last_loader(build: &Build) -> Option<LoaderKind> {
    let kind = serde_json::from_value(build.options.get(LAST_LOADER_OPTION)?.clone()).ok()?;
    (kind != LoaderKind::Minecraft).then_some(kind)
}

/// The mod loader `build` runs: its component's, else its client's (vanilla when neither says).
fn loader_of(build: &Build) -> LoaderKind {
    component_id(build)
        .and_then(LoaderKind::of_component)
        .or_else(|| build.client.as_deref().and_then(LoaderKind::from_client))
        .unwrap_or(LoaderKind::Minecraft)
}

/// Notes on `build`, about to go to vanilla (`vanilla`) or to a loader, the loader it leaves: one
/// going back to vanilla keeps the name of its loader, one taking a loader needs it no more.
fn note_last_loader(build: &mut Build, vanilla: bool) {
    match loader_of(build) {
        _ if !vanilla => {
            build.options.remove(LAST_LOADER_OPTION);
        }
        LoaderKind::Minecraft => {}
        from => {
            build.options.insert(LAST_LOADER_OPTION.into(), json!(from));
        }
    }
}

/// A module's say over the builds it sets up itself (a server's): their loader is its own, and the
/// player never changes it by hand.
pub trait LoaderKeeper: Send + Sync {
    /// Why `build` (its game folder `game_dir`) keeps its loader, as a translation key; `None`
    /// when the player may change it.
    fn kept(&self, build: &Build, game_dir: &Path) -> Option<&'static str>;
}

/// What a copy of a build leaves behind, by its path in the build: the record (the copy gets its
/// own), the source's journals, work files, claim, launch log and downloaded modpack, and what the
/// version before kept in the root and in `mods/.backups`. The notes in `.launcher/` go along: a
/// copied modpack build stays one.
pub fn copy_skips(relative: &str) -> bool {
    let lower = relative.to_lowercase();
    let under = |folder: &str| lower == folder || lower.starts_with(&format!("{folder}/"));
    lower == RECORD_FILE.to_lowercase()
        || under(".launcher/journal")
        || under(".launcher/work")
        || matches!(lower.as_str(), ".launcher/claim.json" | ".launcher/launch.log")
        || lower.starts_with(".launcher/modpack.")
        || lower.starts_with(".launcher-")
        || under("mods/.backups")
}

/// Copies everything inside `from` into `to`, except what `skip` names by its path in `from`
/// (relative, `/`-separated). Links are skipped (never followed); folders `to` has are filled, and
/// an existing file in `to` is an error rather than overwritten.
pub fn copy_dir_contents(from: &Path, to: &Path, skip: &dyn Fn(&str) -> bool) -> io::Result<()> {
    copy_below(from, to, "", skip)
}

fn copy_below(from: &Path, to: &Path, prefix: &str, skip: &dyn Fn(&str) -> bool) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if skip(&relative) {
            continue;
        }
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            tracing::warn!("Not copying link {}", entry.path().display());
        } else if kind.is_dir() {
            fs::create_dir_all(&target)?;
            copy_below(&entry.path(), &target, &relative, skip)?;
        } else {
            let mut source = fs::File::open(entry.path())?;
            let mut copy = fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            io::copy(&mut source, &mut copy)?;
        }
    }
    Ok(())
}

pub struct BuildService {
    versions: Arc<VersionStore>,
    components: Arc<ComponentInstaller>,
    feedback: Arc<FeedbackService>,
    config: Arc<ConfigStore>,
    instances: Arc<Coordinator>,
    keepers: Vec<Arc<dyn LoaderKeeper>>,
}

impl BuildService {
    pub fn new(
        versions: Arc<VersionStore>,
        components: Arc<ComponentInstaller>,
        feedback: Arc<FeedbackService>,
        config: Arc<ConfigStore>,
        instances: Arc<Coordinator>,
    ) -> BuildService {
        BuildService { versions, components, feedback, config, instances, keepers: Vec::new() }
    }

    /// The modules' say over which builds keep their loader (`Module::loader_keeper`).
    pub fn with_loader_keepers(mut self, keepers: Vec<Arc<dyn LoaderKeeper>>) -> BuildService {
        self.keepers = keepers;
        self
    }

    /// Why a module keeps `build`'s loader (a translation key), if one does.
    pub fn kept_loader(&self, build: &Build) -> Option<&'static str> {
        let folder = game_dir(build, self.versions.minecraft_dir());
        self.keepers.iter().find_map(|keeper| keeper.kept(build, &folder))
    }

    /// `build` as the UI lists it, with what its folder tells: the mods a build without a mod
    /// loader still holds.
    pub fn dto(&self, build: &Build, running: bool) -> BuildDto {
        let folder = game_dir(build, self.versions.minecraft_dir());
        let leftover = if mods_supported(build.client.as_deref()) { 0 } else { count_mods(&folder) };
        BuildDto {
            key: build.key.clone(),
            version_id: build.version_id.clone(),
            name: build.name.clone(),
            version: build.version.clone(),
            loader: build.loader.clone(),
            client: build.client.clone(),
            loader_version: build.loader_version.clone(),
            game_dir: folder.to_string_lossy().into_owned(),
            image: crate::builds::settings::image_src(build.image.as_deref()),
            description: build.description.clone(),
            running,
            profile: crate::launch::options::assigned_profile(&build.options).map(str::to_string),
            loader_kept: self.kept_loader(build).map(str::to_string),
            last_loader: last_loader(build),
            leftover_mods: u32::try_from(leftover).unwrap_or(u32::MAX),
        }
    }

    /// Every build in the user's order (the rest by name); `running` tells which build keys have
    /// a game running.
    pub fn snapshot(&self, running: impl Fn(&Build) -> bool) -> BuildsSnapshot {
        let mut builds: Vec<BuildDto> =
            self.versions.list().iter().map(|b| self.dto(b, running(b))).collect();
        ordered(&mut builds, &saved_order(&self.config));
        BuildsSnapshot { builds }
    }

    /// Keeps `keys` as the order of the builds (Home and Builds show them so).
    pub fn reorder(&self, keys: &[String]) -> AppResult<()> {
        self.config.set(BUILD_ORDER_KEY, json!(keys)).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))
    }

    /// The trimmed new name, refusing an empty one or one another build has (ignoring case).
    fn check_new_name<'a>(&self, name: &'a str) -> AppResult<&'a str> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new(ErrorCode::VersionNameEmpty, "the build name is empty"));
        }
        let lower = name.to_lowercase();
        if self.versions.list().iter().any(|b| b.name.trim().to_lowercase() == lower) {
            return Err(AppError::new(ErrorCode::VersionExists, "a build with this name exists")
                .with_param("name", name));
        }
        Ok(name)
    }

    /// Installs Minecraft `mc_version` and registers a build named `name` for it, with its own
    /// game folder, the managed Java and the default GPU mode. Refuses an empty or taken name and
    /// any install while another operation runs (`installation_already_running`).
    pub async fn install_vanilla(&self, name: &str, mc_version: &str) -> AppResult<Build> {
        self.install_build(name, ComponentSpec::vanilla(mc_version)).await
    }

    /// `install_vanilla` for a loader build (Fabric and Quilt).
    pub async fn install_loader(
        &self,
        name: &str,
        kind: LoaderKind,
        mc_version: &str,
        loader_version: &str,
    ) -> AppResult<Build> {
        self.install_build(name, ComponentSpec::loader(kind, mc_version, loader_version)).await
    }

    async fn install_build(&self, name: &str, spec: ComponentSpec) -> AppResult<Build> {
        let name = self.check_new_name(name)?;
        if self.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        let status = match spec.kind {
            LoaderKind::Minecraft => Text::key("installing_minecraft_version").param("version", &spec.mc),
            kind => Text::key("version_component_installing")
                .param("loader", kind.display_name())
                .param("version", &spec.mc),
        };
        let op = self
            .feedback
            .begin(OperationSpec::new(Text::key("installation_started"), "install").status(status));
        match self.install_and_register(name, &spec, &op).await {
            Ok(build) => {
                op.finish();
                self.feedback.success(Text::key("version_install_success").param("version", name));
                Ok(build)
            }
            Err(e) => {
                tracing::error!("Installing build {name} ({}) failed: {}", spec.component_id(), e.detail);
                op.fail(
                    Text::key("version_install_error")
                        .param("client", spec.kind.display_name())
                        .param("version", name)
                        .param("error", e.detail.clone()),
                );
                Err(e)
            }
        }
    }

    async fn install_and_register(
        &self,
        name: &str,
        spec: &ComponentSpec,
        op: &OperationHandle,
    ) -> AppResult<Build> {
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        let installed = self.components.install(spec, false, &progress).await?;
        let mut build = Build::new(name);
        build.version = Some(spec.mc.clone());
        build.loader = Some(installed.id.clone());
        build.client = Some(spec.kind.display_name().to_string());
        build.loader_version = spec.loader_version.clone();
        build.options.insert("gpuMode".into(), json!(default_gpu_mode(&self.config)));
        if let Some(java) = &installed.java {
            build.options.insert("executablePath".into(), json!(java.to_string_lossy()));
        }
        self.versions.create(&mut build)?;
        Ok(build)
    }

    /// Copies build `key` as a new build `name`: its game folder goes to a new `games/<id>` (never
    /// an existing folder) and its settings come along.
    /// Refused while its game runs (`running()`, or `game_open`: a game the launcher does not know has its folder open).
    pub async fn copy(&self, key: &str, name: &str, running: impl Fn() -> bool) -> AppResult<Build> {
        let name = self.check_new_name(name)?;
        let source = self.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })?;
        if self.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        let from = game_dir(&source, self.versions.minecraft_dir());
        let lease = self.instances.try_acquire(&from, "copy")?;
        if running() || game_open(&from) {
            return Err(running_error(&source));
        }
        let op = self
            .feedback
            .begin(OperationSpec::new(Text::key("copy_in_progress").param("version", &source.name), "copy"));
        let result = self.copy_and_register(&source, name, &from).await;
        drop(lease);
        match result {
            Ok(build) => {
                op.finish();
                self.feedback.success(Text::key("version_copy_success").param("version", name));
                Ok(build)
            }
            Err(e) => {
                tracing::error!("Copying build {} as {name} failed: {}", source.name, e.detail);
                op.fail(
                    Text::key("version_copy_error").param("version", name).param("error", e.detail.clone()),
                );
                Err(e)
            }
        }
    }

    async fn copy_and_register(&self, source: &Build, name: &str, from: &Path) -> AppResult<Build> {
        let mut build = Build::new(name);
        build.version = source.version.clone();
        build.loader = source.loader.clone();
        build.client = source.client.clone();
        build.loader_version = source.loader_version.clone();
        build.force_update = source.force_update;
        build.options = source.options.clone();
        build.image = source.image.clone();
        build.description = source.description.clone();
        // A copy of a build a server manages is the player's own: nothing syncs it, and it names
        // the loader it runs.
        if source.options.get("managedByApi") == Some(&Value::Bool(true)) {
            build.options.insert("managedByApi".into(), Value::Bool(false));
            build.options.insert("syncMode".into(), Value::String("manual".into()));
            build.force_update = false;
            let kind =
                source.loader.as_deref().and_then(LoaderKind::of_component).unwrap_or(LoaderKind::Minecraft);
            build.client = Some(kind.display_name().to_string());
        }
        // The copy becomes a build only once its files are in place: an interrupted copy leaves a
        // folder without a record, never a build that looks whole.
        let id = self.versions.claim_folder(name)?;
        let to = self.versions.games_dir().join(&id);
        let (src, dst) = (from.to_path_buf(), to.clone());
        let registered = tokio::task::spawn_blocking(move || {
            if src.is_dir() { copy_dir_contents(&src, &dst, &copy_skips) } else { Ok(()) }
        })
        .await
        .map_err(|e| AppError::internal(e.to_string()))
        .and_then(|r| {
            r.map_err(|e| {
                AppError::new(ErrorCode::Io, e.to_string()).with_param("path", from.to_string_lossy())
            })
        })
        .and_then(|()| self.versions.create_in(&mut build, &id));
        if let Err(e) = registered {
            let _ = fs::remove_dir_all(&to);
            return Err(e);
        }
        Ok(build)
    }

    fn find(&self, key: &str) -> AppResult<Build> {
        self.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })
    }

    /// The settings page's view of build `key`.
    pub fn settings(&self, key: &str) -> AppResult<BuildSettingsDto> {
        let build = self.find(key)?;
        let auto_java = component_id(&build).and_then(|c| self.components.minecraft().managed_java(c));
        Ok(settings_of(&build, self.versions.minecraft_dir(), auto_java))
    }

    /// Saves the settings page: a non-empty name no other build has, an installed component, a
    /// real Java file, a valid port and an image icon — or nothing at all. The key and the game
    /// folder stay. It holds the build's folder and writes over the record as it is then, so a
    /// loader changed meanwhile stays; another component is a loader change, guarded as
    /// `change_component` is (`running()` tells the build's game runs).
    pub fn update_settings(
        &self,
        key: &str,
        update: BuildSettingsUpdate,
        running: impl Fn() -> bool,
    ) -> AppResult<BuildSettingsDto> {
        let folder = game_dir(&self.find(key)?, self.versions.minecraft_dir());
        let _lease = self.instances.try_acquire(&folder, "settings")?;
        let mut build = self.find(key)?;
        let name = update.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::new(ErrorCode::VersionNameEmpty, "the build name is empty"));
        }
        let lower = name.to_lowercase();
        if self.versions.list().iter().any(|b| b.key != build.key && b.name.trim().to_lowercase() == lower) {
            return Err(AppError::new(ErrorCode::VersionExists, "a build with this name exists")
                .with_param("name", &name));
        }
        let mut update = update;
        if let Some(raw) = update.java_path.clone().filter(|p| !p.trim().is_empty()) {
            update.java_path = Some(resolve_java_executable(&raw)?.to_string_lossy().into_owned());
        }
        let component = update.component.as_deref().map(str::trim).filter(|c| !c.is_empty());
        if let Some(component) = component.filter(|c| build.loader.as_deref() != Some(*c)) {
            self.may_change_loader(&build, &folder, &running)?;
            validate_component_id(component)?;
            let json = read_version_json(self.versions.minecraft_dir(), component).map_err(|_| {
                AppError::new(ErrorCode::VersionNotFound, format!("no component {component}"))
                    .with_param("version", component)
            })?;
            let (loader, minecraft, loader_version) = classify(component, &json);
            note_last_loader(&mut build, loader == Some(LoaderKind::Minecraft));
            build.loader = Some(component.to_string());
            if let Some(mc) = minecraft {
                build.version = Some(mc);
            }
            if let Some(kind) = loader {
                build.client = Some(kind.display_name().to_string());
            }
            build.loader_version = loader_version;
        }
        apply_options(&mut build, &update)?;
        if update.remove_image {
            build.image = None;
        }
        if let Some(path) = update.image_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            build.image = Some(read_icon(Path::new(path))?);
        }
        if let Some(description) = &update.description {
            build.description = description.trim().chars().take(DESCRIPTION_MAX).collect();
        }
        build.name = name;
        self.versions.save(&mut build)?;
        self.feedback.success(Text::key("version_updated"));
        self.settings(key)
    }

    /// A change of `build`'s loader, under its folder's lease (`folder`): refused for a build a
    /// module keeps, while another operation runs and while its game runs (`running()`, or
    /// `game_open`: a game the launcher does not know has its folder open).
    fn may_change_loader(&self, build: &Build, folder: &Path, running: &impl Fn() -> bool) -> AppResult<()> {
        if self.kept_loader(build).is_some() {
            return Err(kept_error(build));
        }
        if self.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        if running() || game_open(folder) {
            return Err(running_error(build));
        }
        Ok(())
    }

    /// The Minecraft version `build` runs: its own, else the one its component names.
    fn minecraft_of(&self, build: &Build) -> AppResult<String> {
        let own = build.version.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let of_component = || {
            let id = component_id(build).filter(|id| validate_component_id(id).is_ok())?;
            let json = read_version_json(self.versions.minecraft_dir(), id).ok()?;
            classify(id, &json).1
        };
        own.or_else(of_component).ok_or_else(|| {
            AppError::new(ErrorCode::InvalidInput, format!("{} names no Minecraft version", build.name))
                .with_param("version", &build.name)
        })
    }

    /// Puts mod loader `kind` (its build `loader_version`) on build `key` for the build's own
    /// Minecraft version, or takes the build back to vanilla (`LoaderKind::Minecraft`), as
    /// `change_component` does. The game folder stays as it is, mods of another loader too.
    pub async fn set_loader(
        &self,
        key: &str,
        kind: LoaderKind,
        loader_version: Option<&str>,
        running: impl Fn() -> bool,
    ) -> AppResult<BuildSettingsDto> {
        let mc = self.minecraft_of(&self.find(key)?)?;
        self.change_component(key, kind, &mc, loader_version, running).await
    }

    /// "Install and use": installs the build of `kind` for Minecraft `mc` and
    /// points build `key` at it — the record changes only once the install succeeded. Refused
    /// for a build a module keeps the loader of, and while the build's game runs (`running()`, or
    /// `game_open`); its folder is held meanwhile, so no game starts on a build half changed.
    pub async fn change_component(
        &self,
        key: &str,
        kind: LoaderKind,
        mc: &str,
        loader_version: Option<&str>,
        running: impl Fn() -> bool,
    ) -> AppResult<BuildSettingsDto> {
        let build = self.find(key)?;
        if self.kept_loader(&build).is_some() {
            return Err(kept_error(&build));
        }
        let spec = match (kind, loader_version) {
            (LoaderKind::Minecraft, _) => ComponentSpec::vanilla(mc),
            (kind, Some(lv)) => ComponentSpec::loader(kind, mc, lv),
            (kind, None) => {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("{} needs a loader build", kind.display_name()),
                )
                .with_param("version", mc));
            }
        };
        let folder = game_dir(&build, self.versions.minecraft_dir());
        let _lease = self.instances.try_acquire(&folder, "loader")?;
        self.may_change_loader(&self.find(key)?, &folder, &running)?;
        let (title, status) = match &spec.loader_version {
            None => (
                Text::key("build_loader_removing").param("build", &build.name),
                Text::key("installing_minecraft_version").param("version", mc),
            ),
            Some(lv) => (
                Text::key("build_loader_installing")
                    .param("build", &build.name)
                    .param("loader", kind.display_name())
                    .param("version", lv),
                Text::key("version_component_installing")
                    .param("loader", kind.display_name())
                    .param("version", mc),
            ),
        };
        let op = self.feedback.begin(OperationSpec::new(title, "install").status(status));
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        let installed = match self.components.install(&spec, false, &progress).await {
            Ok(installed) => installed,
            Err(e) => {
                op.fail(Text::key("version_component_install_failed").param("error", e.detail.clone()));
                return Err(e);
            }
        };
        op.finish();
        let mut build = self.find(key)?;
        note_last_loader(&mut build, kind == LoaderKind::Minecraft);
        build.loader = Some(installed.id);
        build.version = Some(spec.mc.clone());
        build.client = Some(kind.display_name().to_string());
        build.loader_version = spec.loader_version.clone();
        // The new component runs on its own managed Java, like a new build.
        if let Some(java) = &installed.java {
            build.options.insert("executablePath".into(), json!(java.to_string_lossy()));
        }
        self.versions.save(&mut build)?;
        let done = match &spec.loader_version {
            None => Text::key("build_loader_removed").param("build", &build.name).param("mc", mc),
            Some(lv) => Text::key("build_loader_set")
                .param("build", &build.name)
                .param("loader", kind.display_name())
                .param("version", lv)
                .param("mc", mc),
        };
        self.feedback.success(done);
        self.settings(key)
    }

    /// Forgets build `key`; with `delete_files` also its game folder — refused while `running()`
    /// or a game the launcher does not know (one it started before it restarted) has the folder
    /// open (`game_open`), checked under the folder's lock so no launch slips in between.
    pub fn delete(&self, key: &str, delete_files: bool, running: impl Fn() -> bool) -> AppResult<()> {
        let build = self.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })?;
        let folder = game_dir(&build, self.versions.minecraft_dir());
        let _lease = self.instances.try_acquire(&folder, "delete")?;
        if delete_files && (running() || game_open(&folder)) {
            return Err(running_error(&build));
        }
        self.versions.remove(&build.key, delete_files)?;
        self.feedback.success(Text::key("version_deleted"));
        Ok(())
    }
}

fn running_error(build: &Build) -> AppError {
    AppError::new(ErrorCode::BuildRunning, "the build's game is running").with_param("version", &build.name)
}

fn kept_error(build: &Build) -> AppError {
    AppError::new(ErrorCode::LoaderKept, "a module keeps the build's loader")
        .with_param("version", &build.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gpu_default_comes_from_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path().join("config.json"));
        let system = crate::java::gpu::platform_default(crate::paths::Os::current()).as_str();
        assert_eq!(default_gpu_mode(&config), system, "unset: the system's default");
        for (value, mode) in [("auto", "auto"), ("igpu", "igpu"), ("dgpu", "dgpu"), ("rtx", system)] {
            config.set(GPU_MODE_DEFAULT_KEY, json!(value)).unwrap();
            assert_eq!(default_gpu_mode(&config), mode, "{value}");
        }
    }

    #[test]
    fn builds_with_the_old_gpu_default_let_windows_choose_once() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path().join("config.json"));
        let versions = VersionStore::open(&dir.path().join("state"), &dir.path().join("mc"));
        let build = |name: &str, gpu: &str| {
            let mut b = Build::new(name);
            b.options.insert("gpuMode".into(), json!(gpu));
            versions.save(&mut b).unwrap();
            b.key
        };
        let (old, chosen) = (build("Aero", "dgpu"), build("Laptop", "igpu"));
        let gpu = |key: &str| versions.get(key).unwrap().options["gpuMode"].clone();
        assert_eq!(migrate_gpu_default(&config, &versions, Os::Windows), 1);
        assert_eq!((gpu(&old), gpu(&chosen)), (json!("auto"), json!("igpu")), "only the old default");
        // Once: a GPU the user picks later stays.
        let later = build("Later", "dgpu");
        assert_eq!(migrate_gpu_default(&config, &versions, Os::Windows), 0);
        assert_eq!(gpu(&later), json!("dgpu"));
    }

    #[test]
    fn a_gpu_default_the_user_picked_or_another_system_keeps_its_builds() {
        let dir = tempfile::tempdir().unwrap();
        let versions = VersionStore::open(&dir.path().join("state"), &dir.path().join("mc"));
        let mut b = Build::new("Aero");
        b.options.insert("gpuMode".into(), json!("dgpu"));
        versions.save(&mut b).unwrap();
        let picked = ConfigStore::open(dir.path().join("picked.json"));
        picked.set(GPU_MODE_DEFAULT_KEY, json!("dgpu")).unwrap();
        assert_eq!(migrate_gpu_default(&picked, &versions, Os::Windows), 0, "the user's own default");
        let linux = ConfigStore::open(dir.path().join("linux.json"));
        assert_eq!(migrate_gpu_default(&linux, &versions, Os::Linux), 0, "no registry there");
        assert_eq!(versions.get(&b.key).unwrap().options["gpuMode"], json!("dgpu"));
    }

    #[test]
    fn folder_copies_skip_links_and_keep_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::create_dir_all(from.join("mods").join("nested")).unwrap();
        fs::write(from.join("mods").join("nested").join("a.jar"), b"a").unwrap();
        fs::write(from.join("options.txt"), b"new").unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("options.txt"), b"old").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), from.join("escape")).unwrap();
        fs::write(from.join("version.json"), b"{}").unwrap();
        let err = copy_dir_contents(&from, &to, &|_| false).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(to.join("options.txt")).unwrap(), b"old");
        fs::remove_file(to.join("options.txt")).unwrap();
        fs::remove_dir_all(to.join("mods")).ok();
        fs::remove_file(to.join("version.json")).ok();
        copy_dir_contents(&from, &to, &|path| path == "version.json").unwrap();
        assert_eq!(fs::read(to.join("mods").join("nested").join("a.jar")).unwrap(), b"a");
        assert!(!to.join("version.json").exists(), "skipped names are not copied");
        assert!(!to.join("escape").exists(), "links are not followed");
    }
}
