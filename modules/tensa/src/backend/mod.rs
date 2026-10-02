//! The server builds module's backend: their API, the build a catalog entry
//! describes, and the plan that brings a build's folder in line with the server.

pub mod api;
pub mod home;
pub mod hook;
pub mod identity;
pub mod install;
pub mod manifest;
pub mod pack;
pub mod plan;
pub mod profile;
pub mod provider;
pub mod service;
pub mod sync;

use std::sync::{Arc, OnceLock};

use launcher_core::builds::service::default_gpu_mode;
use launcher_core::core_app::CoreApp;
use launcher_core::launch::hooks::LaunchHook;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_core::providers::ContentProvider;
use launcher_shared::provider::ProviderInfo;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};

use api::{API_BASE, TensaApi};
use service::{Deps, force_sync};

/// The module's command that syncs a build now (`{"key": <build key>}`).
pub const FORCE_SYNC: &str = "force_sync";
/// The server builds Home offers to install: `[{id, name, description, image, runs}]`.
pub const HOME_PACKS: &str = "home_packs";
/// Installs a server build from Home (`{"pack_id"}`) → `{key, name}`.
pub const INSTALL: &str = "install";
/// The module's settings: `{"show": bool}`; `set_settings` takes the same.
pub const SETTINGS: &str = "settings";
pub const SET_SETTINGS: &str = "set_settings";

fn text_arg(args: &Value, key: &str) -> AppResult<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, format!("the command needs `{key}`")))
}

/// The one API client of the module.
fn api() -> AppResult<Arc<TensaApi>> {
    static API: OnceLock<Arc<TensaApi>> = OnceLock::new();
    if let Some(api) = API.get() {
        return Ok(api.clone());
    }
    let api = Arc::new(TensaApi::new(API_BASE)?);
    Ok(API.get_or_init(|| api).clone())
}

/// The module's services from the launcher's.
fn deps_of(core: &Arc<CoreApp>) -> AppResult<Deps> {
    let (config, launcher) = (core.config.clone(), core.launcher.clone());
    Ok(Deps {
        api: api()?,
        versions: core.versions.clone(),
        instances: core.instances.clone(),
        feedback: core.feedback.clone(),
        downloader: core.downloader.clone(),
        components: core.components.clone(),
        gpu_mode: Arc::new(move || default_gpu_mode(&config)),
        running: Arc::new(move |key: &str| launcher.is_running(key)),
    })
}

pub struct TensaModule;

impl Module for TensaModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn config_defaults(&self) -> Vec<(&'static str, serde_json::Value)> {
        vec![("show_server_builds", serde_json::json!("yes"))]
    }

    fn provider_info(&self) -> Option<ProviderInfo> {
        Some(provider::info())
    }

    fn provider(&self, core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
        Ok(Arc::new(provider::ServerBuilds::new(Arc::new(deps_of(core)?))))
    }

    fn launch_hook(&self) -> Option<Arc<dyn LaunchHook>> {
        match api() {
            Ok(api) => Some(Arc::new(hook::ServerSync::new(api))),
            Err(e) => {
                tracing::warn!("Server builds will not sync before a launch: {}", e.detail);
                None
            }
        }
    }

    fn changes_builds(&self, command: &str) -> bool {
        [FORCE_SYNC, INSTALL, SET_SETTINGS].contains(&command)
    }

    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        let deps = deps_of(core);
        let config = core.config.clone();
        let io = |e: std::io::Error| AppError::new(ErrorCode::Io, e.to_string());
        let future: ModuleFuture = match command {
            FORCE_SYNC => Box::pin(async move {
                force_sync(&deps?, &text_arg(&args, "key")?).await?;
                Ok(Value::Null)
            }),
            HOME_PACKS => Box::pin(async move { Ok(Value::Array(home::home_packs(&deps?, &config).await?)) }),
            INSTALL => Box::pin(async move {
                let build = home::install_from_home(&deps?, &text_arg(&args, "pack_id")?).await?;
                Ok(json!({"key": build.key, "name": build.name}))
            }),
            SETTINGS => Box::pin(async move { Ok(json!({"show": home::shown(&config)})) }),
            SET_SETTINGS => Box::pin(async move {
                let show = args
                    .get("show")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, "set_settings needs `show`"))?;
                home::set_shown(&config, show).map_err(io)?;
                Ok(Value::Null)
            }),
            _ => return None,
        };
        Some(future)
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(TensaModule)
}
