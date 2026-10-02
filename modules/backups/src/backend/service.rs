//! What the module's commands do: a build's worlds and their
//! backups, backing up, restoring and deleting — never while the build's game runs — and the
//! module's settings.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use launcher_core::feedback::{FeedbackService, OperationSpec};
use launcher_core::launch::alive::game_open;
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::paths::probe_writable;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{AppError, AppResult, ErrorCode, Text};
use serde_json::json;

use super::restore::{recover_all, restore};
use super::settings::{self, DIR, ENABLED, KEEP};
use super::store::{Store, build_folder, worlds};
use crate::dto::{BackupDto, BackupSettings, Kind, WorldDto};

pub struct Deps {
    pub config: Arc<ConfigStore>,
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    /// Whether build `key`'s game runs.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

pub struct BackupsService {
    deps: Deps,
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

fn io_error(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

impl BackupsService {
    pub fn new(deps: Deps) -> BackupsService {
        BackupsService { deps }
    }

    pub fn settings(&self) -> BackupSettings {
        let s = settings::read(&self.deps.config, self.deps.versions.minecraft_dir());
        BackupSettings {
            enabled: s.enabled,
            keep: s.keep,
            dir: s.dir.to_string_lossy().into_owned(),
            default_dir: s.default_dir.to_string_lossy().into_owned(),
        }
    }

    /// Stores `wanted`: at least one automatic backup a world; the default folder is not stored,
    /// another one must be writable.
    pub fn set_settings(&self, wanted: BackupSettings) -> AppResult<BackupSettings> {
        if wanted.keep < 1 {
            return Err(AppError::new(ErrorCode::InvalidInput, "keep at least one backup")
                .with_param("key", "world_backups_keep_count_invalid"));
        }
        let config = &self.deps.config;
        let default_dir = settings::default_dir(self.deps.versions.minecraft_dir());
        let dir = PathBuf::from(wanted.dir.trim());
        if wanted.dir.trim().is_empty() || dir == default_dir {
            config.delete(DIR).map_err(io_error)?;
        } else {
            if !dir.is_absolute() {
                return Err(AppError::new(ErrorCode::InvalidDirectoryPath, "the folder must be absolute")
                    .with_param("path", &wanted.dir));
            }
            probe_writable(&dir).map_err(|e| {
                AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string()).with_param("path", &wanted.dir)
            })?;
            config.set(DIR, json!(dir.to_string_lossy())).map_err(io_error)?;
        }
        config.set(ENABLED, json!(if wanted.enabled { "yes" } else { "no" })).map_err(io_error)?;
        config.set(KEEP, json!(wanted.keep)).map_err(io_error)?;
        Ok(self.settings())
    }

    fn build(&self, key: &str) -> AppResult<Build> {
        self.deps.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })
    }

    fn store(&self) -> Store {
        Store::new(settings::read(&self.deps.config, self.deps.versions.minecraft_dir()).dir)
    }

    fn game(&self, build: &Build) -> PathBuf {
        game_dir(build, self.deps.versions.minecraft_dir())
    }

    /// The build's game must not run while its worlds change hands (nor a game the launcher does
    /// not know with the build's folder open: `game_open`).
    fn idle(&self, build: &Build) -> AppResult<()> {
        if (self.deps.running)(&build.key) || game_open(&self.game(build)) {
            return Err(AppError::new(ErrorCode::GameRunning, format!("{} is running", build.name))
                .with_param("version", &build.name));
        }
        Ok(())
    }

    pub async fn worlds(&self, key: &str) -> AppResult<Vec<WorldDto>> {
        let build = self.build(key)?;
        let (game, store, folder) = (self.game(&build), self.store(), build_folder(&build));
        blocking(move || {
            // A restore a crash cut short is finished first, so the world shows again.
            for e in recover_all(&game.join("saves")) {
                tracing::warn!("An interrupted restore could not be finished: {}", e.detail);
            }
            let found = worlds(&game).map_err(io_error)?;
            Ok(found
                .into_iter()
                .map(|(name, size, modified)| {
                    let modified =
                        modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                    let backups = store.list(&folder, &name).len();
                    let backups_dir = store.world_dir(&folder, &name).to_string_lossy().into_owned();
                    WorldDto { folder: name, size, modified, backups, backups_dir }
                })
                .collect())
        })
        .await
    }

    pub async fn backups(&self, key: &str, world: &str) -> AppResult<Vec<BackupDto>> {
        let build = self.build(key)?;
        let (store, folder, world) = (self.store(), build_folder(&build), world.to_string());
        blocking(move || Ok(store.list(&folder, &world))).await
    }

    pub async fn create(&self, key: &str, world: &str) -> AppResult<BackupDto> {
        let build = self.build(key)?;
        self.idle(&build)?;
        let game = self.game(&build);
        if !game.join("saves").join(world).join("level.dat").is_file() {
            return Err(
                AppError::new(ErrorCode::NotFound, format!("no world {world}")).with_param("world", world)
            );
        }
        let lease = self.deps.instances.try_acquire(&game, "world_backup")?;
        let op = self
            .deps
            .feedback
            .begin(OperationSpec::new(Text::key("world_backup_creating").param("world", world), "backup"));
        let (store, name) = (self.store(), world.to_string());
        let made = blocking(move || store.create(&build, &game, &name, Kind::Manual, Utc::now())).await;
        drop(lease);
        match made {
            Ok(backup) => {
                op.finish();
                self.deps.feedback.success(Text::key("world_backup_created").param("world", world));
                Ok(backup)
            }
            Err(e) => {
                op.fail(Text::key("world_backup_create_failed").param("world", world));
                Err(e)
            }
        }
    }

    pub async fn restore(&self, key: &str, world: &str, zip_name: &str) -> AppResult<()> {
        let build = self.build(key)?;
        self.idle(&build)?;
        let game = self.game(&build);
        let archive = self.store().zip_path(&build_folder(&build), world, zip_name)?;
        let lease = self.deps.instances.try_acquire(&game, "world_restore")?;
        let op = self
            .deps
            .feedback
            .begin(OperationSpec::new(Text::key("world_backup_restoring").param("world", world), "backup"));
        let (saves, name) = (game.join("saves"), world.to_string());
        let done = blocking(move || restore(&archive, &saves, &name)).await;
        drop(lease);
        match done {
            Ok(()) => {
                op.finish();
                self.deps.feedback.success(Text::key("world_backup_restored").param("world", world));
                Ok(())
            }
            Err(e) => {
                op.fail(Text::key("world_backup_restore_failed").param("world", world));
                Err(e)
            }
        }
    }

    pub async fn delete(&self, key: &str, world: &str, zip_name: &str) -> AppResult<()> {
        let build = self.build(key)?;
        self.idle(&build)?;
        let (store, folder, name, zip) =
            (self.store(), build_folder(&build), world.to_string(), zip_name.to_string());
        match blocking(move || store.delete(&folder, &name, &zip)).await {
            Ok(()) => {
                self.deps.feedback.success(Text::key("world_backup_deleted").param("world", world));
                Ok(())
            }
            Err(e) => {
                self.deps.feedback.error(Text::key("world_backup_delete_failed").param("world", world));
                Err(e)
            }
        }
    }

    /// Every backup of the build goes (asked for when the build is deleted).
    pub async fn delete_build(&self, key: &str) -> AppResult<()> {
        let build = self.build(key)?;
        let game = self.game(&build);
        let _lease = self.deps.instances.try_acquire(&game, "world_backup")?;
        // Only a build that can go loses its backups: a running game keeps both.
        self.idle(&build)?;
        let store = self.store();
        blocking(move || store.delete_build(&build)).await
    }
}
