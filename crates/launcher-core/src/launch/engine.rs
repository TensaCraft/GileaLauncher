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

/// Where a game keeps what it switches on itself, in the build's game folder (`key:value` lines
/// as `options.txt`'s): its own mods (files of `ContentKind::Mods.endings(true)`, each an archive
/// with a TOML manifest of `id`, `name`, `version` and `description` at its root, kept as
/// `<id>.<ending>` and on while `mods_list` has a `mod.<id>:true` line) and its shader pack pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineContent {
    pub mods: &'static str,
    pub mods_list: &'static str,
    pub mod_manifest: &'static str,
    /// The file whose `<shader_pick>:<name>` line picks a pack of `shaderpacks/` (empty: none).
    pub graphics: &'static str,
    pub shader_pick: &'static str,
}

pub trait GameEngine: Send + Sync {
    /// The name a build's `engine` option gives.
    fn id(&self) -> &'static str;

    /// Brings what the build needs up to date and gives the command that starts its game.
    fn command<'a>(&'a self, ctx: &'a EngineContext<'a>) -> EngineFuture<'a>;

    /// Where its game keeps its own content, when the launcher may switch it.
    fn content(&self) -> Option<EngineContent> {
        None
    }
}

/// The engine build `options` name, if any.
pub fn engine_of(options: &Map<String, Value>) -> Option<&str> {
    options.get(ENGINE_OPTION).and_then(Value::as_str).filter(|id| !id.is_empty())
}
