//! The storage service behind the «Сховище» settings tab. It measures what the launcher's own
//! files take (`storage::usage`) and clears a line when asked. Once a day it also cleans by the
//! rules whose switches are on. No clear and no rule touches a build whose game runs or that
//! another operation holds.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_shared::{
    AppError, AppResult, BuildStorage, ErrorCode, Level, LineSize, RuleState, StorageLine, StorageRule,
    StorageSnapshot, Text, units,
};
use serde_json::json;

use super::config::ConfigStore;
use super::usage::{
    THUMBS_CAP, build_items, dir_size, launcher_items, line_size, old_sync_copies, rule_items,
    stale_thumbnails, temp_files,
};
use super::versions::{Build, VersionStore};
use crate::backups::{BackupsRoot, BuildBackups};
use crate::content::screenshots::list_screenshots;
use crate::content::thumbs::Thumbs;
use crate::feedback::FeedbackService;
use crate::launch::alive::game_open;
use crate::launch::options::game_dir;
use crate::loaders::ComponentSpec;
use crate::lock::{Coordinator, Lease};
use launcher_shared::LoaderKind;

/// When the rules last ran (ms since the epoch).
pub const CLEANED_AT: &str = "storage_cleaned_at";
const DAY_MS: u64 = 24 * 3600 * 1000;

/// What the service needs of the launcher.
pub struct StorageDeps {
    pub config: Arc<ConfigStore>,
    pub versions: Arc<VersionStore>,
    pub instances: Arc<Coordinator>,
    pub feedback: Arc<FeedbackService>,
    pub backups: BackupsRoot,
    pub cache_dir: PathBuf,
    /// Whether build `key`'s game runs.
    pub running: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Removes the downloaded launcher updates, unless one is prepared or downloading.
    pub clear_updates: Arc<dyn Fn() -> AppResult<()> + Send + Sync>,
    /// The language sizes are written in.
    pub lang: Arc<dyn Fn() -> String + Send + Sync>,
}

pub struct StorageService {
    deps: StorageDeps,
    cached: Mutex<Option<StorageSnapshot>>,
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Removes `items`; one that cannot go (a file another program holds) is logged and left. What
/// was freed.
fn remove(items: &[PathBuf]) -> u64 {
    let mut freed = 0;
    for item in items {
        let size = dir_size(item);
        match remove_any(item) {
            Ok(()) => freed += size,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("Unable to remove {}: {e}", item.display()),
        }
    }
    freed
}

/// `build` has no mod loader.
fn vanilla(build: &Build) -> bool {
    ComponentSpec::from_build(build).is_none_or(|spec| spec.kind == LoaderKind::Minecraft)
}

impl StorageService {
    pub fn new(deps: StorageDeps) -> StorageService {
        StorageService { deps, cached: Mutex::new(None) }
    }

    fn mc_dir(&self) -> &Path {
        self.deps.versions.minecraft_dir()
    }

    fn game(&self, build: &Build) -> PathBuf {
        game_dir(build, self.mc_dir())
    }

    fn shelf(&self, build: &Build) -> BuildBackups {
        BuildBackups::of_build(&(self.deps.backups)(), build)
    }

    fn rule_on(&self, rule: StorageRule) -> bool {
        self.deps.config.get_str(rule.config_key()).is_none_or(|v| !v.eq_ignore_ascii_case("no"))
    }

    fn rules(&self) -> Vec<RuleState> {
        StorageRule::ALL.iter().map(|&rule| RuleState { rule, on: self.rule_on(rule) }).collect()
    }

    fn running(&self, build: &Build) -> bool {
        (self.deps.running)(&build.key) || game_open(&self.game(build))
    }

    /// Holds `build` for a clear: its game must not run and no other operation may hold it.
    fn hold(&self, build: &Build) -> AppResult<Lease> {
        let game = self.game(build);
        if self.running(build) {
            return Err(AppError::new(ErrorCode::InstanceBusy, format!("{} runs", build.key))
                .with_param("version", &build.name)
                .with_param("kind", "launch"));
        }
        self.deps.instances.try_acquire(&game, "storage")
    }

    fn build(&self, key: &str) -> AppResult<Build> {
        self.deps.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })
    }

    fn measure(&self) -> StorageSnapshot {
        let builds = self.deps.versions.list();
        let games: Vec<PathBuf> = builds.iter().map(|b| self.game(b)).collect();
        let builds = builds
            .iter()
            .zip(&games)
            .map(|(build, game)| {
                let shelf = self.shelf(build);
                BuildStorage {
                    key: build.key.clone(),
                    name: build.name.clone(),
                    vanilla: vanilla(build),
                    running: self.running(build),
                    lines: StorageLine::BUILD
                        .iter()
                        .map(|&line| LineSize { line, bytes: line_size(&build_items(line, game, &shelf)) })
                        .collect(),
                }
            })
            .collect();
        let now = SystemTime::now();
        let launcher = StorageLine::LAUNCHER
            .iter()
            .map(|&line| LineSize {
                line,
                bytes: line_size(&launcher_items(line, &self.deps.cache_dir, &games, now)),
            })
            .collect();
        let free = fs4::available_space(self.mc_dir()).ok();
        StorageSnapshot { builds, launcher, free, rules: self.rules() }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Option<StorageSnapshot>> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the launcher's files take: measured the first time, then kept until a clear or
    /// `refresh`.
    pub fn snapshot(&self, refresh: bool) -> StorageSnapshot {
        if !refresh && let Some(mut snapshot) = self.cache().clone() {
            // Kept sizes, but whose game runs is told as it is now.
            for kept in &mut snapshot.builds {
                kept.running = self.deps.versions.get(&kept.key).is_some_and(|b| self.running(&b));
            }
            return snapshot;
        }
        let snapshot = self.measure();
        *self.cache() = Some(snapshot.clone());
        snapshot
    }

    fn note(&self, key: &str, freed: u64) {
        let size = units::size(freed, &(self.deps.lang)());
        self.deps.feedback.note(Level::Info, Text::key(key).param("size", size));
    }

    /// Clears line `line` of build `key` (`None`: a launcher line); notes what it freed.
    pub fn clear(&self, key: Option<&str>, line: StorageLine) -> AppResult<StorageSnapshot> {
        let freed = match key {
            Some(key) => {
                let build = self.build(key)?;
                let _held = self.hold(&build)?;
                remove(&build_items(line, &self.game(&build), &self.shelf(&build)))
            }
            None => self.clear_launcher(line)?,
        };
        self.note("storage_cleaned", freed);
        Ok(self.snapshot(true))
    }

    fn clear_launcher(&self, line: StorageLine) -> AppResult<u64> {
        let now = SystemTime::now();
        let items = launcher_items(line, &self.deps.cache_dir, &[], now);
        match line {
            StorageLine::UpdateDownloads => {
                let size = line_size(&items);
                (self.deps.clear_updates)()?;
                Ok(size)
            }
            StorageLine::TempFiles => {
                // A build's only while it is held: a change being undone writes a copy whose date is
                // its source's (Windows keeps it).
                let mut freed = remove(&items);
                for build in self.deps.versions.list() {
                    if let Ok(_held) = self.hold(&build) {
                        freed += remove(&temp_files(&self.game(&build), now));
                    }
                }
                Ok(freed)
            }
            _ => Ok(remove(&items)),
        }
    }

    /// Clears every line «Очистити непотрібне» names, in every build that is not running or held,
    /// and the launcher's; notes the whole once.
    pub fn clear_unneeded(&self) -> AppResult<StorageSnapshot> {
        let mut freed = 0;
        for build in self.deps.versions.list() {
            let Ok(_held) = self.hold(&build) else {
                tracing::info!("Not clearing {}: it is running or busy", build.key);
                continue;
            };
            let (game, shelf) = (self.game(&build), self.shelf(&build));
            for line in StorageLine::BUILD.iter().filter(|l| l.unneeded()) {
                freed += remove(&build_items(*line, &game, &shelf));
            }
        }
        for line in StorageLine::LAUNCHER {
            match self.clear_launcher(line) {
                Ok(size) => freed += size,
                Err(e) => tracing::info!("Not clearing {line:?}: {e}"),
            }
        }
        self.note("storage_cleaned", freed);
        Ok(self.snapshot(true))
    }

    /// Turns rule `rule` on or off.
    pub fn set_rule(&self, rule: StorageRule, on: bool) -> AppResult<StorageSnapshot> {
        let value = if on { "yes" } else { "no" };
        self.deps.config.set(rule.config_key(), json!(value)).map_err(|e| {
            AppError::new(ErrorCode::Io, e.to_string())
                .with_param("path", self.deps.config.path().to_string_lossy())
        })?;
        let mut cache = self.cache();
        match cache.as_mut() {
            Some(snapshot) => {
                snapshot.rules = self.rules();
                Ok(snapshot.clone())
            }
            None => {
                drop(cache);
                Ok(self.snapshot(false))
            }
        }
    }

    /// Cleans by the rules that are on, at most once a day (`CLEANED_AT`), every build that is not
    /// running or held; a damaged `config.json` stops it (the file is never rewritten by a start).
    /// What it freed.
    pub fn auto_clean(&self, now: SystemTime) -> u64 {
        let config = &self.deps.config;
        if !config.is_healthy() {
            return 0;
        }
        let at = millis(now);
        let last = config.get_u64(CLEANED_AT);
        if last.is_some_and(|last| at.saturating_sub(last) < DAY_MS) {
            return 0;
        }
        let rules: Vec<StorageRule> = StorageRule::ALL.into_iter().filter(|r| self.rule_on(*r)).collect();
        let builds = self.deps.versions.list();
        let mut freed = 0;
        for build in &builds {
            let Ok(_held) = self.hold(build) else { continue };
            let (game, shelf) = (self.game(build), self.shelf(build));
            if last.is_none() {
                // The first clean: an older version dated a mod's copy as its jar (Windows keeps a
                // copied file's time), so the copies it made are dated now and age from here.
                for copy in build_items(StorageLine::ModBackups, &game, &shelf) {
                    let dated = fs::File::options().write(true).open(&copy).and_then(|f| f.set_modified(now));
                    if let Err(e) = dated {
                        tracing::warn!("Unable to date {}: {e}", copy.display());
                    }
                }
            }
            for rule in rules.iter().filter(|r| last.is_some() || **r != StorageRule::ModBackups) {
                freed += remove(&rule_items(*rule, &game, &shelf, now));
            }
            // Sync keeps no copies since 0.0.18: what older passes kept goes, whatever the rules.
            freed += remove(&old_sync_copies(&shelf));
            let _ = fs::remove_dir(shelf.sync());
        }
        if rules.contains(&StorageRule::Thumbnails) {
            let thumbs = Thumbs::new(self.deps.cache_dir.join("thumbs"));
            let wanted: HashSet<PathBuf> = builds
                .iter()
                .flat_map(|b| list_screenshots(&self.game(b)))
                .map(|shot| thumbs.path_of(&shot))
                .collect();
            freed += remove(&stale_thumbnails(&self.deps.cache_dir.join("thumbs"), &wanted, THUMBS_CAP));
        }
        if let Err(e) = config.set(CLEANED_AT, json!(at)) {
            tracing::warn!("Unable to note the storage clean: {e}");
        }
        if freed > 0 {
            tracing::info!("Cleaned {freed} bytes of the launcher's files by its rules");
            self.note("storage_cleaned_auto", freed);
            *self.cache() = None;
        }
        freed
    }
}
