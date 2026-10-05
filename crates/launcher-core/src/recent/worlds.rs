//! A build's worlds as their `saves/<folder>/level.dat` (gzip NBT) and `icon.png` describe them.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

/// What Home shows of a world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    pub folder: String,
    pub name: String,
    pub last_played_ms: u64,
    /// `survival`, `creative`, `adventure` or `spectator`.
    pub mode: String,
    /// `peaceful`, `easy`, `normal` or `hard`; empty when the world does not say.
    pub difficulty: String,
    pub hardcore: bool,
    /// Its `icon.png` as a `data:` URL, once asked for (`with_icon`).
    pub icon: Option<String>,
}

impl World {
    /// The world with its icon, read from the game folder `game`.
    pub fn with_icon(mut self, game: &Path) -> World {
        let path = game.join("saves").join(&self.folder).join("icon.png");
        self.icon = fs::metadata(&path)
            .ok()
            .filter(|m| m.is_file() && m.len() <= ICON_MAX)
            .and_then(|_| fs::read(&path).ok())
            .filter(|png| png.starts_with(b"\x89PNG"))
            .map(|png| format!("data:image/png;base64,{}", STANDARD.encode(png)));
        self
    }
}

#[derive(Deserialize)]
struct Level {
    #[serde(rename = "Data")]
    data: LevelData,
}

#[derive(Deserialize)]
struct LevelData {
    #[serde(rename = "LevelName", default)]
    name: String,
    #[serde(rename = "LastPlayed", default)]
    last_played: i64,
    #[serde(rename = "GameType", default)]
    game_type: i32,
    #[serde(rename = "Difficulty")]
    difficulty: Option<i8>,
    #[serde(default)]
    hardcore: i8,
}

/// The largest `level.dat` read (a world's is a few kilobytes) and the largest icon shown.
const LEVEL_MAX: u64 = 16 * 1024 * 1024;
/// The most a `level.dat` may unpack to.
const LEVEL_UNPACKED_MAX: u64 = 64 * 1024 * 1024;
const ICON_MAX: u64 = 512 * 1024;

fn mode(game_type: i32) -> &'static str {
    match game_type {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    }
}

fn difficulty(level: Option<i8>) -> &'static str {
    match level {
        Some(0) => "peaceful",
        Some(1) => "easy",
        Some(2) => "normal",
        Some(3) => "hard",
        _ => "",
    }
}

/// The world in `dir`, when its `level.dat` reads (without its icon).
pub fn world(dir: &Path) -> Option<World> {
    let level = dir.join("level.dat");
    if fs::metadata(&level).ok()?.len() > LEVEL_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(File::open(&level).ok()?)
        .take(LEVEL_UNPACKED_MAX + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > LEVEL_UNPACKED_MAX {
        return None;
    }
    let data = fastnbt::from_bytes::<Level>(&bytes).ok()?.data;
    let folder = dir.file_name()?.to_string_lossy().into_owned();
    Some(World {
        name: if data.name.trim().is_empty() { folder.clone() } else { data.name },
        folder,
        last_played_ms: u64::try_from(data.last_played).unwrap_or(0),
        mode: mode(data.game_type).to_string(),
        difficulty: difficulty(data.difficulty).to_string(),
        hardcore: data.hardcore != 0,
        icon: None,
    })
}

/// The worlds of the game folder `game` (those whose `level.dat` reads), most recently played first.
pub fn worlds(game: &Path) -> Vec<World> {
    let Ok(entries) = fs::read_dir(game.join("saves")) else { return Vec::new() };
    let mut found: Vec<World> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| world(&e.path()))
        .collect();
    found.sort_by_key(|w| std::cmp::Reverse(w.last_played_ms));
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct TestLevel {
        #[serde(rename = "Data")]
        data: TestData,
    }

    #[derive(Serialize)]
    struct TestData {
        #[serde(rename = "LevelName")]
        name: String,
        #[serde(rename = "LastPlayed")]
        last_played: i64,
        #[serde(rename = "GameType")]
        game_type: i32,
        #[serde(rename = "Difficulty")]
        difficulty: i8,
        hardcore: i8,
    }

    /// A world `folder` in `game` as the game writes it.
    pub(crate) fn write_world(game: &Path, folder: &str, name: &str, last_played: i64, game_type: i32) {
        let dir = game.join("saves").join(folder);
        fs::create_dir_all(&dir).unwrap();
        let level = TestLevel {
            data: TestData { name: name.into(), last_played, game_type, difficulty: 3, hardcore: 0 },
        };
        let nbt = fastnbt::to_bytes(&level).unwrap();
        let mut gz = flate2::write::GzEncoder::new(
            File::create(dir.join("level.dat")).unwrap(),
            flate2::Compression::fast(),
        );
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap();
    }

    #[test]
    fn worlds_are_read_from_their_level_and_newest_come_first() {
        let game = tempfile::tempdir().unwrap();
        write_world(game.path(), "Old", "Старий", 1_000, 0);
        write_world(game.path(), "Build", "Будівництво", 5_000, 1);
        fs::write(game.path().join("saves/Build/icon.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
        fs::create_dir_all(game.path().join("saves/Broken")).unwrap();
        fs::write(game.path().join("saves/Broken/level.dat"), b"not nbt").unwrap();
        let found = worlds(game.path());
        assert_eq!(found.len(), 2, "a world whose level does not read is left out");
        assert_eq!(
            (
                found[0].folder.as_str(),
                found[0].name.as_str(),
                found[0].mode.as_str(),
                found[0].difficulty.as_str()
            ),
            ("Build", "Будівництво", "creative", "hard")
        );
        assert!(found.iter().all(|w| w.icon.is_none()), "an icon is read for the world shown only");
        assert_eq!((found[1].folder.as_str(), found[1].last_played_ms), ("Old", 1_000));
        assert!(worlds(&game.path().join("none")).is_empty());
        let shown = found[0].clone().with_icon(game.path());
        assert!(shown.icon.as_deref().is_some_and(|i| i.starts_with("data:image/png;base64,")));
        assert!(found[1].clone().with_icon(game.path()).icon.is_none(), "it has none");
    }

    #[test]
    fn a_level_that_unpacks_past_its_limit_is_no_world() {
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join("saves/Huge");
        fs::create_dir_all(&dir).unwrap();
        let mut gz = flate2::write::GzEncoder::new(
            File::create(dir.join("level.dat")).unwrap(),
            flate2::Compression::best(),
        );
        let zeros = vec![0u8; 1024 * 1024];
        for _ in 0..(LEVEL_UNPACKED_MAX / zeros.len() as u64 + 1) {
            gz.write_all(&zeros).unwrap();
        }
        gz.finish().unwrap();
        assert!(world(&dir).is_none());
    }
}
