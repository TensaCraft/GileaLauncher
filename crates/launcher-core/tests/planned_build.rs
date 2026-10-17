mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use launcher_core::auth::NoOpener;
use launcher_core::builds::planned::create_planned;
use launcher_core::builds::service::BuildService;
use launcher_core::core_app::{BootstrapOptions, CoreApp};
use launcher_core::feedback::EventSink;
use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::loaders::ComponentInstaller;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::modules::Module;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::paths::{Os, PathEnv};
use launcher_core::providers::{ContentProvider, ProviderFuture};
use launcher_core::storage::versions::Build;
use launcher_shared::provider::{
    Action, InstallAnswer, InstallArgs, InstallOutcome, PlanDto, PlanItem, ProviderInfo,
};
use launcher_shared::{
    ActivityEntry, Alert, AppResult, ContentKind, ErrorCode, LoaderKind, OpsSnapshot, PlannedBuild,
    PlannedPick, PlannedSettings, Text, Toast,
};
use support::fake_mojang::FakeMojang;

/// The toasts, alerts and activity the launcher showed.
#[derive(Default)]
struct Heard {
    toasts: Mutex<Vec<Toast>>,
    alerts: Mutex<Vec<Alert>>,
    activity: Mutex<Vec<ActivityEntry>>,
}

impl Heard {
    fn told(&self) -> usize {
        let lists = (self.toasts.lock().unwrap(), self.alerts.lock().unwrap(), self.activity.lock().unwrap());
        lists.0.len() + lists.1.len() + lists.2.len()
    }
}

impl EventSink for Heard {
    fn ops(&self, _: &OpsSnapshot) {}
    fn activity(&self, entry: &ActivityEntry) {
        self.activity.lock().unwrap().push(entry.clone());
    }
    fn toast(&self, toast: &Toast) {
        self.toasts.lock().unwrap().push(toast.clone());
    }
    fn alert(&self, alert: &Alert) {
        self.alerts.lock().unwrap().push(alert.clone());
    }
}

/// The parameters of the note `key`, wherever it was shown.
fn note(heard: &Heard, key: &str) -> Option<Vec<(String, String)>> {
    let params = |text: &Text| match text {
        Text::Key { key: k, params } if k == key => Some(params.clone().into_iter().collect()),
        _ => None,
    };
    let toasts = heard.toasts.lock().unwrap();
    let alerts = heard.alerts.lock().unwrap();
    toasts.iter().find_map(|t| params(&t.title)).or_else(|| alerts.iter().find_map(|a| params(&a.message)))
}

fn plan_item(project: &str) -> PlanItem {
    PlanItem {
        project_id: project.into(),
        version_id: format!("{project}-v1"),
        title: project.into(),
        version_number: "1".into(),
        filename: format!("{project}.jar"),
        action: Action::Install,
        current: None,
        url: None,
        unrecognized: false,
    }
}

/// A provider that installs whatever it is asked to.
struct Shop;

impl ContentProvider for Shop {
    fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
        Box::pin(async move { Ok(PlanDto { main: Some(plan_item(&args.project_id)), ..PlanDto::default() }) })
    }

    fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
        Box::pin(async move {
            Ok(InstallAnswer::Installed(InstallOutcome {
                project_id: args.project_id.clone(),
                filename: format!("{}.jar", args.project_id),
                version_number: "1".into(),
            }))
        })
    }
}

struct ShopModule;

impl Module for ShopModule {
    fn id(&self) -> &'static str {
        "shop"
    }
    fn version(&self) -> &'static str {
        "1.0.0"
    }
    fn provider_info(&self) -> Option<ProviderInfo> {
        Some(ProviderInfo {
            id: "shop".into(),
            name: "Shop".into(),
            icon: "search".into(),
            content: vec![ContentKind::Mods],
            updates: Vec::new(),
            modpacks: false,
            modpack_updates: false,
            content_clients: Vec::new(),
            content_label: None,
        })
    }
    fn provider(&self, _core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
        Ok(Arc::new(Shop))
    }
}

struct World {
    _home: tempfile::TempDir,
    _repo: tempfile::TempDir,
    core: Arc<CoreApp>,
    heard: Arc<Heard>,
}

/// The launcher with its builds installed from `fake`.
fn world(fake: &FakeMojang) -> World {
    let (home, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let env = PathEnv {
        os: Os::Windows,
        home: home.path().to_path_buf(),
        dev_root: Some(repo.path().to_path_buf()),
        ..PathEnv::default()
    };
    let heard = Arc::new(Heard::default());
    let opts =
        BootstrapOptions { init_logging: false, system_lang: "en_US".into(), opener: Arc::new(NoOpener) };
    let modules: Vec<Box<dyn Module>> = vec![Box::new(ShopModule)];
    let mut core = CoreApp::bootstrap(env, heard.clone(), modules, opts).unwrap();
    let mc = core.paths.minecraft_dir.clone();
    let downloader = Arc::new(
        Downloader::new(DownloaderConfig {
            retry_delay: Duration::from_millis(1),
            timeout: Duration::from_secs(5),
            ..DownloaderConfig::default()
        })
        .unwrap(),
    );
    let platform = GamePlatform::current();
    let java = Arc::new(JavaRuntimes::new(
        &mc,
        platform.clone(),
        fake.endpoints.clone(),
        core.meta.clone(),
        downloader.clone(),
    ));
    let installer = Arc::new(MinecraftInstaller::new(
        &mc,
        platform,
        fake.endpoints.clone(),
        core.meta.clone(),
        downloader,
        java,
        core.shared.clone(),
    ));
    let components =
        Arc::new(ComponentInstaller::new(installer, core.meta.clone(), fake.loader_endpoints(), &mc));
    core.builds = Arc::new(BuildService::new(
        core.versions.clone(),
        components,
        core.feedback.clone(),
        core.config.clone(),
        core.instances.clone(),
    ));
    World { _home: home, _repo: repo, core: Arc::new(core), heard }
}

fn pick(provider: &str, project: &str, title: &str) -> PlannedPick {
    PlannedPick {
        provider: provider.into(),
        kind: ContentKind::Mods,
        project_id: project.into(),
        slug: project.into(),
        title: title.into(),
    }
}

fn vanilla(picks: Vec<PlannedPick>) -> PlannedBuild {
    PlannedBuild {
        name: "Друзі".into(),
        mc: "1.21.1".into(),
        loader: LoaderKind::Minecraft,
        loader_version: None,
        settings: PlannedSettings::default(),
        picks,
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_note_counts_the_picks_installed_out_of_all_and_names_the_rest() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let picks = vec![pick("shop", "sodium", "Sodium"), pick("ghost", "x", "Ghost mod")];
    let build = create_planned(&w.core, vanilla(picks), &|_: &Build| {}).await.unwrap();
    assert_eq!(build.name, "Друзі");
    assert_eq!(
        note(&w.heard, "build_planned_done"),
        Some(pairs(&[("done", "1"), ("name", "Друзі"), ("total", "2")]))
    );
    assert_eq!(
        note(&w.heard, "build_planned_failed"),
        Some(pairs(&[("name", "Друзі"), ("titles", "Ghost mod")]))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_installed_is_not_told_as_a_success() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let picks = vec![pick("ghost", "x", "Ghost mod"), pick("ghost", "y", "Other mod")];
    create_planned(&w.core, vanilla(picks), &|_: &Build| {}).await.unwrap();
    assert_eq!(note(&w.heard, "build_planned_done"), None, "no «0 of 2 installed»");
    assert_eq!(
        note(&w.heard, "build_planned_failed"),
        Some(pairs(&[("name", "Друзі"), ("titles", "Ghost mod, Other mod")]))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loader_build_without_its_loader_version_is_refused_before_anything_starts() {
    let fake = FakeMojang::start().await;
    fake.add_vanilla("1.21.1");
    let w = world(&fake);
    let before = w.heard.told();
    for loader_version in [None, Some("  ".to_string())] {
        let args = PlannedBuild { loader: LoaderKind::Fabric, loader_version, ..vanilla(Vec::new()) };
        let created = Mutex::new(false);
        let err =
            create_planned(&w.core, args, &|_: &Build| *created.lock().unwrap() = true).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(!*created.lock().unwrap(), "no build is announced");
    }
    assert_eq!(w.heard.told(), before, "no install was begun, so none failed");
    assert_eq!(fake.server.total_requests(), 0);
    assert!(w.core.versions.list().is_empty());
}
