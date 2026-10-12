//! After a game closes: every group of its build syncs, in the background.

use std::sync::{Arc, OnceLock};

use launcher_core::launch::hooks::{GameWatch, GameWatcher, WatchedGame};

use super::service::SyncService;

pub struct SyncWatcher(pub Arc<OnceLock<SyncService>>);

impl GameWatcher for SyncWatcher {
    fn watch(&self, game: &WatchedGame) -> Box<dyn GameWatch> {
        Box::new(SyncWatch { shared: self.0.clone(), key: game.build_key.clone() })
    }
}

struct SyncWatch {
    shared: Arc<OnceLock<SyncService>>,
    key: String,
}

impl GameWatch for SyncWatch {
    fn exited(&mut self) {
        let (shared, key) = (self.shared.clone(), self.key.clone());
        let spawned = std::thread::Builder::new().name("sync-after-game".into()).spawn(move || {
            let Some(service) = shared.get() else { return };
            for (group, result) in service.sync_build(&key, None) {
                if let Err(e) = result {
                    tracing::warn!("Sync of {group} after {key} closed failed: {}", e.detail);
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("Unable to sync after {} closed: {e}", self.key);
        }
    }
}
