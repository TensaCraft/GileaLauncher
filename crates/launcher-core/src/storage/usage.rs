//! What each line of the «Сховище» tab is made of (`StorageLine`): the paths the launcher wrote,
//! found by name and place. One answer serves measuring, the clear buttons and the rules, so what
//! the tab shows is what a clear frees.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use launcher_shared::{StorageLine, StorageRule};
use serde_json::Value;

use crate::backups::{BuildBackups, an_update_s};
use crate::launch::process::launch_log;
use crate::storage::journal::{JOURNAL_DIR, SYNC_WORK_DIR, unfinished};
use crate::updater::stage::{DOWNLOADS_DIR, PENDING_DIR, marker_path, pending_dir};

const DAY: Duration = Duration::from_secs(24 * 3600);
/// A temporary file untouched this long is a leftover: a download in progress keeps writing its own.
pub const TEMP_AGE: Duration = DAY;
/// How long a mod's copy, a modpack's copy, a crash report, a log archive and a set-aside journal
/// are kept by the rules.
pub const MONTH: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long Java's dumps are kept by their rule.
pub const DUMP_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// What the screenshots' thumbnails may take at most.
pub const THUMBS_CAP: u64 = 500 * 1024 * 1024;

fn entries(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir).into_iter().flatten().flatten().collect()
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn is_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_dir())
}

fn is_file(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_file())
}

/// The files in `dir` (not below) whose names `keep` accepts.
fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    entries(dir).into_iter().filter(|e| is_file(e) && keep(&name_of(&e.path()))).map(|e| e.path()).collect()
}

/// The files below `dir`, at any depth, whose names `keep` accepts; links are not followed.
fn files_below(dir: &Path, keep: &dyn Fn(&str) -> bool, out: &mut Vec<PathBuf>) {
    for entry in entries(dir) {
        if is_dir(&entry) {
            files_below(&entry.path(), keep, out);
        } else if is_file(&entry) && keep(&name_of(&entry.path())) {
            out.push(entry.path());
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path).and_then(|m| m.modified()).ok()
}

/// `path` was last written at least `age` before `now`.
pub(crate) fn older_than(path: &Path, age: Duration, now: SystemTime) -> bool {
    modified(path).and_then(|at| now.duration_since(at).ok()).is_some_and(|d| d >= age)
}

/// A file's length, or a folder's files summed; a link counts as nothing and a missing path as 0.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    if meta.is_dir() {
        entries(path).iter().map(|e| dir_size(&e.path())).sum()
    } else if meta.is_file() {
        meta.len()
    } else {
        0
    }
}

/// What `items` take together.
pub fn line_size(items: &[PathBuf]) -> u64 {
    items.iter().map(|p| dir_size(p)).sum()
}

/// The world backups in `worlds` (a build's): each zip whose metadata names its world, with that
/// metadata, as the backups module writes them.
fn world_backups(worlds: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for world in entries(worlds).into_iter().filter(is_dir) {
        for meta in files(&world.path(), |n| n.ends_with(".zip.json")) {
            let zip = meta.with_file_name(name_of(&meta).trim_end_matches(".json"));
            if zip.is_file() && read_json(&meta).is_some_and(|m| m.get("world_folder").is_some()) {
                out.extend([zip, meta]);
            }
        }
    }
    out
}

/// The files of a build's service folder besides its journals and notes.
pub(crate) struct Service {
    /// Each change's work folder, with its id.
    pub work: Vec<(PathBuf, String)>,
    /// The ids of the changes not over yet; `None` when such a journal could not be read (then
    /// every work folder stays).
    pub unfinished: Option<HashSet<String>>,
    /// Set-aside journals (`<file>.<ms>.failed`), with the id of their change.
    pub set_aside: Vec<(PathBuf, Option<String>)>,
    /// Downloads an install keeps there while it runs (`modpack.*`).
    pub downloads: Vec<PathBuf>,
}

fn transaction_id(journal: &Path) -> Option<String> {
    read_json(journal)?.get("transaction_id")?.as_str().map(str::to_string)
}

pub(crate) fn service(game: &Path) -> Service {
    let journals = game.join(JOURNAL_DIR);
    let mut ids = Some(HashSet::new());
    for journal in files(&journals, |n| n.ends_with(".json")).into_iter().filter(|j| unfinished(j)) {
        match (transaction_id(&journal), ids.as_mut()) {
            (Some(id), Some(ids)) => {
                ids.insert(id);
            }
            _ => ids = None,
        }
    }
    let set_aside = files(&journals, |n| n.ends_with(".failed"))
        .into_iter()
        .map(|path| {
            let id = transaction_id(&path);
            (path, id)
        })
        .collect();
    let work = entries(&game.join(SYNC_WORK_DIR))
        .into_iter()
        .filter(is_dir)
        .map(|e| (e.path(), name_of(&e.path())))
        .collect();
    let downloads = files(&game.join(".launcher"), |n| n.starts_with("modpack."));
    Service { work, unfinished: ids, set_aside, downloads }
}

impl Service {
    /// The work folders that no change that is not over names.
    pub(crate) fn idle_work(&self) -> Vec<(PathBuf, String)> {
        let Some(unfinished) = &self.unfinished else { return Vec::new() };
        self.work.iter().filter(|(_, id)| !unfinished.contains(id)).cloned().collect()
    }
}

/// The game's crash reports: the files in `crash-reports/`.
pub(crate) fn crash_reports(game: &Path) -> Vec<PathBuf> {
    files(&game.join("crash-reports"), |_| true)
}

/// Java's dumps in the build's folder: `hs_err_pid*.log`, `replay_pid*.log`, `win_event*.txt`.
pub(crate) fn dumps(game: &Path) -> Vec<PathBuf> {
    files(game, |n| {
        ((n.starts_with("hs_err_pid") || n.starts_with("replay_pid")) && n.ends_with(".log"))
            || (n.starts_with("win_event") && n.ends_with(".txt"))
    })
}

/// The game's old logs: the archives (`*.gz`) below `logs/`.
pub(crate) fn log_archives(game: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    files_below(&game.join("logs"), &|n| n.ends_with(".gz"), &mut out);
    out
}

/// The paths build line `line` is made of in `game` and its backups (none for a launcher line).
pub fn build_items(line: StorageLine, game: &Path, backups: &BuildBackups) -> Vec<PathBuf> {
    match line {
        StorageLine::WorldBackups => world_backups(&backups.worlds()),
        StorageLine::ModBackups => {
            files(&backups.mods(), |n| n.ends_with(".backup") || n.ends_with(".backup.part"))
        }
        StorageLine::ModpackBackups => entries(&backups.modpacks())
            .into_iter()
            .filter(|e| is_dir(e) && an_update_s(&name_of(&e.path())))
            .map(|e| e.path())
            .collect(),
        StorageLine::ServiceFolder => {
            let service = service(game);
            let mut out: Vec<PathBuf> = service.idle_work().into_iter().map(|(path, _)| path).collect();
            out.extend(service.set_aside.into_iter().map(|(path, _)| path));
            out.extend(Some(launch_log(game)).filter(|log| log.is_file()));
            out.extend(service.downloads);
            out
        }
        StorageLine::CrashReports => crash_reports(game).into_iter().chain(dumps(game)).collect(),
        StorageLine::GameLogs => log_archives(game),
        StorageLine::Thumbnails | StorageLine::UpdateDownloads | StorageLine::TempFiles => Vec::new(),
    }
}

/// When the modpack update whose copies are in folder `name` (its time, `2026-10-01_10-00-00`) ran.
fn update_time(name: &str) -> Option<SystemTime> {
    let time = chrono::NaiveDateTime::parse_from_str(name.get(..19)?, "%Y-%m-%d_%H-%M-%S").ok()?.and_utc();
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(time.timestamp()).ok()?))
}

/// The paths rule `rule` takes from build `game` and its backups at `now` (the thumbnails' rule
/// is `stale_thumbnails`).
pub fn rule_items(rule: StorageRule, game: &Path, backups: &BuildBackups, now: SystemTime) -> Vec<PathBuf> {
    let past = |age: Duration| move |path: &PathBuf| older_than(path, age, now);
    match rule {
        StorageRule::ModBackups => {
            build_items(StorageLine::ModBackups, game, backups).into_iter().filter(past(MONTH)).collect()
        }
        StorageRule::ModpackBackups => {
            let mut updates = build_items(StorageLine::ModpackBackups, game, backups);
            // Their names are their times: the last one is the newest.
            updates.sort();
            let newest = updates.pop();
            let stale = newest.filter(|folder| {
                update_time(&name_of(folder))
                    .and_then(|at| now.duration_since(at).ok())
                    .is_some_and(|d| d >= MONTH)
            });
            updates.into_iter().chain(stale).collect()
        }
        StorageRule::JavaDumps => dumps(game).into_iter().filter(past(DUMP_AGE)).collect(),
        StorageRule::CrashReports => {
            crash_reports(game).into_iter().chain(log_archives(game)).filter(past(MONTH)).collect()
        }
        StorageRule::ServiceFolder => {
            let service = service(game);
            // A set-aside change's work keeps copies for a repair by hand, as long as its journal.
            let (old, young): (Vec<_>, Vec<_>) =
                service.set_aside.iter().cloned().partition(|(path, _)| older_than(path, MONTH, now));
            let kept: HashSet<String> = young.into_iter().filter_map(|(_, id)| id).collect();
            let mut out: Vec<PathBuf> = service
                .idle_work()
                .into_iter()
                .filter(|(_, id)| !kept.contains(id))
                .map(|(p, _)| p)
                .collect();
            out.extend(old.into_iter().map(|(path, _)| path));
            out.extend(service.downloads);
            out
        }
        StorageRule::Thumbnails => Vec::new(),
    }
}

/// The thumbnails in `thumbs` to remove: every one not `wanted` (its screenshot is gone or
/// changed), then the oldest of the rest until they take at most `cap`.
pub fn stale_thumbnails(thumbs: &Path, wanted: &HashSet<PathBuf>, cap: u64) -> Vec<PathBuf> {
    let (mut kept, mut out): (Vec<PathBuf>, Vec<PathBuf>) =
        files(thumbs, |n| n.ends_with(".jpg")).into_iter().partition(|path| wanted.contains(path));
    out.sort();
    let mut total: u64 = kept.iter().map(|p| dir_size(p)).sum();
    kept.sort_by_key(|path| modified(path));
    for path in kept {
        if total <= cap {
            break;
        }
        total = total.saturating_sub(dir_size(&path));
        out.push(path);
    }
    out
}

fn temporary(name: &str) -> bool {
    name.ends_with(".part") || name.ends_with(".tmp") || name.ends_with(".part.json")
}

/// The temporary files below `dir` untouched for `TEMP_AGE`, but for its folders named in `skip`.
fn leftovers(dir: &Path, skip: &[&str], now: SystemTime, out: &mut Vec<PathBuf>) {
    for entry in entries(dir) {
        let path = entry.path();
        if is_dir(&entry) {
            if !skip.contains(&name_of(&path).as_str()) {
                leftovers(&path, &[], now, out);
            }
        } else if is_file(&entry) && temporary(&name_of(&path)) && older_than(&path, TEMP_AGE, now) {
            out.push(path);
        }
    }
}

/// The paths launcher line `line` is made of: in the cache, and for temporary files in the builds'
/// folders `games` too (none for a build line).
pub fn launcher_items(line: StorageLine, cache: &Path, games: &[PathBuf], now: SystemTime) -> Vec<PathBuf> {
    match line {
        StorageLine::Thumbnails => files(&cache.join("thumbs"), |n| n.ends_with(".jpg")),
        StorageLine::UpdateDownloads => {
            let downloads = pending_dir(cache).join(DOWNLOADS_DIR);
            if marker_path(cache).exists() || !downloads.exists() { Vec::new() } else { vec![downloads] }
        }
        StorageLine::TempFiles => {
            let mut out = Vec::new();
            for game in games {
                leftovers(game, &["saves", ".launcher"], now, &mut out);
            }
            leftovers(cache, &[PENDING_DIR], now, &mut out);
            out
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use launcher_shared::{StorageLine, StorageRule};
    use serde_json::json;

    use super::*;
    use crate::backups::BuildBackups;

    const HOUR: Duration = Duration::from_secs(3600);

    fn put(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn aged(path: &Path, age: Duration) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    fn sorted(mut items: Vec<PathBuf>) -> Vec<PathBuf> {
        items.sort();
        items
    }

    fn setup() -> (tempfile::TempDir, PathBuf, BuildBackups) {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("games/aero");
        fs::create_dir_all(&game).unwrap();
        let backups = BuildBackups::of(&tmp.path().join("backups"), "aero");
        (tmp, game, backups)
    }

    #[test]
    fn world_backups_are_the_zips_with_their_metadata() {
        let (_tmp, game, backups) = setup();
        let worlds = backups.worlds();
        let zip = put(&worlds, "World/a.zip", b"zip");
        let meta = put(&worlds, "World/a.zip.json", json!({"world_folder": "World"}).to_string().as_bytes());
        put(&worlds, "World/notes.txt", b"mine");
        put(&worlds, "World/b.zip", b"no metadata");
        put(&worlds, "notes.txt", b"mine");
        let items = build_items(StorageLine::WorldBackups, &game, &backups);
        assert_eq!(sorted(items.clone()), sorted(vec![zip, meta]));
        assert_eq!(line_size(&items), 3 + json!({"world_folder": "World"}).to_string().len() as u64);
    }

    #[test]
    fn mod_and_modpack_backups_are_what_the_launcher_wrote() {
        let (_tmp, game, backups) = setup();
        let copy = put(&backups.mods(), "sodium.jar.backup", b"old");
        let part = put(&backups.mods(), "iris.jar.backup.part", b"half");
        put(&backups.mods(), "notes.txt", b"mine");
        let update = backups.modpacks().join("2026-10-01_10-00-00");
        put(&update, "config/a.toml", b"mine");
        put(&backups.modpacks(), "My pack/a.toml", b"mine");
        let mods = build_items(StorageLine::ModBackups, &game, &backups);
        assert_eq!(sorted(mods), sorted(vec![copy, part]));
        assert_eq!(build_items(StorageLine::ModpackBackups, &game, &backups), vec![update]);
    }

    #[test]
    fn the_service_folder_line_leaves_journals_and_notes() {
        let (_tmp, game, backups) = setup();
        let applying = json!({"schema_version": 2, "status": "applying", "transaction_id": "ab"});
        put(&game, ".launcher/journal/modrinth-content.json", applying.to_string().as_bytes());
        put(&game, ".launcher/journal/tensa.json", json!({"status": "complete"}).to_string().as_bytes());
        let failed = json!({"schema_version": 2, "status": "repair_required", "transaction_id": "ef"});
        let set_aside = put(&game, ".launcher/journal/tensa.json.1700.failed", failed.to_string().as_bytes());
        put(&game, ".launcher/work/ab/backup/mods/a.jar", b"old");
        put(&game, ".launcher/work/cd/stage/mods/a.jar", b"left");
        put(&game, ".launcher/work/ef/backup/mods/a.jar", b"old");
        let log = put(&game, ".launcher/launch.log", b"started");
        let download = put(&game, ".launcher/modpack.mrpack", b"pack");
        put(&game, ".launcher/tensa-checked.json", b"{}");
        let items = build_items(StorageLine::ServiceFolder, &game, &backups);
        let work = game.join(".launcher/work");
        assert_eq!(sorted(items), sorted(vec![work.join("cd"), work.join("ef"), set_aside, log, download]));
    }

    #[test]
    fn crash_reports_and_dumps_are_items_and_the_games_files_are_not() {
        let (_tmp, game, backups) = setup();
        let report = put(&game, "crash-reports/crash-2026-10-01.txt", b"crash");
        let dump = put(&game, "hs_err_pid42.log", b"dump");
        let replay = put(&game, "replay_pid42.log", b"replay");
        let event = put(&game, "win_event_1.txt", b"event");
        put(&game, "options.txt", b"mine");
        put(&game, "logs/latest.log", b"log");
        let items = build_items(StorageLine::CrashReports, &game, &backups);
        assert_eq!(sorted(items), sorted(vec![report, dump, replay, event]));
    }

    #[test]
    fn log_archives_are_items_and_the_current_logs_are_not() {
        let (_tmp, game, backups) = setup();
        let archive = put(&game, "logs/2026-10-01-1.log.gz", b"gz");
        let nested = put(&game, "logs/telemetry/2026-10-01.log.gz", b"gz");
        put(&game, "logs/latest.log", b"log");
        put(&game, "logs/debug.log", b"log");
        let items = build_items(StorageLine::GameLogs, &game, &backups);
        assert_eq!(sorted(items), sorted(vec![archive, nested]));
    }

    #[test]
    fn a_temp_file_younger_than_a_day_is_not_an_item() {
        let (tmp, game, _backups) = setup();
        let cache = tmp.path().join("cache");
        let old: Vec<PathBuf> =
            ["mods/sodium.jar.part.0123.tmp", "mods/sodium.jar.part.json", "config/x.part"]
                .iter()
                .map(|relative| put(&game, relative, b"half"))
                .chain([put(&cache, "thumbs/ab.jpg.42-0.part", b"half")])
                .collect();
        let young = put(&game, "config/y.tmp", b"writing");
        let kept = [
            put(&game, "saves/World/level.dat.tmp", b"the game's"),
            put(&game, ".launcher/work/ab/stage/x.part", b"a change"),
            put(&cache, "pending-update/downloads/0.1.0/x.part", b"resuming"),
        ];
        for path in old.iter().chain(&kept) {
            aged(path, 25 * HOUR);
        }
        aged(&young, 23 * HOUR);
        let items = launcher_items(StorageLine::TempFiles, &cache, &[game], SystemTime::now());
        assert_eq!(sorted(items), sorted(old));
    }

    #[test]
    fn a_prepared_update_s_downloads_are_not_an_item() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let downloads = crate::updater::stage::pending_dir(&cache).join(crate::updater::stage::DOWNLOADS_DIR);
        put(&downloads, "0.2.0/Launcher-new", b"payload");
        let thumb = put(&cache, "thumbs/ab.jpg", b"jpg");
        let now = SystemTime::now();
        assert_eq!(launcher_items(StorageLine::UpdateDownloads, &cache, &[], now), vec![downloads.clone()]);
        assert_eq!(launcher_items(StorageLine::Thumbnails, &cache, &[], now), vec![thumb]);
        fs::write(crate::updater::stage::marker_path(&cache), b"{}").unwrap();
        assert!(launcher_items(StorageLine::UpdateDownloads, &cache, &[], now).is_empty());
    }

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn rules_take_what_is_past_its_age() {
        let (_tmp, game, backups) = setup();
        let now = SystemTime::now();
        let young_copy = put(&backups.mods(), "a.jar.backup", b"old");
        let old_copy = put(&backups.mods(), "b.jar.backup", b"old");
        aged(&young_copy, 29 * DAY);
        aged(&old_copy, 31 * DAY);
        assert_eq!(rule_items(StorageRule::ModBackups, &game, &backups, now), vec![old_copy]);

        let young_dump = put(&game, "hs_err_pid1.log", b"dump");
        let old_dump = put(&game, "hs_err_pid2.log", b"dump");
        aged(&young_dump, 6 * DAY);
        aged(&old_dump, 8 * DAY);
        assert_eq!(rule_items(StorageRule::JavaDumps, &game, &backups, now), vec![old_dump]);

        let young_report = put(&game, "crash-reports/c1.txt", b"crash");
        let old_report = put(&game, "crash-reports/c2.txt", b"crash");
        let young_logs = put(&game, "logs/y.log.gz", b"gz");
        let old_logs = put(&game, "logs/x.log.gz", b"gz");
        for (path, days) in [(&young_report, 29), (&old_report, 31), (&young_logs, 29), (&old_logs, 31)] {
            aged(path, days * DAY);
        }
        let reports = rule_items(StorageRule::CrashReports, &game, &backups, now);
        assert_eq!(sorted(reports), sorted(vec![old_report, old_logs]), "dumps have their own rule");

        put(&game, ".launcher/work/cd/stage/a.jar", b"left");
        let young_aside = json!({"status": "repair_required", "transaction_id": "ef"});
        let young_aside =
            put(&game, ".launcher/journal/tensa.json.1.failed", young_aside.to_string().as_bytes());
        put(&game, ".launcher/work/ef/backup/a.jar", b"old");
        let old_aside = json!({"status": "repair_required", "transaction_id": "gh"});
        let old_aside = put(&game, ".launcher/journal/tensa.json.2.failed", old_aside.to_string().as_bytes());
        put(&game, ".launcher/work/gh/backup/a.jar", b"old");
        aged(&young_aside, 29 * DAY);
        aged(&old_aside, 31 * DAY);
        let download = put(&game, ".launcher/modpack.zip", b"pack");
        put(&game, ".launcher/launch.log", b"started");
        let work = game.join(".launcher/work");
        let service = rule_items(StorageRule::ServiceFolder, &game, &backups, now);
        assert_eq!(sorted(service), sorted(vec![work.join("cd"), work.join("gh"), old_aside, download]));
    }

    #[test]
    fn only_the_newest_modpack_copy_stays() {
        let (_tmp, game, backups) = setup();
        let older = backups.modpacks().join("2026-10-01_10-00-00");
        let newest = backups.modpacks().join("2026-10-05_10-00-00");
        put(&older, "config/a.toml", b"mine");
        put(&newest, "config/a.toml", b"mine");
        let at = |text: &str| -> SystemTime {
            let time = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap().and_utc();
            SystemTime::UNIX_EPOCH + Duration::from_secs(time.timestamp() as u64)
        };
        let soon = at("2026-10-12 10:00");
        assert_eq!(rule_items(StorageRule::ModpackBackups, &game, &backups, soon), vec![older.clone()]);
        let later = at("2026-11-10 10:00");
        let both = rule_items(StorageRule::ModpackBackups, &game, &backups, later);
        assert_eq!(sorted(both), sorted(vec![older, newest]), "the newest too, past its month");
    }

    #[test]
    fn thumbnails_of_gone_screenshots_go_then_the_oldest_over_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let thumbs = tmp.path().join("thumbs");
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| put(&thumbs, &format!("{n}.jpg"), &[0; 10]));
        aged(&a, 2 * DAY);
        aged(&c, 3 * DAY);
        aged(&d, DAY);
        let wanted: std::collections::HashSet<PathBuf> = [a, c.clone(), d].into_iter().collect();
        assert_eq!(stale_thumbnails(&thumbs, &wanted, 20), vec![b, c], "the gone one, then the oldest");
    }

    #[test]
    fn sizes_add_up() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "a/b/c.bin", &[0; 1000]);
        put(tmp.path(), "a/d.bin", &[0; 24]);
        assert_eq!(dir_size(&tmp.path().join("a")), 1024);
        assert_eq!(dir_size(&tmp.path().join("a/d.bin")), 24);
        assert_eq!(dir_size(&tmp.path().join("missing")), 0);
    }
}
