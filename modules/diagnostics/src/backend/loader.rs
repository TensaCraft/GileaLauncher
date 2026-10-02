//! A mod loader that gave up: Fabric and Quilt print their fatal error, show their own error
//! window and wait until it is closed, so the game cannot go on.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use launcher_core::launch::process::{FRESHNESS, LAUNCH_LOG};

/// What Fabric and Quilt write just before they show their error window: their handler exits
/// once the window is closed.
const LOADER_GAVE_UP: [&str; 2] =
    ["net.fabricmc.loader.impl.formattedexception", "org.quiltmc.loader.impl.formattedexception"];

/// Reads what the game adds to `logs/launch.log` since the last look.
pub(crate) struct LaunchLog {
    path: PathBuf,
    since: SystemTime,
    read: u64,
    /// The end of what was read, lower case, for a marker split between two looks.
    carry: String,
}

impl LaunchLog {
    pub(crate) fn new(game_dir: &Path, launched_at: SystemTime) -> LaunchLog {
        LaunchLog {
            path: game_dir.join("logs").join(LAUNCH_LOG),
            since: launched_at.checked_sub(FRESHNESS).unwrap_or(launched_at),
            read: 0,
            carry: String::new(),
        }
    }

    /// Whether the lines added since the last look say the mod loader gave up. A log left from
    /// an earlier launch says nothing.
    pub(crate) fn loader_gave_up(&mut self) -> bool {
        let Ok(mut file) = File::open(&self.path) else { return false };
        let Ok(meta) = file.metadata() else { return false };
        if meta.modified().is_ok_and(|m| m < self.since) || meta.len() == self.read {
            return false;
        }
        if meta.len() < self.read {
            self.read = 0;
        }
        let mut added = Vec::new();
        if file.seek(SeekFrom::Start(self.read)).is_err()
            || (&mut file).take(meta.len() - self.read).read_to_end(&mut added).is_err()
        {
            return false;
        }
        self.read += added.len() as u64;
        let text = std::mem::take(&mut self.carry) + &String::from_utf8_lossy(&added).to_lowercase();
        let gave_up = LOADER_GAVE_UP.iter().any(|m| text.contains(m));
        let keep = text.char_indices().rev().nth(63).map_or(0, |(i, _)| i);
        self.carry = text[keep..].to_string();
        gave_up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_written_in_two_goes_is_still_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join(LAUNCH_LOG);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut log = LaunchLog::new(dir.path(), SystemTime::now());
        std::fs::write(&path, "header\nnet.fabricmc.loader.impl.Format").unwrap();
        assert!(!log.loader_gave_up());
        std::fs::write(&path, "header\nnet.fabricmc.loader.impl.FormattedException: gave up\n").unwrap();
        assert!(log.loader_gave_up());
        assert!(!log.loader_gave_up(), "nothing new");
    }
}
