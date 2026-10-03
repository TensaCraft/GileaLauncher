//! Crash-safe file replacement: temp file in the same directory, fsync, rename.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    let tmp = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));

    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(path)
            && meta.is_file()
        {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        rename_retrying(&tmp, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn atomic_write_text(path: &Path, text: &str) -> io::Result<()> {
    atomic_write(path, text.as_bytes())
}

/// Renames `from` to `to`, trying again for a moment while Windows refuses because another program
/// holds one of them (an antivirus scanning a file just written: "access denied", "in use").
pub fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 12;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(150);
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Err(e) if attempt < ATTEMPTS && held_elsewhere(&e) => {
                std::thread::sleep(PAUSE);
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Windows' "access denied", "sharing violation" and "lock violation": another program has it.
fn held_elsewhere(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    /// An antivirus scans a file just written and holds it for a moment: it still takes its place.
    #[cfg(windows)]
    #[test]
    fn a_rename_waits_out_a_file_held_for_a_moment() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("lib.jar.part"), dir.path().join("lib.jar"));
        fs::write(&from, b"jar").unwrap();
        let held = fs::OpenOptions::new().read(true).share_mode(0x1).open(&from).unwrap();
        let scanner = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            drop(held);
        });
        rename_retrying(&from, &to).unwrap();
        scanner.join().unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"jar");
        assert!(!from.exists());
    }

    #[test]
    fn writes_and_replaces_content_without_leaving_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.json");
        atomic_write_text(&target, "first").unwrap();
        atomic_write_text(&target, "second").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "second");
        assert!(tmp_files(dir.path()).is_empty());
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a").join("b").join("file.txt");
        atomic_write(&target, b"x").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"x");
    }

    #[test]
    fn atomic_write_unicode_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Іван Петренко").join("налаштування.json");
        atomic_write_text(&target, "{}").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
    }

    #[test]
    fn failed_write_keeps_original_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        // Target is an existing directory: rename over it must fail.
        let target = dir.path().join("occupied");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), "keep").unwrap();
        assert!(atomic_write_text(&target, "data").is_err());
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
        assert!(tmp_files(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("secret.key");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write_text(&target, "new").unwrap();
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
