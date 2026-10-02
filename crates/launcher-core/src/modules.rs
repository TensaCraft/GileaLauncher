//! Compile-time module system. Modules register through cargo features in `launcher-app`/`launcher-ui`.
//! A module may provide content (`ContentProvider`), steps before a launch
//! (`LaunchHook`), eyes on a started game (`GameWatcher`) and its own commands (`Module::call`).

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use launcher_shared::provider::{Need, ProviderInfo};
use launcher_shared::{AppError, AppResult, ErrorCode, ModuleInfo};
use serde_json::Value;

use crate::core_app::CoreApp;
use crate::feedback::FeedbackService;
use crate::launch::hooks::{GameWatcher, LaunchHook};
use crate::paths::LauncherPaths;
use crate::providers::{ContentProvider, require};
use crate::storage::config::ConfigStore;

/// A module command's answer.
pub type ModuleFuture = Pin<Box<dyn Future<Output = AppResult<Value>> + Send + 'static>>;

pub struct ModuleContext<'a> {
    pub paths: &'a LauncherPaths,
    pub config: &'a ConfigStore,
    pub feedback: &'a Arc<FeedbackService>,
}

pub trait Module: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;

    /// Config keys this module owns, with default values (written only when missing).
    fn config_defaults(&self) -> Vec<(&'static str, Value)> {
        Vec::new()
    }

    fn init(&self, _ctx: &ModuleContext<'_>) -> AppResult<()> {
        Ok(())
    }

    /// What this module offers as a content provider (`None`: it is none).
    fn provider_info(&self) -> Option<ProviderInfo> {
        None
    }

    /// Its content provider, made from the launcher's services (asked only when `provider_info`
    /// is `Some`).
    fn provider(&self, core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
        let _ = core;
        Err(AppError::new(ErrorCode::NotFound, format!("module {} is no content provider", self.id())))
    }

    /// Its step in every launch, if it has one.
    fn launch_hook(&self) -> Option<Arc<dyn LaunchHook>> {
        None
    }

    /// Its eye on every started game, if it has one.
    fn game_watcher(&self) -> Option<Arc<dyn GameWatcher>> {
        None
    }

    /// `command` creates or changes builds: the app announces the build list after it.
    fn changes_builds(&self, command: &str) -> bool {
        let _ = command;
        false
    }

    /// Runs `command` for the UI (`module_invoke`); `None` when the module has no such command.
    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        let _ = (core, command, args);
        None
    }
}

pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
}

impl ModuleRegistry {
    pub fn new(modules: Vec<Box<dyn Module>>) -> AppResult<Self> {
        let mut seen = HashSet::new();
        for m in &modules {
            if !seen.insert(m.id()) {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("duplicate module id {}", m.id()),
                ));
            }
        }
        Ok(Self { modules })
    }

    /// What each provider module offers, in registration order.
    pub fn provider_infos(&self) -> Vec<ProviderInfo> {
        self.modules.iter().filter_map(|m| m.provider_info()).collect()
    }

    /// Provider `id` when it offers `need`: `NotFound` without such a provider, `Unsupported` when
    /// it does not offer `need`.
    pub fn provider_info(&self, id: &str, need: Need) -> AppResult<ProviderInfo> {
        let info =
            self.modules.iter().find(|m| m.id() == id).and_then(|m| m.provider_info()).ok_or_else(|| {
                AppError::new(ErrorCode::NotFound, format!("no provider {id}")).with_param("provider", id)
            })?;
        require(&info, need)?;
        Ok(info)
    }

    /// Provider `id`, for `need` only (see `provider_info`).
    pub fn provider_for(
        &self,
        core: &Arc<CoreApp>,
        id: &str,
        need: Need,
    ) -> AppResult<Arc<dyn ContentProvider>> {
        self.provider_info(id, need)?;
        let module = self.modules.iter().find(|m| m.id() == id).expect("provider_info found it");
        module.provider(core)
    }

    /// The modules' steps in every launch, in registration order.
    pub fn launch_hooks(&self) -> Vec<Arc<dyn LaunchHook>> {
        self.modules.iter().filter_map(|m| m.launch_hook()).collect()
    }

    /// The modules' eyes on every started game, in registration order.
    pub fn game_watchers(&self) -> Vec<Arc<dyn GameWatcher>> {
        self.modules.iter().filter_map(|m| m.game_watcher()).collect()
    }

    /// Module `module`'s `command` creates or changes builds.
    pub fn changes_builds(&self, module: &str, command: &str) -> bool {
        self.modules.iter().any(|m| m.id() == module && m.changes_builds(command))
    }

    pub fn infos(&self) -> Vec<ModuleInfo> {
        self.modules.iter().map(|m| ModuleInfo { id: m.id().into(), version: m.version().into() }).collect()
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.modules.iter().any(|m| m.id() == id)
    }

    pub fn apply_defaults(&self, config: &ConfigStore) -> io::Result<()> {
        let missing: Vec<(String, Value)> = self
            .modules
            .iter()
            .flat_map(|m| m.config_defaults())
            .filter(|(key, _)| !config.contains(key))
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        // Never rewrites a corrupt config.json: defaults then live in memory only.
        if missing.is_empty() { Ok(()) } else { config.set_defaults(missing) }
    }

    /// Initialises every module; a failing module never prevents the others from starting.
    pub fn init_all(&self, ctx: &ModuleContext<'_>) -> Vec<(String, AppError)> {
        let mut failures = Vec::new();
        for m in &self.modules {
            if let Err(err) = m.init(ctx) {
                tracing::error!("Module {} failed to initialise: {err}", m.id());
                failures.push((m.id().to_string(), err));
            }
        }
        failures
    }

    /// Runs command `command` of module `module` (the UI's `module_invoke`).
    pub async fn call(
        &self,
        core: &Arc<CoreApp>,
        module: &str,
        command: &str,
        args: Value,
    ) -> AppResult<Value> {
        let Some(found) = self.modules.iter().find(|m| m.id() == module) else {
            return Err(AppError::new(ErrorCode::NotFound, format!("no module {module}"))
                .with_param("module", module));
        };
        match found.call(core, command, args) {
            Some(answer) => answer.await,
            None => Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("module {module} has no command {command}"),
            )
            .with_param("command", command)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::NullSink;
    use crate::paths::{Os, PathEnv};
    use serde_json::json;

    use crate::providers::{ContentProvider, ProviderFuture};
    use launcher_shared::ContentKind;
    use launcher_shared::provider::{Need, PacksArgs, ProviderInfo, SearchArgs, SearchPage};

    struct Dummy(&'static str);
    impl Module for Dummy {
        fn id(&self) -> &'static str {
            self.0
        }
        fn version(&self) -> &'static str {
            "1.2.3"
        }
        fn config_defaults(&self) -> Vec<(&'static str, Value)> {
            vec![("dummy_flag", json!("yes")), ("dummy_count", json!(3))]
        }
    }

    #[test]
    fn a_module_says_which_commands_change_builds() {
        let registry = ModuleRegistry::new(vec![Box::new(Dummy("a"))]).unwrap();
        assert!(!registry.changes_builds("a", "anything"), "by default nothing does");
        assert!(!registry.changes_builds("ghost", "pack_install"));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let err = ModuleRegistry::new(vec![Box::new(Dummy("a")), Box::new(Dummy("a"))]).err().unwrap();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn lists_infos_in_registration_order() {
        let reg = ModuleRegistry::new(vec![Box::new(Dummy("b")), Box::new(Dummy("a"))]).unwrap();
        assert_eq!(reg.ids(), vec!["b", "a"]);
        assert_eq!(reg.infos()[0], ModuleInfo { id: "b".into(), version: "1.2.3".into() });
        assert!(reg.contains("a") && !reg.contains("zzz"));
    }

    #[test]
    fn defaults_fill_only_missing_keys() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ConfigStore::open(dir.path().join("config.json"));
        cfg.set("dummy_flag", json!("no")).unwrap();
        let reg = ModuleRegistry::new(vec![Box::new(Dummy("a"))]).unwrap();
        reg.apply_defaults(&cfg).unwrap();
        assert_eq!(cfg.get("dummy_flag"), Some(json!("no")));
        assert_eq!(cfg.get("dummy_count"), Some(json!(3)));
    }

    struct Failing;
    impl Module for Failing {
        fn id(&self) -> &'static str {
            "failing"
        }
        fn version(&self) -> &'static str {
            "0.0.1"
        }
        fn init(&self, _ctx: &ModuleContext<'_>) -> AppResult<()> {
            Err(AppError::internal("boom"))
        }
    }

    #[test]
    fn init_failures_are_isolated() {
        let home = tempfile::tempdir().unwrap();
        let env = PathEnv { os: Os::Linux, home: home.path().to_path_buf(), ..PathEnv::default() };
        let paths = LauncherPaths::resolve(&env, None);
        let cfg = ConfigStore::open(home.path().join("config.json"));
        let feedback = FeedbackService::new(std::sync::Arc::new(NullSink));
        let reg = ModuleRegistry::new(vec![Box::new(Failing), Box::new(Dummy("ok"))]).unwrap();
        let ctx = ModuleContext { paths: &paths, config: &cfg, feedback: &feedback };
        let failures = reg.init_all(&ctx);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "failing");
    }

    /// A provider of mods only.
    struct Shop;
    impl ContentProvider for Shop {
        fn search(&self, args: SearchArgs) -> ProviderFuture<'_, SearchPage> {
            Box::pin(
                async move { Ok(SearchPage { hits: Vec::new(), total: 1, offset: args.offset, limit: 16 }) },
            )
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
            })
        }
        fn provider(&self, _core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
            Ok(Arc::new(Shop))
        }
    }

    #[test]
    fn a_provider_is_asked_only_for_what_it_offers() {
        let registry = ModuleRegistry::new(vec![Box::new(Dummy("plain")), Box::new(ShopModule)]).unwrap();
        let ids: Vec<String> = registry.provider_infos().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["shop"], "a module that is no provider is not listed");
        assert_eq!(registry.provider_info("shop", Need::Content(ContentKind::Mods)).unwrap().name, "Shop");
        let refused = |id, need| registry.provider_info(id, need).unwrap_err();
        let other_kind = refused("shop", Need::Content(ContentKind::ShaderPacks));
        assert_eq!(
            (other_kind.code, other_kind.params.get("provider").map(String::as_str)),
            (ErrorCode::Unsupported, Some("Shop"))
        );
        assert_eq!(refused("shop", Need::Updates(ContentKind::Mods)).code, ErrorCode::Unsupported);
        assert_eq!(refused("shop", Need::Modpacks).code, ErrorCode::Unsupported);
        assert_eq!(refused("plain", Need::Modpacks).code, ErrorCode::NotFound);
        assert_eq!(refused("ghost", Need::Content(ContentKind::Mods)).code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn what_a_provider_leaves_out_answers_unsupported() {
        let shop = Shop;
        let page = shop.search(SearchArgs {
            key: "aero".into(),
            kind: ContentKind::Mods,
            query: String::new(),
            offset: 16,
        });
        assert_eq!(page.await.unwrap().offset, 16);
        let packs = shop.modpacks(PacksArgs { query: String::new(), offset: 0 }).await.unwrap_err();
        assert_eq!(packs.code, ErrorCode::Unsupported);
        let updates = shop.modpack_builds().await.unwrap_err();
        assert_eq!(updates.code, ErrorCode::Unsupported);
    }
}
