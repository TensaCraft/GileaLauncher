//! The launch step that syncs a server build before its files are checked:
//! only builds the server manages; an unreachable server is skipped, any other failure stops the
//! launch.

use std::sync::Arc;

use launcher_core::launch::hooks::{HookFuture, LaunchContext, LaunchHook, PrepareContext};
use launcher_shared::Text;

use super::api::TensaApi;
use super::identity;
use super::sync::{SyncDeps, Synced, sync};

pub struct ServerSync {
    api: Arc<TensaApi>,
}

impl ServerSync {
    pub fn new(api: Arc<TensaApi>) -> ServerSync {
        ServerSync { api }
    }
}

impl LaunchHook for ServerSync {
    fn before_launch<'a>(&'a self, _: &'a LaunchContext<'a>) -> HookFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_build<'a>(&'a self, ctx: &'a PrepareContext<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            if !identity::is_managed(ctx.build, ctx.game_dir) {
                return Ok(());
            }
            let deps = SyncDeps {
                api: &self.api,
                versions: ctx.versions,
                components: ctx.components,
                downloader: ctx.downloader,
                running: ctx.running,
            };
            if sync(&deps, ctx.build, false, ctx.op).await? == Synced::Updated {
                ctx.feedback.success(Text::key("syncing_files_complete"));
            }
            Ok(())
        })
    }
}
