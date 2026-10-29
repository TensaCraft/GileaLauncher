//! The skins module's backend: the skin library and the skins and capes of official profiles.

pub mod library;
pub mod png;

use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::modules::{Module, ModuleFuture};
use launcher_shared::AppResult;
use serde::Serialize;
use serde_json::Value;

use crate::dto::Account;

pub struct SkinsModule;

const COMMANDS: [&str; 1] = ["accounts"];

impl Module for SkinsModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn call(&self, _core: &Arc<CoreApp>, command: &str, _args: Value) -> Option<ModuleFuture> {
        if !COMMANDS.contains(&command) {
            return None;
        }
        Some(Box::pin(async move { answer(Vec::<Account>::new()) }))
    }
}

fn answer<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| launcher_shared::AppError::internal(e.to_string()))
}

pub fn module() -> Box<dyn Module> {
    Box::new(SkinsModule)
}
