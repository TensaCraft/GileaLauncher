//! What modules add to every launch: a step before the game starts (`LaunchHook`:
//! the backups module archives changed worlds) and an eye on the game while it runs
//! (`GameWatcher`: the crash diagnostics module reads its logs).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use launcher_shared::AppResult;

use crate::feedback::{EventSink, FeedbackService, OperationHandle};
use crate::loaders::ComponentSource;
use crate::net::downloader::Downloader;
use crate::storage::config::ConfigStore;
use crate::storage::versions::{Build, VersionStore};

/// What a hook sees of the launch it runs in.
pub struct LaunchContext<'a> {
    pub build: &'a Build,
    pub game_dir: &'a Path,
    pub mc_dir: &'a Path,
    pub config: &'a ConfigStore,
    /// For operations and warnings of its own.
    pub feedback: &'a Arc<FeedbackService>,
}

/// What a step that prepares the build sees, before the build's files are checked: the services to
/// bring the build up to date with, and the launch's own (hidden) operation for its status.
pub struct PrepareContext<'a> {
    pub build: &'a Build,
    pub game_dir: &'a Path,
    pub config: &'a ConfigStore,
    pub feedback: &'a Arc<FeedbackService>,
    pub versions: &'a Arc<VersionStore>,
    pub components: &'a dyn ComponentSource,
    pub downloader: &'a Arc<Downloader>,
    pub op: &'a OperationHandle,
    /// Another copy of the build's game is running (the player chose to start a second one): the
    /// build's files must not change under it.
    pub running: bool,
}

pub type HookFuture<'a> = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'a>>;

/// A module's step in every launch.
pub trait LaunchHook: Send + Sync {
    /// After the build's files are checked, before the game starts, under the launch's lease; an
    /// error is only warned about.
    fn before_launch<'a>(&'a self, ctx: &'a LaunchContext<'a>) -> HookFuture<'a>;

    /// Before the build's files are checked, under the launch's lease: the build may be brought up
    /// to date — its record saved, its component changed — and the launch reads it again. An
    /// error stops the launch.
    fn prepare_build<'a>(&'a self, ctx: &'a PrepareContext<'a>) -> HookFuture<'a> {
        let _ = ctx;
        Box::pin(async { Ok(()) })
    }
}

/// The game a watcher watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedGame {
    pub build_key: String,
    pub build_name: String,
    pub game_dir: PathBuf,
    pub launched_at: SystemTime,
}

/// A module's eye on every started game.
pub trait GameWatcher: Send + Sync {
    /// Its watch over one game that has just started.
    fn watch(&self, game: &WatchedGame) -> Box<dyn GameWatch>;
}

/// A watch over one game, asked from the thread that watches it.
pub trait GameWatch: Send {
    /// Asked at every look while the game runs: it cannot go on (the launcher stops it, and it
    /// is a crash).
    fn cannot_go_on(&mut self) -> bool {
        false
    }

    /// The game crashed: whether this watch told the user what happened (the plain alert then
    /// only goes to the activity log).
    fn crashed(&mut self, sink: &dyn EventSink) -> bool {
        let _ = sink;
        false
    }
}
