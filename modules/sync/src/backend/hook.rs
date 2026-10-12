//! Before Play: every group of the build syncs before the build's files are checked. A failure
//! is a warning only (its reason is on the group's card); the game still starts.

use std::sync::{Arc, OnceLock};

use launcher_core::launch::hooks::{HookFuture, LaunchContext, LaunchHook, PrepareContext};
use launcher_shared::Text;

use super::service::SyncService;

pub struct SyncHook(pub Arc<OnceLock<SyncService>>);

impl LaunchHook for SyncHook {
    fn before_launch<'a>(&'a self, _ctx: &'a LaunchContext<'a>) -> HookFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_build<'a>(&'a self, ctx: &'a PrepareContext<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            // Another copy of the build plays: its files must not change under it.
            if ctx.running || !self.0.get().is_some_and(|service| service.in_a_group(&ctx.build.key)) {
                return Ok(());
            }
            ctx.op.status(Text::key("sync_before_launch"));
            let (shared, key) = (self.0.clone(), ctx.build.key.clone());
            let results = tokio::task::spawn_blocking(move || {
                shared.get().map(|service| service.sync_build(&key, Some(&key))).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            for (group, result) in results {
                if result.is_err() {
                    ctx.feedback.warning(Text::key("sync_failed_before_launch").param("group", group));
                }
            }
            Ok(())
        })
    }
}
