//! The Horizon module's backend: the signed nightly build, preparing it from the player's own
//! Minecraft, and starting it as a build's game.

pub mod args;
pub mod create;
pub mod engine;
pub mod install;
pub mod release;

use std::sync::{Arc, OnceLock, Weak};

use launcher_core::core_app::CoreApp;
use launcher_core::launch::engine::GameEngine;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_shared::{AppError, ErrorCode};
use serde_json::{Value, json};

use crate::dto::CreateArgs;

pub struct HorizonModule {
    core: Arc<OnceLock<Weak<CoreApp>>>,
}

pub fn module() -> Box<dyn Module> {
    Box::new(HorizonModule { core: Arc::default() })
}

impl Module for HorizonModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn attach(&self, core: &Arc<CoreApp>) {
        // Weak: the core keeps this module, and through it the engine.
        let _ = self.core.set(Arc::downgrade(core));
    }

    fn game_engine(&self) -> Option<Arc<dyn GameEngine>> {
        Some(Arc::new(engine::HorizonEngine { core: self.core.clone() }))
    }

    fn changes_builds(&self, command: &str) -> bool {
        command == "create"
    }

    fn call(&self, core: &Arc<CoreApp>, command: &str, args: Value) -> Option<ModuleFuture> {
        if command != "create" {
            return None;
        }
        let core = core.clone();
        Some(Box::pin(async move {
            let args: CreateArgs = serde_json::from_value(args)
                .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
            let build = create::create(&core, &args)?;
            Ok(json!({"key": build.key, "name": build.name}))
        }))
    }
}
