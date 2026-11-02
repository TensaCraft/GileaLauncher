//! A game a module brings that is not Minecraft's Java edition (a native rewrite): it installs
//! what a build needs and gives the command that starts it. The launcher still does the rest of a
//! launch as for any game: the lease, the account, its watch, its end.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use launcher_shared::AppResult;
use launcher_shared::recent::Join;
use serde_json::{Map, Value};

use super::process::GameCommand;
use crate::auth::service::LaunchIdentity;
use crate::feedback::OperationHandle;
use crate::storage::versions::Build;

/// The build option that names its engine (`"horizon"`); none: Minecraft's Java edition.
pub const ENGINE_OPTION: &str = "engine";

pub type EngineFuture<'a> = Pin<Box<dyn Future<Output = AppResult<GameCommand>> + Send + 'a>>;

/// What an engine sees of the launch it starts.
pub struct EngineContext<'a> {
    pub build: &'a Build,
    /// The build's folder, there already.
    pub game_dir: &'a Path,
    /// The account the game plays with.
    pub identity: &'a LaunchIdentity,
    /// A world or server to open at once.
    pub join: Option<&'a Join>,
    /// The launch's own (hidden) operation: an engine shows a long step as one of its own.
    pub op: &'a OperationHandle,
}

pub trait GameEngine: Send + Sync {
    /// The name a build's `engine` option gives.
    fn id(&self) -> &'static str;

    /// Brings what the build needs up to date and gives the command that starts its game.
    fn command<'a>(&'a self, ctx: &'a EngineContext<'a>) -> EngineFuture<'a>;
}

/// The engine build `options` name, if any.
pub fn engine_of(options: &Map<String, Value>) -> Option<&str> {
    options.get(ENGINE_OPTION).and_then(Value::as_str).filter(|id| !id.is_empty())
}
