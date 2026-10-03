//! Wires paths, config, logging, feedback, modules and settings together.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use launcher_shared::branding::{
    DEFAULT_UPDATE_API, ISSUES_URL, MS_CLIENT_ID, PROFILE, SUPPORT_URL, UPDATE_API, UPDATE_REPO, VERSION,
};
use launcher_shared::{AppInfo, AppResult, PathsInfo, SetupPlan, SetupPreview, SetupState, Text};
use serde_json::json;

use crate::auth::service::AuthService;
use crate::auth::{AuthConfig, UrlOpener};
use crate::builds::components::ComponentManager;
use crate::builds::service::BuildService;
use crate::content::service::ContentService;
use crate::feedback::{EventSink, FeedbackService};
use crate::java::discovery::DiscoveryRoots;
use crate::java::gpu::WindowsGpuPreferences;
use crate::java::preferences::JavaService;
use crate::java::runtime::JavaRuntimes;
use crate::launch::ledger::LEDGER_FILE;
use crate::launch::process::SystemSpawner;
use crate::launch::service::{LaunchDeps, LaunchService, LaunchTimings};
use crate::loaders::ComponentInstaller;
use crate::loaders::LoaderEndpoints;
use crate::lock::Coordinator;
use crate::minecraft::install::MinecraftInstaller;
use crate::minecraft::manifest::MojangEndpoints;
use crate::minecraft::platform::GamePlatform;
use crate::modules::{Module, ModuleContext, ModuleRegistry};
use crate::net::downloader::{Downloader, DownloaderConfig};
use crate::net::meta::{META_TTL, MetaClient};
use crate::paths::{LauncherPaths, Os, PathEnv, default_log_dir};
use crate::platform::shortcuts::ShortcutService;
use crate::recent::RecentService;
use crate::settings::{MINECRAFT_DIR_KEY, SettingsService};
use crate::setup::{self, SetupOutcome};
use crate::storage::config::ConfigStore;
use crate::storage::versions::VersionStore;
use crate::updater::select::Platform;
use crate::updater::stage::{self as update_stage, ExecContext};
use crate::updater::version::Version;
use crate::updater::{UpdateConfig, UpdateService};

pub const LAST_RUN_VERSION_KEY: &str = "last_run_version";

/// Returns the previous version when this run is the first one after an update, and records the
/// current version (never rewriting a damaged config).
fn track_version(config: &ConfigStore) -> Option<String> {
    let previous = config.get_str(LAST_RUN_VERSION_KEY);
    let updated_from = match (previous.as_deref().and_then(Version::parse), Version::parse(VERSION)) {
        (Some(prev), Some(current)) if prev < current => previous.clone(),
        _ => None,
    };
    if previous.as_deref() != Some(VERSION) && config.is_healthy() {
        let _ = config.set(LAST_RUN_VERSION_KEY, json!(VERSION));
    }
    updated_from
}

pub struct BootstrapOptions {
    pub init_logging: bool,
    pub system_lang: String,
    /// Opens Microsoft sign-in pages; `auth::NoOpener` for headless runs.
    pub opener: Arc<dyn UrlOpener>,
}

pub struct CoreApp {
    pub env: PathEnv,
    pub paths: LauncherPaths,
    pub config: Arc<ConfigStore>,
    pub feedback: Arc<FeedbackService>,
    pub modules: ModuleRegistry,
    pub settings: SettingsService,
    pub auth: Arc<AuthService>,
    /// Build records (`games/<id>/version.json`).
    pub versions: Arc<VersionStore>,
    /// Locks on game folders.
    pub instances: Arc<Coordinator>,
    /// Lock on the shared Minecraft folder.
    pub shared: Arc<Coordinator>,
    pub downloader: Arc<Downloader>,
    /// Mojang metadata with its one-hour cache.
    pub meta: Arc<MetaClient>,
    /// Mojang's Java runtimes in the Minecraft folder.
    pub java: Arc<JavaRuntimes>,
    pub minecraft: Arc<MinecraftInstaller>,
    pub components: Arc<ComponentInstaller>,
    pub component_manager: Arc<ComponentManager>,
    pub builds: Arc<BuildService>,
    /// A build's installed mods and packs.
    pub content: Arc<ContentService>,
    /// Starts builds and watches running games.
    pub launcher: Arc<LaunchService>,
    /// The user's Java list and the discovery cache.
    pub java_settings: Arc<JavaService>,
    /// Desktop shortcuts for builds.
    pub shortcuts: Arc<ShortcutService>,
    /// Home's «Продовжити гру»: the builds played last and what on.
    pub recent: Arc<RecentService>,
    pub updater: Arc<UpdateService>,
    pub startup_warnings: Vec<Text>,
}

impl CoreApp {
    pub fn bootstrap(
        env: PathEnv,
        sink: Arc<dyn EventSink>,
        modules: Vec<Box<dyn Module>>,
        opts: BootstrapOptions,
    ) -> AppResult<CoreApp> {
        let initial = LauncherPaths::resolve(&env, None);
        let _ = std::fs::create_dir_all(&initial.app_state_dir);
        // The log dir does not depend on config.json, so logging starts first and captures
        // warnings about a damaged config.
        if opts.init_logging
            && let Err(e) = crate::logging::init_logging(&initial.log_dir, &default_log_dir(&env))
        {
            eprintln!("The launcher's log is unavailable: {e}");
        }
        let config = Arc::new(ConfigStore::open(initial.app_state_dir.join("config.json")));
        let override_dir = config.get_str(MINECRAFT_DIR_KEY);
        let paths = LauncherPaths::resolve(&env, override_dir.as_deref());
        tracing::info!("Starting the launcher {VERSION} (profile {PROFILE}, dev={})", paths.dev_mode);

        let mut startup_warnings = Vec::new();
        if paths.rejected_minecraft_override {
            tracing::warn!("Ignoring unsafe Minecraft directory override {override_dir:?}");
            let _ = config.delete(MINECRAFT_DIR_KEY);
        }
        if let Err(e) = paths.ensure_dirs() {
            tracing::error!("Unable to prepare launcher directories: {e}");
            startup_warnings.push(
                Text::key("minecraft_game_dir_unavailable")
                    .param("path", paths.minecraft_dir.to_string_lossy())
                    .param("error", e.to_string()),
            );
        }

        let feedback = FeedbackService::new(sink.clone());
        let game_sink = sink.clone();
        let modules = ModuleRegistry::new(modules)?;
        if let Err(e) = modules.apply_defaults(&config) {
            tracing::warn!("Unable to write module defaults: {e}");
        }
        let ctx = ModuleContext { paths: &paths, config: &config, feedback: &feedback };
        let _ = modules.init_all(&ctx);

        let auth = AuthService::new(
            AuthConfig::new(MS_CLIENT_ID, &paths.app_state_dir, &paths.cache_dir),
            feedback.clone(),
            sink.clone(),
            opts.opener.clone(),
        )?;
        if let Some(current) = Version::parse(VERSION) {
            update_stage::cleanup_after_update(&paths.cache_dir, &current);
        }
        // The one downloader every download of the app goes through.
        let downloader = Arc::new(Downloader::new(DownloaderConfig::default())?);
        let updater = UpdateService::new(
            UpdateConfig {
                api_base: UPDATE_API.to_string(),
                repo: UPDATE_REPO.to_string(),
                current_version: VERSION.to_string(),
                cache_dir: paths.cache_dir.clone(),
                dev_mode: paths.dev_mode,
                platform: Platform::current(),
                exec: ExecContext::current().ok(),
                env: env.clone(),
                updated_from: track_version(&config),
            },
            feedback.clone(),
            sink,
            downloader.clone(),
        );
        let versions = Arc::new(VersionStore::open(&paths.app_state_dir, &paths.minecraft_dir));
        let shared = Arc::new(Coordinator::shared());
        let platform = GamePlatform::current();
        let endpoints = MojangEndpoints::default();
        let meta = Arc::new(MetaClient::new(Duration::from_secs(30), META_TTL)?);
        let java = Arc::new(JavaRuntimes::new(
            &paths.minecraft_dir,
            platform.clone(),
            endpoints.clone(),
            meta.clone(),
            downloader.clone(),
        ));
        let minecraft = Arc::new(MinecraftInstaller::new(
            &paths.minecraft_dir,
            platform,
            endpoints,
            meta.clone(),
            downloader.clone(),
            java.clone(),
            shared.clone(),
        ));
        let components = Arc::new(ComponentInstaller::new(
            minecraft.clone(),
            meta.clone(),
            LoaderEndpoints::default(),
            &paths.minecraft_dir,
        ));
        let instances = Arc::new(Coordinator::instances());
        let builds = Arc::new(BuildService::new(
            versions.clone(),
            components.clone(),
            feedback.clone(),
            config.clone(),
            instances.clone(),
        ));
        let content = Arc::new(ContentService::new(versions.clone(), instances.clone(), feedback.clone()));
        let component_manager =
            Arc::new(ComponentManager::new(versions.clone(), components.clone(), feedback.clone()));
        let launcher = Arc::new(LaunchService::new(LaunchDeps {
            mc_dir: paths.minecraft_dir.clone(),
            platform: GamePlatform::current(),
            versions: versions.clone(),
            components: components.clone(),
            auth: auth.clone(),
            feedback: feedback.clone(),
            config: config.clone(),
            instances: instances.clone(),
            sink: game_sink,
            spawner: Arc::new(SystemSpawner),
            gpu: Arc::new(WindowsGpuPreferences),
            hooks: modules.launch_hooks(),
            downloader: downloader.clone(),
            watchers: modules.game_watchers(),
            timings: LaunchTimings::default(),
            ledger: Some(paths.app_state_dir.join(LEDGER_FILE)),
            memory: Arc::new(crate::java::memory::MemoryLimits::detect),
        }));
        // Games a launcher before this one started (it updated, restarted, crashed) are its own.
        let adopted = launcher.adopt();
        if adopted > 0 {
            tracing::info!("Games still running from before the restart: {adopted}");
        }
        let java_settings = Arc::new(JavaService::new(
            config.clone(),
            DiscoveryRoots::from_system(&paths.minecraft_dir, &paths.app_state_dir),
        ));
        let shortcuts = Arc::new(ShortcutService::from_system(&paths.app_state_dir));
        let recent = Arc::new(RecentService::new(versions.clone()));
        let settings = SettingsService::new(env.clone(), paths.clone(), config.clone(), opts.system_lang);
        Ok(CoreApp {
            env,
            paths,
            config,
            feedback,
            modules,
            settings,
            auth,
            versions,
            instances,
            shared,
            downloader,
            meta,
            java,
            minecraft,
            components,
            component_manager,
            builds,
            content,
            launcher,
            java_settings,
            shortcuts,
            recent,
            updater,
            startup_warnings,
        })
    }

    /// The launcher log: the file logging writes to, or where it would be in the log directory.
    pub fn log_file(&self) -> PathBuf {
        crate::logging::log_path().unwrap_or_else(|| self.paths.log_dir.join(crate::logging::LOG_FILE))
    }

    pub fn app_info(&self) -> AppInfo {
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        AppInfo {
            version: VERSION.to_string(),
            profile: PROFILE.to_string(),
            dev_mode: self.paths.dev_mode,
            os: Os::current().as_str().to_string(),
            support_url: (!SUPPORT_URL.is_empty()).then(|| SUPPORT_URL.to_string()),
            issues_url: (!ISSUES_URL.is_empty()).then(|| ISSUES_URL.to_string()),
            updates_configured: !UPDATE_REPO.is_empty(),
            update_source: (UPDATE_API != DEFAULT_UPDATE_API).then(|| UPDATE_API.to_string()),
            modules: self.modules.infos(),
            providers: self.modules.provider_infos(),
            paths: PathsInfo {
                app_state_dir: s(&self.paths.app_state_dir),
                minecraft_dir: s(&self.paths.minecraft_dir),
                cache_dir: s(&self.paths.cache_dir),
                log_dir: s(&self.paths.log_dir),
                log_file: s(&self.log_file()),
            },
        }
    }

    pub fn setup_state(&self) -> SetupState {
        setup::setup_state(&self.env, &self.paths, &self.config, &self.settings.lang())
    }

    pub fn setup_preview(&self, dir: &str) -> AppResult<SetupPreview> {
        setup::preview_current(&self.env, &self.paths, &self.config, dir)
    }

    pub fn apply_setup(&self, plan: &SetupPlan) -> AppResult<SetupOutcome> {
        setup::apply_setup(&self.env, &self.paths, &self.config, plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::NullSink;
    use crate::paths::Os;
    use launcher_shared::ErrorCode;
    use launcher_shared::branding::APP_NAME;
    use serde_json::{Value, json};

    use crate::modules::ModuleFuture;

    struct Echo;
    impl Module for Echo {
        fn id(&self) -> &'static str {
            "echo"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
            let dev = core.paths.dev_mode;
            (command == "echo")
                .then(|| Box::pin(async move { Ok(json!({"args": args, "dev": dev})) }) as ModuleFuture)
        }
    }

    #[tokio::test]
    async fn module_commands_reach_their_module() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app =
            Arc::new(CoreApp::bootstrap(env, Arc::new(NullSink), vec![Box::new(Echo)], opts()).unwrap());
        let answer = app.modules.call(&app, "echo", "echo", json!({"a": 1})).await.unwrap();
        assert_eq!(answer, json!({"args": {"a": 1}, "dev": true}));
        let unknown = app.modules.call(&app, "echo", "nope", Value::Null).await.unwrap_err();
        assert_eq!(
            (unknown.code, unknown.params.get("command").map(String::as_str)),
            (ErrorCode::InvalidInput, Some("nope"))
        );
        let missing = app.modules.call(&app, "ghost", "echo", Value::Null).await.unwrap_err();
        assert_eq!(
            (missing.code, missing.params.get("module").map(String::as_str)),
            (ErrorCode::NotFound, Some("ghost"))
        );
    }

    struct Flag;
    impl Module for Flag {
        fn id(&self) -> &'static str {
            "flag"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn config_defaults(&self) -> Vec<(&'static str, Value)> {
            vec![("flag_enabled", json!("yes"))]
        }
    }

    fn opts() -> BootstrapOptions {
        BootstrapOptions {
            init_logging: false,
            system_lang: "en_US".into(),
            opener: Arc::new(crate::auth::NoOpener),
        }
    }

    #[test]
    fn bootstrap_in_dev_mode_creates_dirs_and_applies_module_defaults() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app = CoreApp::bootstrap(env, Arc::new(NullSink), vec![Box::new(Flag)], opts()).unwrap();
        assert!(app.paths.games_dir.is_dir());
        assert_eq!(app.config.get_str("flag_enabled").as_deref(), Some("yes"));
        let info = app.app_info();
        assert_eq!(std::path::Path::new(&info.paths.log_file), repo.path().join(".dev").join("app.log"));
        assert!(info.dev_mode);
        assert_eq!(info.modules[0].id, "flag");
        assert_eq!(info.issues_url.as_deref(), Some(launcher_shared::branding::ISSUES_URL));
        assert!(app.setup_state().should_open);
        assert!(app.auth.snapshot().profiles.is_empty());
    }

    #[test]
    fn bootstrap_drops_protected_minecraft_override() {
        let home = tempfile::tempdir().unwrap();
        let exe_dir = home.path().join("Program Files").join(APP_NAME);
        let env = PathEnv {
            os: Os::Linux,
            home: home.path().to_path_buf(),
            exe_dir: Some(exe_dir.clone()),
            ..PathEnv::default()
        };
        let state = crate::paths::default_app_state_dir(&env);
        std::fs::create_dir_all(&state).unwrap();
        let cfg = ConfigStore::open(state.join("config.json"));
        cfg.set("minecraft_game_dir", json!(exe_dir.join("mc").to_string_lossy())).unwrap();
        let app = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert!(app.paths.minecraft_dir_is_default);
        assert_eq!(app.config.get("minecraft_game_dir"), None);
    }

    #[test]
    fn bootstrap_never_rewrites_a_corrupt_config() {
        let home = tempfile::tempdir().unwrap();
        let env = PathEnv { os: Os::Linux, home: home.path().to_path_buf(), ..PathEnv::default() };
        let state = crate::paths::default_app_state_dir(&env);
        std::fs::create_dir_all(&state).unwrap();
        let config = state.join("config.json");
        for garbage in [&b"{\"lang\":\"uk_UA\",\"compact_sidebar\":\"no\""[..], b"null"] {
            std::fs::write(&config, garbage).unwrap();
            let app =
                CoreApp::bootstrap(env.clone(), Arc::new(NullSink), vec![Box::new(Flag)], opts()).unwrap();
            assert_eq!(std::fs::read(&config).unwrap(), garbage);
            assert_eq!(app.config.get_str("flag_enabled").as_deref(), Some("yes"));
        }
    }

    #[test]
    fn bootstrap_reports_the_first_start_after_an_update() {
        let home = tempfile::tempdir().unwrap();
        let env = PathEnv { os: Os::Linux, home: home.path().to_path_buf(), ..PathEnv::default() };
        let state = crate::paths::default_app_state_dir(&env);
        std::fs::create_dir_all(&state).unwrap();
        ConfigStore::open(state.join("config.json")).set(LAST_RUN_VERSION_KEY, json!("0.0.0")).unwrap();
        let app = CoreApp::bootstrap(env.clone(), Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert_eq!(app.updater.status().updated_from.as_deref(), Some("0.0.0"));
        assert_eq!(app.config.get_str(LAST_RUN_VERSION_KEY).as_deref(), Some(VERSION));
        let again = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert_eq!(again.updater.status().updated_from, None);
    }

    #[test]
    fn bootstrap_opens_the_build_registry() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app = CoreApp::bootstrap(env.clone(), Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert!(app.versions.list().is_empty());
        app.versions.create(&mut crate::storage::versions::Build::new("Aeronautics")).unwrap();
        assert!(app.paths.games_dir.join("aeronautics").join("version.json").is_file());
        let lease = app.instances.try_acquire(&app.paths.games_dir.join("aeronautics"), "launch").unwrap();
        assert_eq!(lease.kind, "launch");
        let again = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert_eq!(again.versions.get("aeronautics").unwrap().name, "Aeronautics");
    }

    #[test]
    fn bootstrap_wires_the_game_services() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert!(app.java.executable("java-runtime-delta").is_none());
        let check = app.minecraft.check("1.21.1");
        assert!(!check.valid && !check.components.manifest);
        drop(app.shared.try_acquire(&app.paths.minecraft_dir, "java_runtime").unwrap());
    }

    #[test]
    fn bootstrap_wires_the_launcher() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert!(!app.launcher.is_running("aeronautics"));
        assert_eq!(app.launcher.terminate("aeronautics"), 0);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let request =
            crate::launch::service::LaunchRequest { build_key: "aeronautics".into(), ..Default::default() };
        let err = runtime.block_on(app.launcher.launch(request)).unwrap_err();
        assert_eq!(err.code, launcher_shared::ErrorCode::VersionNotFound);
    }

    #[test]
    fn bootstrap_wires_builds_java_and_shortcuts() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let app = CoreApp::bootstrap(env, Arc::new(NullSink), Vec::new(), opts()).unwrap();
        assert!(app.builds.snapshot(|_| false).builds.is_empty());
        assert_eq!(app.java_settings.list(), launcher_shared::JavaList::default());
        assert!(app.java_settings.needs_rescan(crate::java::preferences::now_secs()));
        assert_eq!(app.shortcuts.state_dir, app.paths.app_state_dir);
    }
}
