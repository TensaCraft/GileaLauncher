//! The backup before every launch: each world changed since its newest automatic
//! backup is archived, then the oldest automatic ones beyond the kept count go.

use chrono::Utc;
use launcher_core::feedback::OperationSpec;
use launcher_core::launch::alive::game_open;
use launcher_core::launch::hooks::{HookFuture, LaunchContext, LaunchHook};
use launcher_shared::{AppError, Text};

use super::restore::recover_all;
use super::settings;
use super::store::{Store, build_folder, worlds};
use crate::dto::Kind;

pub struct AutoBackup;

impl LaunchHook for AutoBackup {
    fn before_launch<'a>(&'a self, ctx: &'a LaunchContext<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            // A restore a crash cut short is finished before the game sees the worlds.
            let saves = ctx.game_dir.join("saves");
            let problems = tokio::task::spawn_blocking(move || recover_all(&saves))
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            if let Some(e) = problems.into_iter().next() {
                return Err(e);
            }
            let settings = settings::read(ctx.config, ctx.mc_dir);
            if !settings.enabled {
                return Ok(());
            }
            // Another copy of the build is playing: its worlds are mid-write, and a torn archive
            // would count as their newest backup.
            let game = ctx.game_dir.to_path_buf();
            let open = tokio::task::spawn_blocking(move || game_open(&game))
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            if open {
                return Ok(());
            }
            let game = ctx.game_dir.to_path_buf();
            let found = tokio::task::spawn_blocking(move || worlds(&game))
                .await
                .map_err(|e| AppError::internal(e.to_string()))?
                .map_err(|e| AppError::internal(e.to_string()))?;
            let folder = build_folder(ctx.build);
            for (world, _, modified) in found {
                let store = Store::new(settings.dir.clone());
                if !store.needs_auto(&folder, &world, modified) {
                    continue;
                }
                let op = ctx.feedback.begin(OperationSpec::new(
                    Text::key("world_backup_progress").param("world", &world),
                    "backup",
                ));
                let (build, game, keep, name) =
                    (ctx.build.clone(), ctx.game_dir.to_path_buf(), settings.keep, world.clone());
                let folder = folder.clone();
                let made = tokio::task::spawn_blocking(move || {
                    store.create(&build, &game, &name, Kind::Auto, Utc::now())?;
                    store.prune_auto(&folder, &name, keep)
                })
                .await
                .map_err(|e| AppError::internal(e.to_string()))
                .and_then(|r| r);
                match made {
                    Ok(_) => op.finish(),
                    Err(e) => {
                        tracing::warn!("The backup of world {world} before a launch failed: {}", e.detail);
                        op.fail(Text::key("world_backup_create_failed").param("world", &world));
                        ctx.feedback.warning(Text::key("world_backup_create_failed").param("world", &world));
                    }
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use launcher_core::feedback::{FeedbackService, NullSink};
    use launcher_core::launch::hooks::{LaunchContext, LaunchHook};
    use launcher_core::storage::config::ConfigStore;
    use launcher_core::storage::versions::Build;
    use serde_json::json;

    use super::*;
    use crate::backend::store::{Store, build_folder};

    fn world(game: &Path, name: &str) {
        let dir = game.join("saves").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("level.dat"), b"level").unwrap();
    }

    fn touch(path: &Path, later: Duration) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + later).unwrap();
    }

    async fn launch(game: &Path, mc: &Path, config: &ConfigStore, build: &Build) {
        let feedback = FeedbackService::new(Arc::new(NullSink));
        let ctx = LaunchContext { build, game_dir: game, mc_dir: mc, config, feedback: &feedback };
        AutoBackup.before_launch(&ctx).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changed_worlds_are_backed_up_before_a_launch_and_the_oldest_autos_go() {
        let tmp = tempfile::tempdir().unwrap();
        let (game, mc) = (tmp.path().join("game"), tmp.path().join("mc"));
        world(&game, "W");
        let config = ConfigStore::open(tmp.path().join("config.json"));
        config.set("world_backups_enabled", json!("yes")).unwrap();
        config.set("world_backups_keep_count", json!(1)).unwrap();
        let build = Build::new("Aero");
        let store = Store::new(mc.join("backups").join("worlds"));
        launch(&game, &mc, &config, &build).await;
        launch(&game, &mc, &config, &build).await;
        assert_eq!(
            store.list(&build_folder(&build), "W").len(),
            1,
            "an unchanged world is not archived again"
        );
        touch(&game.join("saves/W/level.dat"), Duration::from_secs(60));
        launch(&game, &mc, &config, &build).await;
        let left = store.list(&build_folder(&build), "W");
        assert_eq!(left.len(), 1, "the new one replaced the old: {left:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn worlds_a_running_copy_of_the_build_writes_are_not_archived() {
        let tmp = tempfile::tempdir().unwrap();
        let (game, mc) = (tmp.path().join("game"), tmp.path().join("mc"));
        world(&game, "W");
        let config = ConfigStore::open(tmp.path().join("config.json"));
        config.set("world_backups_enabled", json!("yes")).unwrap();
        let build = Build::new("Aero");
        // Another copy of the build plays world W: the game holds its session.lock.
        let lock = fs::File::create(game.join("saves/W/session.lock")).unwrap();
        lock.lock().unwrap();
        launch(&game, &mc, &config, &build).await;
        let store = Store::new(mc.join("backups").join("worlds"));
        assert!(store.list(&build_folder(&build), "W").is_empty(), "a world mid-write is no backup");
        drop(lock);
        launch(&game, &mc, &config, &build).await;
        assert_eq!(store.list(&build_folder(&build), "W").len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nothing_is_archived_while_backups_are_off_or_into_another_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (game, mc) = (tmp.path().join("game"), tmp.path().join("mc"));
        world(&game, "W");
        let config = ConfigStore::open(tmp.path().join("config.json"));
        let build = Build::new("Aero");
        launch(&game, &mc, &config, &build).await;
        assert!(!mc.join("backups").exists(), "off by default");
        let elsewhere = tmp.path().join("elsewhere");
        config.set("world_backups_enabled", json!("yes")).unwrap();
        config.set("world_backups_dir", json!(elsewhere.to_string_lossy())).unwrap();
        launch(&game, &mc, &config, &build).await;
        assert_eq!(Store::new(&elsewhere).list(&build_folder(&build), "W").len(), 1);
    }
}
