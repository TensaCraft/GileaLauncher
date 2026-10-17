//! A build made by the build wizard: installed, given the wizard's settings, then each picked
//! project installed through its provider with its required dependencies.

use std::sync::Arc;

use launcher_shared::provider::{InstallAnswer, InstallArgs, Need};
use launcher_shared::{
    AppError, AppResult, BuildSettingsDto, BuildSettingsUpdate, ErrorCode, LoaderKind, PlannedBuild,
    PlannedPick, PlannedSettings, Text,
};

use crate::core_app::CoreApp;
use crate::storage::versions::Build;

/// What came of the picks: the titles installed and those that were not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanOutcome {
    pub done: Vec<String>,
    pub failed: Vec<String>,
}

/// The settings as saved, to send back with some fields changed.
fn update_from(settings: &BuildSettingsDto) -> BuildSettingsUpdate {
    BuildSettingsUpdate {
        name: settings.name.clone(),
        component: settings.component.clone(),
        java_path: settings.java_path.clone(),
        gpu_mode: settings.gpu_mode.clone(),
        max_ram_gb: settings.max_ram_gb,
        jvm_arguments: settings.jvm_arguments.clone(),
        server_host: settings.server_host.clone(),
        server_port: settings.server_port.map(|p| p.to_string()).unwrap_or_default(),
        image_path: None,
        remove_image: false,
        profile: settings.profile.clone(),
        description: None,
        fullscreen: settings.fullscreen,
    }
}

/// The wizard's settings over build `key`'s own; the icon after them, on its own, so a picture
/// that cannot be read takes nothing else with it.
fn apply_settings(core: &CoreApp, key: &str, settings: &PlannedSettings) -> AppResult<()> {
    let mut update = update_from(&core.builds.settings(key)?);
    if settings.max_ram_gb.is_some() {
        update.max_ram_gb = settings.max_ram_gb;
    }
    if let Some(gpu) = settings.gpu_mode.clone() {
        update.gpu_mode = gpu;
    }
    if settings.profile.is_some() {
        update.profile = settings.profile.clone();
    }
    if !settings.description.trim().is_empty() {
        update.description = Some(settings.description.clone());
    }
    core.builds.update_settings(key, update)?;
    if let Some(image) = settings.image_path.clone() {
        let icon =
            BuildSettingsUpdate { image_path: Some(image), ..update_from(&core.builds.settings(key)?) };
        core.builds.update_settings(key, icon)?;
    }
    Ok(())
}

/// One pick into build `key`: its plan, then its install with the plan's required changes.
async fn install_pick(core: &Arc<CoreApp>, key: &str, pick: &PlannedPick) -> AppResult<()> {
    let provider = core.modules.provider_for(core, &pick.provider, Need::Content(pick.kind))?;
    let args =
        InstallArgs::new(key, pick.kind, pick.project_id.clone(), pick.slug.clone(), pick.title.clone());
    let plan = provider.plan(args.clone()).await?;
    if !plan.can_install() {
        return Err(AppError::new(ErrorCode::Unsupported, format!("{} cannot be installed", pick.title)));
    }
    match provider.install(InstallArgs { approved: plan.changes(&[]), ..args }).await? {
        InstallAnswer::Installed(_) => Ok(()),
        InstallAnswer::Replanned(_) => {
            Err(AppError::new(ErrorCode::Unsupported, format!("the plan of {} changed", pick.title)))
        }
    }
}

/// Gives build `key` the wizard's settings, then installs each pick in order; a failure is
/// noted and the next goes.
pub async fn apply_plan(
    core: &Arc<CoreApp>,
    key: &str,
    settings: &PlannedSettings,
    picks: &[PlannedPick],
) -> PlanOutcome {
    if let Err(e) = apply_settings(core, key, settings) {
        tracing::warn!("The wizard's settings of {key} were not saved: {}", e.detail);
        let name = core.versions.get(key).map_or_else(|| key.to_string(), |b| b.name);
        core.feedback.warning(Text::key("build_planned_settings_failed").param("name", name));
    }
    let mut outcome = PlanOutcome::default();
    for pick in picks {
        match install_pick(core, key, pick).await {
            Ok(()) => outcome.done.push(pick.title.clone()),
            Err(e) => {
                tracing::warn!(
                    "{} ({}) was not installed into {key}: {}",
                    pick.title,
                    pick.provider,
                    e.detail
                );
                outcome.failed.push(pick.title.clone());
            }
        }
    }
    outcome
}

/// Installs the build `args` describes (`created` hears of it at once), then its plan; a note of
/// the picks installed (none when nothing was) and one of those that were not.
pub async fn create_planned(
    core: &Arc<CoreApp>,
    args: PlannedBuild,
    created: &(dyn Fn(&Build) + Send + Sync),
) -> AppResult<Build> {
    let build = match args.loader {
        LoaderKind::Minecraft => core.builds.install_vanilla(&args.name, &args.mc).await?,
        kind => {
            let version =
                args.loader_version.as_deref().map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| {
                    AppError::new(ErrorCode::InvalidInput, "a loader build needs its loader version")
                })?;
            core.builds.install_loader(&args.name, kind, &args.mc, version).await?
        }
    };
    created(&build);
    let outcome = apply_plan(core, &build.key, &args.settings, &args.picks).await;
    if !outcome.done.is_empty() {
        let note = Text::key("build_planned_done")
            .param("name", &build.name)
            .param("done", outcome.done.len().to_string())
            .param("total", args.picks.len().to_string());
        core.feedback.success(note);
    }
    if !outcome.failed.is_empty() {
        let note = Text::key("build_planned_failed")
            .param("name", &build.name)
            .param("titles", outcome.failed.join(", "));
        core.feedback.warning(note);
    }
    Ok(core.versions.get(&build.key).unwrap_or(build))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use launcher_shared::provider::{
        Action, Change, InstallAnswer, InstallArgs, InstallOutcome, PlanDto, PlanIssue, PlanItem,
        ProviderInfo,
    };
    use launcher_shared::{ActivityEntry, Alert, ContentKind, OpsSnapshot, Toast};

    use super::*;
    use crate::core_app::{BootstrapOptions, CoreApp};
    use crate::feedback::EventSink;
    use crate::modules::Module;
    use crate::paths::{Os, PathEnv};
    use crate::providers::{ContentProvider, ProviderFuture};
    use crate::storage::versions::Build;

    /// What the test provider was asked to install, with what was approved.
    #[derive(Default)]
    struct Calls(Mutex<Vec<(String, Vec<Change>)>>);

    fn item(project: &str, action: Action) -> PlanItem {
        PlanItem {
            project_id: project.into(),
            version_id: format!("{project}-v1"),
            title: project.into(),
            version_number: "1".into(),
            filename: format!("{project}.jar"),
            action,
            current: None,
            url: None,
            unrecognized: false,
        }
    }

    struct Shop(Arc<Calls>);

    impl ContentProvider for Shop {
        fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
            Box::pin(async move {
                let mut plan =
                    PlanDto { main: Some(item(&args.project_id, Action::Install)), ..PlanDto::default() };
                if args.project_id == "good-1" {
                    plan.install.push(item("fabric-api", Action::Install));
                }
                if args.project_id == "bad" {
                    plan.blocking.push(PlanIssue {
                        code: "no_version".into(),
                        name: Some("Bad".into()),
                        file_name: None,
                        url: None,
                        blocking: true,
                        held: None,
                    });
                }
                Ok(plan)
            })
        }

        fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
            Box::pin(async move {
                self.0.0.lock().unwrap().push((args.project_id.clone(), args.approved.clone()));
                Ok(InstallAnswer::Installed(InstallOutcome {
                    project_id: args.project_id.clone(),
                    filename: format!("{}.jar", args.project_id),
                    version_number: "1".into(),
                }))
            })
        }
    }

    struct ShopModule(Arc<Calls>);

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
            Ok(Arc::new(Shop(self.0.clone())))
        }
    }

    /// The alerts the launcher raised.
    #[derive(Default)]
    struct Heard(Mutex<Vec<Alert>>);

    impl EventSink for Heard {
        fn ops(&self, _: &OpsSnapshot) {}
        fn activity(&self, _: &ActivityEntry) {}
        fn toast(&self, _: &Toast) {}
        fn alert(&self, alert: &Alert) {
            self.0.lock().unwrap().push(alert.clone());
        }
    }

    struct World {
        _home: tempfile::TempDir,
        _repo: tempfile::TempDir,
        core: Arc<CoreApp>,
        calls: Arc<Calls>,
        heard: Arc<Heard>,
        key: String,
    }

    fn world() -> World {
        let (home, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let env = PathEnv {
            os: Os::Windows,
            home: home.path().to_path_buf(),
            dev_root: Some(repo.path().to_path_buf()),
            ..PathEnv::default()
        };
        let calls = Arc::new(Calls::default());
        let opts = BootstrapOptions {
            init_logging: false,
            system_lang: "en_US".into(),
            opener: Arc::new(crate::auth::NoOpener),
        };
        let module: Box<dyn Module> = Box::new(ShopModule(calls.clone()));
        let heard = Arc::new(Heard::default());
        let core = Arc::new(CoreApp::bootstrap(env, heard.clone(), vec![module], opts).unwrap());
        let mut build = Build::new("Моя збірка");
        build.version = Some("1.21.1".into());
        build.client = Some("Fabric".into());
        core.versions.create(&mut build).unwrap();
        World { _home: home, _repo: repo, core, calls, heard, key: build.key }
    }

    /// The parameter `name` of the alert `key` the launcher raised.
    fn alert_param(heard: &Heard, key: &str, name: &str) -> Option<String> {
        heard.0.lock().unwrap().iter().find_map(|alert| match &alert.message {
            Text::Key { key: k, params } if k == key => params.get(name).cloned(),
            _ => None,
        })
    }

    fn pick(provider: &str, project: &str, title: &str) -> PlannedPick {
        PlannedPick {
            provider: provider.into(),
            kind: ContentKind::Mods,
            project_id: project.into(),
            slug: project.into(),
            title: title.into(),
            icon_url: None,
        }
    }

    #[tokio::test]
    async fn a_plan_installs_its_picks_in_order_past_a_failure() {
        let w = world();
        let picks =
            [pick("shop", "good-1", "Good 1"), pick("shop", "bad", "Bad"), pick("shop", "good-2", "Good 2")];
        let outcome = apply_plan(&w.core, &w.key, &PlannedSettings::default(), &picks).await;
        let installed: Vec<String> = w.calls.0.lock().unwrap().iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(installed, ["good-1", "good-2"]);
        assert_eq!(
            (outcome.done, outcome.failed),
            (vec!["Good 1".to_string(), "Good 2".into()], vec!["Bad".to_string()])
        );
    }

    #[tokio::test]
    async fn each_install_approves_the_plans_required_changes() {
        let w = world();
        apply_plan(&w.core, &w.key, &PlannedSettings::default(), &[pick("shop", "good-1", "Good 1")]).await;
        let approved = w.calls.0.lock().unwrap()[0].1.clone();
        let projects: Vec<&str> = approved.iter().map(|c| c.project_id.as_str()).collect();
        assert_eq!(projects, ["fabric-api", "good-1"], "the dependency, then the project");
    }

    #[tokio::test]
    async fn the_wizards_settings_are_applied_and_the_rest_kept() {
        let w = world();
        let before = w.core.builds.settings(&w.key).unwrap();
        let settings = PlannedSettings {
            max_ram_gb: Some(6),
            description: "Для друзів".into(),
            ..PlannedSettings::default()
        };
        apply_plan(&w.core, &w.key, &settings, &[]).await;
        let after = w.core.builds.settings(&w.key).unwrap();
        assert_eq!((after.max_ram_gb, after.description.as_str()), (Some(6), "Для друзів"));
        assert_eq!((after.gpu_mode, after.name), (before.gpu_mode, before.name), "the rest kept");
    }

    #[tokio::test]
    async fn an_icon_that_cannot_be_read_leaves_the_other_settings_saved() {
        let w = world();
        let settings = PlannedSettings {
            max_ram_gb: Some(6),
            image_path: Some(w._home.path().join("missing.png").to_string_lossy().into_owned()),
            description: "Для друзів".into(),
            ..PlannedSettings::default()
        };
        apply_plan(&w.core, &w.key, &settings, &[]).await;
        let after = w.core.builds.settings(&w.key).unwrap();
        assert_eq!((after.max_ram_gb, after.description.as_str()), (Some(6), "Для друзів"));
    }

    #[tokio::test]
    async fn settings_that_were_not_saved_are_told_by_the_builds_name() {
        let w = world();
        let settings = PlannedSettings {
            image_path: Some(w._home.path().join("missing.png").to_string_lossy().into_owned()),
            ..PlannedSettings::default()
        };
        apply_plan(&w.core, &w.key, &settings, &[]).await;
        assert_ne!(w.key, "Моя збірка");
        assert_eq!(
            alert_param(&w.heard, "build_planned_settings_failed", "name").as_deref(),
            Some("Моя збірка")
        );
    }

    #[tokio::test]
    async fn an_unknown_provider_is_a_failed_pick_not_an_error() {
        let w = world();
        let outcome =
            apply_plan(&w.core, &w.key, &PlannedSettings::default(), &[pick("ghost", "x", "Ghost mod")])
                .await;
        assert_eq!((outcome.done.len(), outcome.failed), (0, vec!["Ghost mod".to_string()]));
    }
}
