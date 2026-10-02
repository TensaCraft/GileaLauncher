//! The launch pipeline: checks, the account, a repaired install, options, GPU,
//! the command and the process, then a watcher thread. A failure before the start frees the
//! launch slot, so a retry is never throttled.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use launcher_shared::{AppError, AppResult, ErrorCode, GameEvent, GameStartAction, GameState, Text};

use super::alive::{game_open, game_processes};
use super::hooks::{GameWatcher, LaunchContext, LaunchHook, PrepareContext, WatchedGame};
use super::ledger::{AdoptedProcess, Entry, Ledger, kill_process, process_started};
use super::monitor::{EARLY_EXIT, POLL, Watch, watch_game};
use super::options::{OptionsInput, assigned_profile, component_id, game_dir, launch_options, resolve_java};
use super::process::{GameCommand, SharedProcess, Spawner, prepare_launch_log};
use super::registry::{LAUNCH_COOLDOWN, LaunchRegistry};
use crate::auth::service::AuthService;
use crate::feedback::{EventSink, FeedbackService, OperationHandle, OperationSpec};
use crate::java::gpu::{GpuPreferenceStore, apply_gpu_mode, nvidia_driver_loaded};
use crate::java::memory::MemoryLimits;
use crate::loaders::{ComponentInstaller, ComponentSpec};
use crate::lock::{Coordinator, path_key};
use crate::minecraft::InstallProgress;
use crate::minecraft::assets::{AssetIndex, index_path};
use crate::minecraft::command::build_command;
use crate::minecraft::platform::GamePlatform;
use crate::minecraft::version::{VersionInfo, load_merged};
use crate::net::downloader::Downloader;
use crate::paths::Os;
use crate::storage::config::ConfigStore;
use crate::storage::transaction::recover_interrupted;
use crate::storage::versions::{Build, VersionStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchTimings {
    pub cooldown: Duration,
    pub early_exit: Duration,
    pub poll: Duration,
}

impl Default for LaunchTimings {
    fn default() -> Self {
        LaunchTimings { cooldown: LAUNCH_COOLDOWN, early_exit: EARLY_EXIT, poll: POLL }
    }
}

#[derive(Clone)]
pub struct LaunchDeps {
    pub mc_dir: PathBuf,
    pub platform: GamePlatform,
    pub versions: Arc<VersionStore>,
    pub components: Arc<ComponentInstaller>,
    pub auth: Arc<AuthService>,
    pub feedback: Arc<FeedbackService>,
    pub config: Arc<ConfigStore>,
    pub instances: Arc<Coordinator>,
    pub sink: Arc<dyn EventSink>,
    pub spawner: Arc<dyn Spawner>,
    pub gpu: Arc<dyn GpuPreferenceStore>,
    /// Modules' steps before every start (`LaunchHook`).
    pub hooks: Vec<Arc<dyn LaunchHook>>,
    /// For steps that bring a build up to date before it starts.
    pub downloader: Arc<Downloader>,
    /// Modules' eyes on every started game.
    pub watchers: Vec<Arc<dyn GameWatcher>>,
    pub timings: LaunchTimings,
    /// The file of the games started (`ledger::LEDGER_FILE` in the state folder): a launcher
    /// that restarts takes them back (`adopt`). None: they are known only while this one runs.
    pub ledger: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub build_key: String,
    /// `None`: the default profile.
    pub profile_key: Option<String>,
    /// Start although this build's folder already has a game running (the user confirmed).
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchStarted {
    pub pid: u32,
    pub java: PathBuf,
    pub game_dir: PathBuf,
}

pub struct LaunchService {
    deps: LaunchDeps,
    registry: Arc<LaunchRegistry>,
    ledger: Option<Arc<Ledger>>,
}

impl LaunchService {
    pub fn new(deps: LaunchDeps) -> LaunchService {
        let registry = Arc::new(LaunchRegistry::new(deps.timings.cooldown));
        let ledger = deps.ledger.clone().map(|path| Arc::new(Ledger::new(path)));
        LaunchService { deps, registry, ledger }
    }

    /// Takes back the games a launcher before this one started that still run; returns how many.
    /// Each is watched until it ends (then it leaves the registry and the ledger, and an `Exited`
    /// event without a code goes out); records of games gone leave the ledger.
    pub fn adopt(&self) -> usize {
        let Some(ledger) = &self.ledger else { return 0 };
        let alive = |e: &Entry| {
            let alive = process_started(e.pid).as_deref() == Some(e.started.as_str());
            if !alive {
                tracing::info!("Minecraft {} from before the restart is gone (pid {})", e.build_name, e.pid);
            }
            alive
        };
        if let Err(e) = ledger.retain(alive) {
            tracing::warn!("Unable to tidy the started games' ledger: {e}");
        }
        let mut adopted = 0;
        for entry in ledger.entries() {
            // Two games of one build are two entries: each is taken back.
            if self.registry.contains(entry.pid) {
                continue;
            }
            let pid = entry.pid;
            let process: SharedProcess =
                Arc::new(Mutex::new(Box::new(AdoptedProcess { pid, started: entry.started.clone() })));
            self.registry.register(&entry.key, process.clone());
            let (registry, sink, ledger, poll) =
                (self.registry.clone(), self.deps.sink.clone(), ledger.clone(), self.deps.timings.poll);
            let watched = std::thread::Builder::new().name(format!("game-{pid}")).spawn(move || {
                while matches!(process.lock().unwrap_or_else(|e| e.into_inner()).try_wait(), Ok(None)) {
                    std::thread::sleep(poll);
                }
                tracing::info!("Minecraft {} from before the restart ended (pid {pid})", entry.build_name);
                registry.take_stopped(pid);
                registry.unregister(&entry.key, pid);
                let _ = ledger.remove(pid);
                sink.game(&GameEvent {
                    build_key: entry.build_key,
                    build_name: entry.build_name,
                    state: GameState::Exited { code: None },
                });
            });
            match watched {
                Ok(_) => adopted += 1,
                Err(e) => tracing::warn!("Unable to watch the game from before the restart (pid {pid}): {e}"),
            }
        }
        adopted
    }

    fn registry_key(&self, build: &Build) -> String {
        path_key(&game_dir(build, &self.deps.mc_dir))
    }

    /// A game of this build is running: one the launcher started, or one that has its folder open
    /// (`game_open`: the launcher lost it, or another launcher started it).
    pub fn is_running(&self, build_key: &str) -> bool {
        self.deps.versions.get(build_key).is_some_and(|build| {
            self.registry.is_active(&self.registry_key(&build))
                || game_open(&game_dir(&build, &self.deps.mc_dir))
        })
    }

    /// Stops the build's games — its own and the Java processes that hold its folder
    /// (`game_processes`); returns how many were asked to stop.
    pub fn terminate(&self, build_key: &str) -> usize {
        let Some(build) = self.deps.versions.get(build_key) else { return 0 };
        let mut stopped = self.registry.terminate(&self.registry_key(&build));
        let strays: Vec<u32> = game_processes(&game_dir(&build, &self.deps.mc_dir))
            .into_iter()
            .filter(|pid| *pid != std::process::id() && !self.registry.contains(*pid))
            .collect();
        for pid in strays {
            match kill_process(pid) {
                Ok(()) => {
                    tracing::info!("Stopped Minecraft {} the launcher did not know (pid {pid})", build.name);
                    stopped += 1;
                    self.announce_end(&build, pid);
                }
                Err(e) => tracing::warn!("Unable to stop Minecraft {} (pid {pid}): {e}", build.name),
            }
        }
        stopped
    }

    /// Says a stopped game the launcher did not know has ended, once its process is gone.
    fn announce_end(&self, build: &Build, pid: u32) {
        let (sink, poll) = (self.deps.sink.clone(), self.deps.timings.poll);
        let (build_key, build_name) = (build.key.clone(), build.name.clone());
        let _ = std::thread::Builder::new().name(format!("game-{pid}")).spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while process_started(pid).is_some() && Instant::now() < deadline {
                std::thread::sleep(poll);
            }
            sink.game(&GameEvent { build_key, build_name, state: GameState::Exited { code: None } });
        });
    }

    /// Starts `request.build_key` and watches it on a thread of its own.
    pub async fn launch(&self, request: LaunchRequest) -> AppResult<LaunchStarted> {
        let d = &self.deps;
        let build = d.versions.get(&request.build_key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {}", request.build_key))
                .with_param("version", &request.build_key)
        })?;
        let folder = game_dir(&build, &d.mc_dir);
        let key = path_key(&folder);
        if !request.allow_duplicate && self.registry.is_active(&key) {
            return Err(AppError::new(ErrorCode::VersionRunning, "the build is running")
                .with_param("version", &build.name));
        }
        if d.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        if let Err(remaining) = self.registry.try_reserve(&key, Instant::now()) {
            let seconds = remaining.as_secs_f64().ceil().max(1.0) as u64;
            return Err(AppError::new(ErrorCode::LaunchThrottled, "launched moments ago")
                .with_param("version", &build.name)
                .with_param("seconds", seconds.to_string()));
        }
        let lease = match d.instances.try_acquire(&folder, "launch") {
            Ok(lease) => lease,
            Err(e) => {
                self.registry.release(&key);
                return Err(e.with_param("version", &build.name));
            }
        };
        let op = d.feedback.begin(OperationSpec::new(Text::key("syncing_files_check"), "launch").hidden());
        let result = self.start(&build, &request, &key, &op).await;
        op.finish();
        drop(lease);
        match result {
            Ok(started) => {
                d.feedback.info(Text::key("version_starting").param("version", &build.name));
                Ok(started)
            }
            Err(e) => {
                tracing::warn!("Launch of {} did not start: {}", build.name, e.detail);
                self.registry.release(&key);
                Err(e)
            }
        }
    }

    async fn start(
        &self,
        build: &Build,
        request: &LaunchRequest,
        key: &str,
        op: &OperationHandle,
    ) -> AppResult<LaunchStarted> {
        let d = &self.deps;
        // A file change that stopped half way (a crash, a killed launcher) is undone first; the
        // game still starts when it cannot be.
        let folder = game_dir(build, &d.mc_dir);
        let problems =
            tokio::task::spawn_blocking(move || recover_interrupted(&folder)).await.unwrap_or_default();
        for e in problems {
            tracing::warn!("An unfinished file change in {} could not be undone: {}", build.name, e.detail);
            d.feedback.warning(
                Text::key("content_recovery_failed").param("version", &build.name).param("error", e.detail),
            );
        }
        // Modules may bring the build up to date first (a server build syncs); the launch then goes
        // on with the build as saved.
        let prepared;
        let build = if d.hooks.is_empty() {
            build
        } else {
            let folder = game_dir(build, &d.mc_dir);
            let ctx = PrepareContext {
                build,
                game_dir: &folder,
                config: &d.config,
                feedback: &d.feedback,
                versions: &d.versions,
                components: d.components.as_ref(),
                downloader: &d.downloader,
                op,
                running: self.registry.is_active(key),
            };
            for hook in &d.hooks {
                hook.prepare_build(&ctx).await?;
            }
            prepared = d.versions.get(&build.key).ok_or_else(|| {
                AppError::new(ErrorCode::VersionNotFound, format!("no build {}", build.key))
                    .with_param("version", &build.key)
            })?;
            &prepared
        };
        // The account picked at Play, else the build's own while the launcher has it, else the default.
        let assigned = assigned_profile(&build.options).filter(|key| d.auth.has_profile(key));
        let identity = d.auth.launch_identity(request.profile_key.as_deref().or(assigned)).await?;
        let failed =
            |why: String| AppError::new(ErrorCode::LaunchFailed, why).with_param("version", &build.name);
        let component = component_id(build)
            .ok_or_else(|| failed("the build names no Minecraft version".into()))?
            .to_string();
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        let spec = ComponentSpec::from_build(build);
        let installed =
            d.components.ensure(&component, spec.as_ref(), false, &progress).await.map_err(|e| {
                match e.code {
                    // Only a version that cannot be found or read is an integrity problem ("reinstall");
                    // the network, Java, the disk and busy folders keep their own messages.
                    ErrorCode::VersionNotFound | ErrorCode::InvalidInput => failed(e.detail),
                    _ => e,
                }
            })?;
        let json = load_merged(&d.mc_dir, &component).map_err(|e| failed(e.detail))?;
        let info = VersionInfo::from_json(&json).map_err(|e| failed(e.detail))?;
        let java = resolve_java(build, installed.java, &d.mc_dir);
        let input = OptionsInput {
            build,
            identity: &identity,
            mc_dir: &d.mc_dir,
            component: &component,
            java: java.clone(),
            config: &d.config,
            limits: MemoryLimits::detect(),
        };
        let (options, gpu) = launch_options(input);
        fs::create_dir_all(&options.game_dir)
            .map_err(|e| failed(format!("{}: {e}", options.game_dir.display())))?;
        let ctx = LaunchContext {
            build,
            game_dir: &options.game_dir,
            mc_dir: &d.mc_dir,
            config: &d.config,
            feedback: &d.feedback,
        };
        for hook in &d.hooks {
            if let Err(e) = hook.before_launch(&ctx).await {
                tracing::warn!("A step before launching {} failed: {}", build.name, e.detail);
                d.feedback.warning(
                    Text::key("launch_hook_failed").param("version", &build.name).param("error", e.detail),
                );
            }
        }
        if let (Some(name), Some(_)) = (&info.asset_index_name, &info.asset_index) {
            let assets = d.mc_dir.join("assets");
            let (index, target) = (index_path(&assets, name), options.game_dir.clone());
            let copied = tokio::task::spawn_blocking(move || {
                AssetIndex::read(&index).and_then(|i| i.materialize_resources(&assets, &target))
            })
            .await;
            if let Ok(Err(e)) = copied {
                tracing::warn!("Unable to copy the legacy resources of {}: {}", build.name, e.detail);
            }
        }
        let nvidia = d.platform.os == Os::Linux && nvidia_driver_loaded();
        let env = apply_gpu_mode(gpu, &java, d.platform.os, d.gpu.as_ref(), nvidia);
        let argv = build_command(&d.mc_dir, &json, build.version.as_deref(), &options, &d.platform)
            .map_err(|e| failed(e.detail))?;
        let minecraft = build.version.clone().unwrap_or_else(|| component.clone());
        let log = prepare_launch_log(&options.game_dir, &component, &minecraft)
            .inspect_err(|e| tracing::warn!("Unable to start the launch log of {}: {e}", build.name))
            .ok();
        let command = GameCommand::new(argv, &options.game_dir, env, log);
        let launched_at = SystemTime::now();
        let process =
            d.spawner.spawn(&command).map_err(|e| failed(format!("{}: {e}", command.program.display())))?;
        let pid = process.pid();
        tracing::info!(
            "Minecraft started: pid={pid} build={} component={component} java={} dir={}",
            build.name,
            java.display(),
            options.game_dir.display()
        );
        let shared: SharedProcess = Arc::new(Mutex::new(process));
        self.registry.register(key, shared.clone());
        if let (Some(ledger), Some(started)) = (&self.ledger, process_started(pid)) {
            let entry = Entry {
                key: key.to_string(),
                build_key: build.key.clone(),
                build_name: build.name.clone(),
                pid,
                started,
            };
            if let Err(e) = ledger.add(entry) {
                tracing::warn!("Unable to note the started game (pid {pid}): {e}");
            }
        }
        d.sink.game(&GameEvent {
            build_key: build.key.clone(),
            build_name: build.name.clone(),
            state: GameState::Started { pid },
        });
        let watch = Watch {
            key: key.to_string(),
            build_key: build.key.clone(),
            build_name: build.name.clone(),
            game_dir: options.game_dir.clone(),
            launched_at,
            // The launcher goes (quits or hides in the tray) once the game runs.
            close_launcher: crate::settings::game_start_action(&d.config) != GameStartAction::Nothing,
            early_exit: d.timings.early_exit,
            poll: d.timings.poll,
        };
        let game = WatchedGame {
            build_key: build.key.clone(),
            build_name: build.name.clone(),
            game_dir: options.game_dir.clone(),
            launched_at,
        };
        let watches = d.watchers.iter().map(|w| w.watch(&game)).collect();
        let (registry, feedback, sink) = (self.registry.clone(), d.feedback.clone(), d.sink.clone());
        let ledger = self.ledger.clone();
        std::thread::Builder::new()
            .name(format!("game-{pid}"))
            .spawn(move || {
                watch_game(shared, watch, watches, &registry, &feedback, sink.as_ref());
                if let Some(ledger) = ledger {
                    let _ = ledger.remove(pid);
                }
            })
            .map_err(|e| failed(e.to_string()))?;
        Ok(LaunchStarted { pid, java, game_dir: options.game_dir })
    }
}
