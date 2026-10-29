//! The skins module's backend: the skin library and the skins and capes of official profiles.

pub mod library;
pub mod mojang;
pub mod service;

use std::sync::{Arc, OnceLock};

use launcher_core::core_app::CoreApp;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_core::net::downloader::Origin;
use launcher_core::skins::service::TEXTURES_HOST;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dto::{ApplyArgs, CapeArgs, IdArgs, ImportArgs, ProfileArgs, RenameArgs, VariantArgs};
use mojang::Endpoints;
use service::{Deps, SkinsService, TokenFuture, session_of};

pub struct SkinsModule {
    /// The service, made once the core is up (`attach`).
    shared: Arc<OnceLock<Arc<SkinsService>>>,
}

const COMMANDS: [&str; 10] = [
    "accounts",
    "state",
    "library",
    "skin",
    "import",
    "rename",
    "set_variant",
    "delete",
    "apply",
    "set_cape",
];

impl Module for SkinsModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn attach(&self, core: &Arc<CoreApp>) {
        let (auth, profiles) = (core.auth.clone(), core.auth.clone());
        let deps = Deps {
            endpoints: Endpoints::default(),
            tokens: Arc::new(move |key: &str, force: bool| -> TokenFuture {
                let (auth, key) = (auth.clone(), key.to_string());
                Box::pin(async move { session_of(auth.refresh(&key, force).await?) })
            }),
            profiles: Arc::new(move || profiles.snapshot().profiles),
            skins: core.skins.clone(),
            downloader: core.downloader.clone(),
            textures: Origin::new(|url| url.scheme() == "https" && url.host_str() == Some(TEXTURES_HOST)),
            feedback: core.feedback.clone(),
            state_dir: core.paths.app_state_dir.clone(),
            cache_dir: core.paths.cache_dir.clone(),
        };
        match SkinsService::new(deps) {
            Ok(service) => {
                let _ = self.shared.set(Arc::new(service));
            }
            Err(e) => tracing::error!("The skins module cannot start: {}", e.detail),
        }
    }

    fn call(&self, _core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        if !COMMANDS.contains(&command) {
            return None;
        }
        let service = self.shared.get().cloned();
        let command = command.to_string();
        Some(Box::pin(async move {
            let service = service.ok_or_else(|| AppError::internal("the skins module is not attached"))?;
            dispatch(&service, &command, args).await
        }))
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> AppResult<T> {
    serde_json::from_value(args).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

fn answer<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

async fn dispatch(service: &SkinsService, command: &str, args: Value) -> AppResult<Value> {
    match command {
        "accounts" => answer(service.accounts()),
        "state" => answer(service.state(&parse::<ProfileArgs>(args)?.profile).await?),
        "library" => answer(service.library()),
        "skin" => answer(service.skin(&parse::<IdArgs>(args)?.id)?),
        "import" => answer(service.import(&parse::<ImportArgs>(args)?).await?),
        "rename" => {
            let a: RenameArgs = parse(args)?;
            answer(service.rename(&a.id, &a.name)?)
        }
        "set_variant" => {
            let a: VariantArgs = parse(args)?;
            answer(service.set_variant(&a.id, a.variant)?)
        }
        "delete" => answer(service.delete(&parse::<IdArgs>(args)?.id)?),
        "apply" => {
            let a: ApplyArgs = parse(args)?;
            answer(service.apply(&a.profile, &a.id).await?)
        }
        "set_cape" => {
            let a: CapeArgs = parse(args)?;
            answer(service.set_cape(&a.profile, a.cape).await?)
        }
        other => Err(AppError::new(ErrorCode::NotFound, format!("no command {other}"))),
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(SkinsModule { shared: Arc::new(OnceLock::new()) })
}
