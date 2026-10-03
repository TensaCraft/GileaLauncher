//! Home's «Продовжити гру»: the builds played last, each with its last activity — the server its
//! newest log shows it joined, or the world played last. Read from what the game and the launcher
//! already write in a build's folder; nothing is recorded for it.

pub mod logs;
pub mod worlds;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use launcher_shared::recent::{Activity, RecentBuild};

use self::logs::{LogMarkers, Marker};
use self::worlds::{World, worlds};
use crate::launch::options::game_dir;
use crate::minecraft::command::release_number;
use crate::storage::versions::VersionStore;

/// The builds on «Продовжити гру» at most.
pub const MOST: usize = 10;

pub struct RecentService {
    versions: Arc<VersionStore>,
    markers: LogMarkers,
}

fn modified_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The game opens a world from the command line from 1.20 on.
fn opens_worlds(minecraft: Option<&str>) -> bool {
    minecraft.and_then(release_number).is_some_and(|number| number >= (1, 20, 0))
}

fn world_activity(world: &World, quick_play: bool) -> Activity {
    Activity::World {
        folder: world.folder.clone(),
        name: world.name.clone(),
        mode: world.mode.clone(),
        difficulty: world.difficulty.clone(),
        hardcore: world.hardcore,
        icon: world.icon.clone(),
        quick_play,
    }
}

/// Build `key` (Minecraft `minecraft`) as Home shows it, from its game folder; `None` when it was
/// never played.
fn recent_of(markers: &LogMarkers, key: &str, minecraft: Option<&str>, game: &Path) -> Option<RecentBuild> {
    let found = worlds(game);
    let logs = game.join("logs");
    let played_ms = [
        modified_ms(&logs.join("launch.log")),
        modified_ms(&logs.join("latest.log")),
        found.first().map_or(0, |w| w.last_played_ms),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    if played_ms == 0 {
        return None;
    }
    let quick_play = opens_worlds(minecraft);
    let newest_world = found.first();
    let activity = match markers.last(game) {
        // A world played after that session ended is the later one.
        Some((Marker::Server { host, port }, ended)) => match newest_world {
            Some(world) if world.last_played_ms > ended => Some(world_activity(world, quick_play)),
            _ => Some(Activity::Server { host, port }),
        },
        Some((Marker::Singleplayer, _)) | None => newest_world.map(|w| world_activity(w, quick_play)),
    };
    Some(RecentBuild { key: key.to_string(), played_ms, activity })
}

impl RecentService {
    pub fn new(versions: Arc<VersionStore>) -> RecentService {
        RecentService { versions, markers: LogMarkers::default() }
    }

    /// The `count` builds played last (at most `MOST`), newest first. Blocking: reads logs and
    /// worlds (each log once while it stays as it is).
    pub fn recent(&self, count: usize) -> Vec<RecentBuild> {
        let mc_dir = self.versions.minecraft_dir();
        let mut found: Vec<RecentBuild> = self
            .versions
            .list()
            .iter()
            .filter_map(|build| {
                recent_of(&self.markers, &build.key, build.version.as_deref(), &game_dir(build, mc_dir))
            })
            .collect();
        found.sort_by_key(|r| std::cmp::Reverse(r.played_ms));
        found.truncate(count.min(MOST));
        found
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::worlds::tests::write_world;
    use super::*;

    fn now_ms() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    fn log(game: &Path, text: &str, ago: Duration) {
        let path = game.join("logs/latest.log");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - ago).unwrap();
    }

    #[test]
    fn a_build_never_played_is_not_recent() {
        let game = tempfile::tempdir().unwrap();
        assert_eq!(recent_of(&LogMarkers::default(), "k", Some("1.21.1"), game.path()), None);
    }

    #[test]
    fn the_server_of_the_last_session_is_the_activity() {
        let game = tempfile::tempdir().unwrap();
        write_world(game.path(), "Test", "Test", now_ms() - 3_600_000, 0);
        log(
            game.path(),
            "[1] [Render thread/INFO]: Connecting to tensa.co.ua, 25565\n",
            Duration::from_secs(60),
        );
        let recent = recent_of(&LogMarkers::default(), "k", Some("1.21.1"), game.path()).unwrap();
        assert_eq!(recent.activity, Some(Activity::Server { host: "tensa.co.ua".into(), port: 25565 }));
    }

    #[test]
    fn a_world_played_since_is_the_activity() {
        let game = tempfile::tempdir().unwrap();
        log(
            game.path(),
            "[1] [Render thread/INFO]: Connecting to tensa.co.ua, 25565\n",
            Duration::from_secs(3600),
        );
        write_world(game.path(), "Test", "Мій світ", now_ms() - 60_000, 1);
        let recent = recent_of(&LogMarkers::default(), "k", Some("1.16.5"), game.path()).unwrap();
        match recent.activity {
            Some(Activity::World { folder, name, mode, quick_play, .. }) => {
                assert_eq!((folder.as_str(), name.as_str(), mode.as_str()), ("Test", "Мій світ", "creative"));
                assert!(!quick_play, "1.16.5 opens no world from the launcher");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_singleplayer_session_takes_the_newest_world() {
        let game = tempfile::tempdir().unwrap();
        write_world(game.path(), "A", "A", now_ms() - 7_200_000, 0);
        write_world(game.path(), "B", "B", now_ms() - 120_000, 0);
        log(
            game.path(),
            "[2] [Server thread/INFO]: Starting integrated minecraft server version 1.21.1\n",
            Duration::from_secs(60),
        );
        let recent = recent_of(&LogMarkers::default(), "k", Some("1.21.1"), game.path()).unwrap();
        assert!(
            matches!(recent.activity, Some(Activity::World { ref folder, quick_play: true, .. }) if folder == "B")
        );
    }

    #[test]
    fn a_build_played_only_in_its_menu_has_no_activity() {
        let game = tempfile::tempdir().unwrap();
        log(game.path(), "[1] [main/INFO]: Loading Minecraft 1.21.1\n", Duration::from_secs(60));
        let recent = recent_of(&LogMarkers::default(), "k", Some("1.21.1"), game.path()).unwrap();
        assert_eq!(recent.activity, None);
        assert!(recent.played_ms > 0);
    }
}
