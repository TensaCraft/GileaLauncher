//! The files a crash leaves: the newest crash report, `latest.log`, the
//! launcher's `logs/launch.log` and the newest `hs_err_*.log` — only those written since the
//! launch — each read whole up to 4 MiB, else as its head and tail.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::Case;
use launcher_core::launch::process::{FRESHNESS, LAUNCH_LOG};

/// What is read of one file at most.
pub const ARTIFACT_LIMIT: u64 = 4 * 1024 * 1024;

/// The files a crash since `launched_at` left in `game_dir`, the most telling first.
pub fn fresh_files(game_dir: &Path, launched_at: SystemTime) -> Vec<PathBuf> {
    let since = launched_at.checked_sub(FRESHNESS).unwrap_or(launched_at);
    let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
    let fresh = |path: &Path| path.is_file() && modified(path).is_some_and(|t| t >= since);
    let newest = |dir: &Path, prefix: &str| {
        fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)) && fresh(p))
            .max_by_key(|p| modified(p))
    };
    let candidates = [
        newest(&game_dir.join("crash-reports"), ""),
        Some(game_dir.join("logs").join("latest.log")).filter(|p| fresh(p)),
        Some(game_dir.join("logs").join(LAUNCH_LOG)).filter(|p| fresh(p)),
        newest(game_dir, "hs_err_"),
    ];
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .flatten()
        .filter(|p| seen.insert(fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .collect()
}

/// The file's text, whole up to `ARTIFACT_LIMIT`, else its first third and last two thirds of
/// that; bytes that are no UTF-8 are replaced.
pub fn read_artifact(path: &Path) -> String {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len <= ARTIFACT_LIMIT {
            let mut all = Vec::new();
            file.read_to_end(&mut all)?;
            return Ok(all);
        }
        let head_len = ARTIFACT_LIMIT / 3;
        let tail_len = ARTIFACT_LIMIT - head_len;
        let mut head = vec![0u8; head_len as usize];
        file.read_exact(&mut head)?;
        file.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail)?;
        let mut out = head;
        out.extend_from_slice(b"\n... diagnostic log truncated by the launcher ...\n");
        out.extend_from_slice(&tail);
        Ok(out)
    };
    match read() {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => format!("Unable to read {}: {e}", path.display()),
    }
}

/// What the detectors read of a crash since `launched_at`.
pub fn case_of(game_dir: &Path, launched_at: SystemTime, managed: bool) -> Case {
    let files = fresh_files(game_dir, launched_at);
    let raw = files.iter().map(|p| read_artifact(p)).collect::<Vec<_>>().join("\n");
    Case { text: raw.to_lowercase(), raw, files, managed }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn a_huge_log_is_read_as_its_head_and_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("latest.log");
        let mut body = "HEAD-LINE\n".to_string();
        body.push_str(&"x".repeat(6 * 1024 * 1024));
        body.push_str("\nTAIL-LINE\n");
        fs::write(&path, body.as_bytes()).unwrap();
        let text = read_artifact(&path);
        assert!(text.starts_with("HEAD-LINE") && text.trim_end().ends_with("TAIL-LINE"));
        assert!(text.contains("diagnostic log truncated by"));
        assert!(text.len() <= 4 * 1024 * 1024 + 100);
        fs::write(&path, [0xffu8, 0xfe, b'o', b'k']).unwrap();
        assert!(read_artifact(&path).ends_with("ok"), "invalid UTF-8 is replaced");
    }

    #[test]
    fn only_files_since_the_launch_count() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path();
        fs::create_dir_all(game.join("crash-reports")).unwrap();
        fs::create_dir_all(game.join("logs")).unwrap();
        let old = game.join("crash-reports/crash-old.txt");
        fs::write(&old, "old").unwrap();
        let launched = SystemTime::now() + Duration::from_secs(10);
        let stamp = |path: &std::path::Path, at: SystemTime| {
            fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(at).unwrap();
        };
        stamp(&old, launched - Duration::from_secs(60));
        for name in ["crash-reports/crash-new.txt", "logs/latest.log", "logs/launch.log", "hs_err_pid42.log"]
        {
            fs::write(game.join(name), name).unwrap();
            stamp(&game.join(name), launched);
        }
        let names: Vec<String> = fresh_files(game, launched)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["crash-new.txt", "latest.log", "launch.log", "hs_err_pid42.log"]);
        let case = case_of(game, launched, false);
        assert!(case.text.contains("logs/latest.log") && !case.text.contains("old"));
    }
}
