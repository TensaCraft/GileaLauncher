//! The launcher log for the in-app viewer: `app.log` and its rotated copies, split into records by
//! the `LineFormat` header (`2026-09-27 12:00:00 INFO     message`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use launcher_shared::{LogEntry, LogLevel, LogView};

use crate::logging::LOG_BACKUPS;

/// The most records the viewer receives.
pub const MAX_ENTRIES: usize = 5000;

static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (ERROR|WARN|INFO|DEBUG|TRACE) +(.*)$")
        .expect("valid log header regex")
});

fn level(word: &str) -> LogLevel {
    match word {
        "ERROR" => LogLevel::Error,
        "WARN" => LogLevel::Warn,
        "INFO" => LogLevel::Info,
        "DEBUG" => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

/// Records in `text`; a line without a header continues the record before it.
pub fn parse(text: &str) -> Vec<LogEntry> {
    let mut entries = Vec::new();
    push_lines(&mut entries, text);
    entries
}

fn push_lines(entries: &mut Vec<LogEntry>, text: &str) {
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Some(c) = HEADER.captures(line) {
            entries.push(LogEntry {
                time: c[1].to_string(),
                level: Some(level(&c[2])),
                message: c[3].to_string(),
            });
        } else if let Some(last) = entries.last_mut() {
            last.message.push('\n');
            last.message.push_str(line);
        } else {
            entries.push(LogEntry { time: String::new(), level: None, message: line.to_string() });
        }
    }
}

/// The launcher log with its rotated copies, keeping the newest [`MAX_ENTRIES`] records.
pub fn read(file: &Path) -> io::Result<LogView> {
    read_with(file, LOG_BACKUPS, MAX_ENTRIES)
}

/// `file` and up to `backups` rotated copies (`.1` is the newest of them), oldest first, keeping
/// the newest `limit` records. A missing file reads as empty.
pub fn read_with(file: &Path, backups: usize, limit: usize) -> io::Result<LogView> {
    let mut entries = Vec::new();
    for n in (1..=backups).rev() {
        push_file(&mut entries, &rotated(file, n))?;
    }
    push_file(&mut entries, file)?;
    let skipped = entries.len().saturating_sub(limit);
    entries.drain(..skipped);
    Ok(LogView { file: file.to_string_lossy().into_owned(), entries, skipped })
}

fn rotated(file: &Path, n: usize) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{n}"));
    file.with_file_name(name)
}

/// `fs::read` shares the file for reading, writing and deletion, so the logger keeps writing and
/// rotating while the viewer reads.
fn push_file(entries: &mut Vec<LogEntry>, path: &Path) -> io::Result<()> {
    match fs::read(path) {
        Ok(bytes) => {
            push_lines(entries, &String::from_utf8_lossy(&bytes));
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(time: &str, level: Option<LogLevel>, message: &str) -> LogEntry {
        LogEntry { time: time.into(), level, message: message.into() }
    }

    #[test]
    fn records_take_their_level_and_continuation_lines() {
        let text = concat!(
            "2026-09-27 12:00:00 INFO     Logging initialised\r\n",
            "2026-09-27 12:00:01 WARN     slow disk\n",
            "2026-09-27 12:00:02 ERROR    launch failed: exit 1\n",
            "Caused by:\n",
            "    0: java not found\n",
            "\n",
            "2026-09-27 12:00:03 DEBUG    probe\n",
            "2026-09-27 12:00:04 TRACE    tick\n",
        );
        assert_eq!(
            parse(text),
            vec![
                entry("2026-09-27 12:00:00", Some(LogLevel::Info), "Logging initialised"),
                entry("2026-09-27 12:00:01", Some(LogLevel::Warn), "slow disk"),
                entry(
                    "2026-09-27 12:00:02",
                    Some(LogLevel::Error),
                    "launch failed: exit 1\nCaused by:\n    0: java not found"
                ),
                entry("2026-09-27 12:00:03", Some(LogLevel::Debug), "probe"),
                entry("2026-09-27 12:00:04", Some(LogLevel::Trace), "tick"),
            ]
        );
    }

    #[test]
    fn text_before_any_record_has_no_level() {
        assert_eq!(
            parse("thread 'main' panicked\n2026-09-27 12:00:00 INFO     up\n"),
            vec![
                entry("", None, "thread 'main' panicked"),
                entry("2026-09-27 12:00:00", Some(LogLevel::Info), "up"),
            ]
        );
    }

    #[test]
    fn rotated_copies_come_first_oldest_to_newest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.log");
        std::fs::write(dir.path().join("app.log.2"), "2026-09-27 10:00:00 INFO     oldest\n").unwrap();
        std::fs::write(dir.path().join("app.log.1"), "2026-09-27 11:00:00 INFO     older\n").unwrap();
        std::fs::write(&file, b"2026-09-27 12:00:00 WARN     now \xff\n").unwrap();
        let view = read_with(&file, 3, 10).unwrap();
        let messages: Vec<&str> = view.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["oldest", "older", "now \u{fffd}"]);
        assert_eq!(view.skipped, 0);
        assert_eq!(view.file, file.to_string_lossy());
    }

    #[test]
    fn only_the_newest_records_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.log");
        let text: String = (0..7).map(|i| format!("2026-09-27 12:00:0{i} INFO     line {i}\n")).collect();
        std::fs::write(&file, text).unwrap();
        let view = read_with(&file, 0, 4).unwrap();
        assert_eq!(view.skipped, 3);
        assert_eq!(view.entries.len(), 4);
        assert_eq!(view.entries[0].message, "line 3");
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let view = read(&dir.path().join("app.log")).unwrap();
        assert!(view.entries.is_empty());
        assert_eq!(view.skipped, 0);
    }
}
