//! What a build's game logs say the player did last: joined a server (`Connecting to host, port`)
//! or opened a world (`Starting integrated minecraft server`). The game writes both on every
//! version and loader; older sessions are in `logs/*.log.gz`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

/// The last thing a log shows the player doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Server { host: String, port: u16 },
    Singleplayer,
}

/// How many logs of a build are read, newest first, to find its last session with a marker.
const LOGS_READ: usize = 8;

static CONNECTING: LazyLock<Regex> = LazyLock::new(|| {
    // The voice-chat mod's "Connecting to voice chat server: '…'" has no ", port" after a host.
    Regex::new(r"\]: Connecting to ([^\s,'/]+), (\d{1,5})\s*$").expect("a valid pattern")
});
const SINGLEPLAYER: &str = "]: Starting integrated minecraft server";

/// The last marker of a log's `text`.
pub fn last_marker(text: &str) -> Option<Marker> {
    text.lines().rev().find_map(|line| {
        if let Some(found) = CONNECTING.captures(line) {
            let port = found[2].parse().ok()?;
            return Some(Marker::Server { host: found[1].to_string(), port });
        }
        line.contains(SINGLEPLAYER).then_some(Marker::Singleplayer)
    })
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// A log's text: `.gz` ones unpacked; bytes that are not UTF-8 are replaced.
fn read_log(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    let file = File::open(path).ok()?;
    if path.extension().is_some_and(|e| e == "gz") {
        flate2::read::GzDecoder::new(file).read_to_end(&mut bytes).ok()?;
    } else {
        let mut file = file;
        file.read_to_end(&mut bytes).ok()?;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

type Stamp = (u64, Option<SystemTime>);

/// Markers by log, kept while a log's size and time stay (a visit to Home reads no log twice).
#[derive(Default)]
pub struct LogMarkers {
    known: Mutex<HashMap<PathBuf, (Stamp, Option<Marker>)>>,
}

impl LogMarkers {
    fn marker(&self, path: &Path, meta: &fs::Metadata) -> Option<Marker> {
        let stamp = (meta.len(), meta.modified().ok());
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((seen, marker)) = known.get(path)
            && *seen == stamp
        {
            return marker.clone();
        }
        drop(known);
        let marker = read_log(path).and_then(|text| last_marker(&text));
        known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        known.insert(path.to_path_buf(), (stamp, marker.clone()));
        marker
    }

    /// The last marker of the newest of `game`'s logs that has one, and when that log was last
    /// written (the session's end).
    pub fn last(&self, game: &Path) -> Option<(Marker, u64)> {
        let Ok(entries) = fs::read_dir(game.join("logs")) else { return None };
        let mut logs: Vec<(PathBuf, fs::Metadata)> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name == "latest.log" || name.ends_with(".log.gz")
            })
            .filter_map(|e| Some((e.path(), e.metadata().ok().filter(fs::Metadata::is_file)?)))
            .collect();
        logs.sort_by_key(|(_, meta)| std::cmp::Reverse(modified_ms(meta)));
        logs.into_iter()
            .take(LOGS_READ)
            .find_map(|(path, meta)| self.marker(&path, &meta).map(|m| (m, modified_ms(&meta))))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_server_joined_is_found_in_every_loader_s_log() {
        let neoforge = "[03жовт.2026 16:13:20.310] [Render thread/INFO] [net.minecraft.client.gui.screens.ConnectScreen/]: Connecting to auro.tensa.co.ua, 25565\n\
            [03жовт.2026 16:13:28.226] [Render thread/INFO] [voicechat/]: [voicechat] Connecting to voice chat server: '95.217.119.207:65535'\n";
        assert_eq!(
            last_marker(neoforge),
            Some(Marker::Server { host: "auro.tensa.co.ua".into(), port: 25565 })
        );
        let vanilla = "[12:14:40] [Render thread/INFO]: Connecting to tensa.co.ua, 25565\n[12:14:47] [Render thread/INFO]: Loaded 315 advancements\n";
        assert_eq!(last_marker(vanilla), Some(Marker::Server { host: "tensa.co.ua".into(), port: 25565 }));
    }

    #[test]
    fn the_last_of_a_server_and_a_world_counts() {
        let server_then_world = "[1] [Render thread/INFO]: Connecting to a.example, 25566\n\
            [2] [Server thread/INFO]: Starting integrated minecraft server version 1.21.1\n";
        assert_eq!(last_marker(server_then_world), Some(Marker::Singleplayer));
        let world_then_server = "[2] [Server thread/INFO]: Starting integrated minecraft server version 1.21.1\n\
            [3] [Render thread/INFO]: Connecting to a.example, 25566\n";
        assert_eq!(
            last_marker(world_then_server),
            Some(Marker::Server { host: "a.example".into(), port: 25566 })
        );
        assert_eq!(last_marker("[1] [main/INFO]: Loading Minecraft 1.21.1\n"), None);
    }

    fn write_gz(path: &Path, text: &str) {
        let mut gz = flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
        gz.write_all(text.as_bytes()).unwrap();
        gz.finish().unwrap();
    }

    fn age(path: &Path, secs: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn the_newest_log_with_a_marker_tells_the_last_session() {
        let game = tempfile::tempdir().unwrap();
        let logs = game.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let markers = LogMarkers::default();
        assert_eq!(markers.last(game.path()), None);
        write_gz(
            &logs.join("2026-09-01-1.log.gz"),
            "[1] [Render thread/INFO]: Connecting to old.example, 25565\n",
        );
        age(&logs.join("2026-09-01-1.log.gz"), 3600);
        // The game was started and closed in the menu: nothing in its newest log.
        fs::write(logs.join("latest.log"), "[1] [main/INFO]: Loading Minecraft 1.21.1\n").unwrap();
        let (marker, at) = markers.last(game.path()).unwrap();
        assert_eq!(marker, Marker::Server { host: "old.example".into(), port: 25565 });
        let gz_time = modified_ms(&fs::metadata(logs.join("2026-09-01-1.log.gz")).unwrap());
        assert_eq!(at, gz_time, "timed by the log it was found in");
        fs::write(
            logs.join("latest.log"),
            "[9] [Server thread/INFO]: Starting integrated minecraft server version 1.21.1\n",
        )
        .unwrap();
        assert_eq!(markers.last(game.path()).unwrap().0, Marker::Singleplayer, "a changed log is read again");
    }
}
