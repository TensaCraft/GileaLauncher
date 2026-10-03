//! Storage preflight: every target folder is creatable and writable, and each volume has room for
//! the bytes about to land on it plus a 32 MiB reserve.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};

use crate::lock::path_key;

pub const STORAGE_RESERVE: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRequest {
    pub dir: PathBuf,
    pub bytes: u64,
    /// What needs the space ("libraries", a file name) — for the message.
    pub label: String,
}

/// `1.5 KiB`, `5.0 GiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(unix)]
fn volume_key(dir: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(dir).map(|m| m.dev().to_string()).unwrap_or_else(|_| dir.to_string_lossy().into_owned())
}

/// The drive or UNC share.
#[cfg(not(unix))]
fn volume_key(dir: &Path) -> String {
    std::path::absolute(dir)
        .ok()
        .and_then(|p| p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()))
        .unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

/// Where writing is probed: a folder that exists as itself, a folder still to be made through
/// the nearest one that exists (the one it is made in) — each once; of folders side by side that
/// exist (an install's ~256 `assets/objects/xx`), the first stands for the rest.
fn probe_targets(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    let (mut seen, mut sides) = (HashSet::new(), HashSet::new());
    for dir in dirs {
        let mut target = dir.as_path();
        while !target.is_dir() {
            match target.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => target = parent,
                _ => break,
            }
        }
        let beside_one =
            target == dir.as_path() && target.parent().is_some_and(|p| !sides.insert(path_key(p)));
        if !beside_one && seen.insert(path_key(target)) {
            targets.push(target.to_path_buf());
        }
    }
    targets
}

/// A file can be made and removed in `dir` (not flushed to disk: dozens of folders are tested
/// before a download).
fn creatable(dir: &Path) -> io::Result<()> {
    let probe = dir.join(format!(".launcher-write-test-{}.tmp", uuid::Uuid::new_v4().simple()));
    let made =
        fs::OpenOptions::new().write(true).create_new(true).open(&probe).and_then(|mut f| f.write_all(&[0]));
    let _ = fs::remove_file(&probe);
    made
}

/// A volume's share of the requests: a folder on it, the bytes, and the labels in first-seen order.
struct Volume {
    dir: PathBuf,
    bytes: u64,
    labels: Vec<String>,
    named: HashSet<String>,
}

pub fn preflight(requests: &[SpaceRequest]) -> AppResult<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut known = HashSet::new();
    for request in requests {
        if known.insert(path_key(&request.dir)) {
            dirs.push(request.dir.clone());
        }
    }
    // Chosen before anything is made: a folder made here is probed through its parent.
    let targets = probe_targets(&dirs);
    for dir in &dirs {
        fs::create_dir_all(dir).map_err(|e| {
            AppError::new(ErrorCode::DirectoryCreateFailed, e.to_string())
                .with_param("path", dir.to_string_lossy())
        })?;
    }
    for target in &targets {
        creatable(target).map_err(|e| {
            AppError::new(ErrorCode::InvalidDirectoryPath, e.to_string())
                .with_param("path", target.to_string_lossy())
        })?;
    }
    let mut volumes: BTreeMap<String, Volume> = BTreeMap::new();
    for request in requests {
        let dir = &request.dir;
        let volume = volumes.entry(volume_key(dir)).or_insert_with(|| Volume {
            dir: dir.clone(),
            bytes: 0,
            labels: Vec::new(),
            named: HashSet::new(),
        });
        volume.bytes = volume.bytes.saturating_add(request.bytes);
        if volume.named.insert(request.label.clone()) {
            volume.labels.push(request.label.clone());
        }
    }
    for Volume { dir, bytes, labels, .. } in volumes.into_values() {
        let available =
            fs4::available_space(&dir).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
        if available < bytes.saturating_add(STORAGE_RESERVE) {
            return Err(AppError::new(
                ErrorCode::NotEnoughSpace,
                format!(
                    "Not enough free space for {} on {}: requires {}, available {}",
                    labels.join(", "),
                    dir.display(),
                    format_size(bytes),
                    format_size(available)
                ),
            )
            .with_param("path", dir.to_string_lossy())
            .with_param("required", format_size(bytes))
            .with_param("available", format_size(available)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_original() {
        assert_eq!(format_size(0), "0.0 B");
        assert_eq!(format_size(1023), "1023.0 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(format_size(3 * 1024u64.pow(4)), "3.0 TiB");
    }

    #[test]
    fn small_requests_pass_and_create_folders() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("libraries").join("deep");
        preflight(&[SpaceRequest { dir: target.clone(), bytes: 1024, label: "libraries".into() }]).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn a_huge_request_is_refused_with_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let requests = [
            SpaceRequest { dir: dir.path().join("a"), bytes: u64::MAX / 4, label: "assets".into() },
            SpaceRequest { dir: dir.path().join("b"), bytes: 1, label: "client".into() },
        ];
        let err = preflight(&requests).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotEnoughSpace);
        assert!(err.params["required"].ends_with("iB"), "{:?}", err.params);
        assert!(err.params.contains_key("available") && err.params.contains_key("path"));
        assert!(err.detail.contains("assets"), "{}", err.detail);
    }

    #[test]
    fn new_folders_are_probed_through_the_folder_they_are_made_in() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        fs::create_dir(&old).unwrap();
        let dirs = vec![
            old.clone(),
            old.clone(),
            root.path().join("new").join("a"),
            root.path().join("new").join("b"),
        ];
        let mut probed = probe_targets(&dirs);
        probed.sort();
        let mut expected = vec![root.path().to_path_buf(), old];
        expected.sort();
        assert_eq!(probed, expected);
    }

    #[test]
    fn folders_side_by_side_are_probed_once() {
        // A second version's install writes into ~256 existing assets/objects/xx folders: one
        // write test for folders side by side, not one each (each with a flush to disk).
        let root = tempfile::tempdir().unwrap();
        let objects = root.path().join("objects");
        let dirs: Vec<PathBuf> = (0..256).map(|i| objects.join(format!("{i:02x}"))).collect();
        for dir in &dirs {
            fs::create_dir_all(dir).unwrap();
        }
        let probed = probe_targets(&dirs);
        assert_eq!(probed.len(), 1);
        assert_eq!(probed[0].parent(), Some(objects.as_path()));
        assert!(
            preflight(
                &dirs
                    .iter()
                    .map(|d| SpaceRequest { dir: d.clone(), bytes: 1, label: "x".into() })
                    .collect::<Vec<_>>()
            )
            .is_ok()
        );
    }

    #[test]
    fn an_unusable_folder_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupied");
        std::fs::write(&file, b"x").unwrap();
        let err =
            preflight(&[SpaceRequest { dir: file.join("sub"), bytes: 1, label: "x".into() }]).unwrap_err();
        assert_eq!(err.code, ErrorCode::DirectoryCreateFailed);
    }
}
