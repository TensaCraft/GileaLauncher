//! Excerpts of files for a report: a big file gives its head and its tail, read
//! without loading the rest; the whole log stays within a megabyte.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use launcher_shared::branding::APP_NAME;

/// The most a file gives to a report.
pub const FILE_CAP: usize = 256 * 1024;
/// The most a report's log holds.
pub const LOG_CAP: usize = 1024 * 1024;

fn read_part(file: &mut File, from: SeekFrom, len: usize) -> std::io::Result<Vec<u8>> {
    file.seek(from)?;
    let mut buf = Vec::with_capacity(len);
    file.take(len as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_excerpt(path: &Path, cap: usize) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= cap as u64 {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        return Ok(String::from_utf8_lossy(&buf).into_owned());
    }
    let half = cap / 2;
    let head = read_part(&mut file, SeekFrom::Start(0), half)?;
    let tail = read_part(&mut file, SeekFrom::End(-(half as i64)), half)?;
    Ok(format!(
        "{}\n--- middle of file omitted by {APP_NAME} ---\n{}",
        String::from_utf8_lossy(&head),
        String::from_utf8_lossy(&tail)
    ))
}

/// File `path` as a report shows it: whole, or its head and tail within `cap` bytes; a file that
/// cannot be read says why.
pub fn excerpt(path: &Path, cap: usize) -> String {
    read_excerpt(path, cap).unwrap_or_else(|e| format!("Unable to read {}: {e}", path.display()))
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `text` within `cap` bytes: its head and tail around a mark naming `what` was cut.
pub fn cap(text: &str, cap: usize, what: &str) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let half = cap / 2;
    let head = floor_boundary(text, half);
    let tail = floor_boundary(text, text.len() - half);
    format!("{}\n--- middle of {what} omitted by {APP_NAME} ---\n{}", &text[..head], &text[tail..])
}

/// A report's log: each attachment once, then the launcher's own log, within [`LOG_CAP`].
pub fn combined(attachments: &[PathBuf], app_log: Option<&Path>) -> String {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    for path in attachments {
        let same = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if seen.contains(&same) {
            continue;
        }
        seen.push(same);
        let name =
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into());
        parts.push(format!("--- diagnostic file: {name} ---\n{}", excerpt(path, FILE_CAP)));
    }
    if let Some(app_log) = app_log {
        parts.push(format!("--- launcher app.log ---\n{}", excerpt(app_log, FILE_CAP)));
    }
    cap(&parts.join("\n\n"), LOG_CAP, "combined log")
}
