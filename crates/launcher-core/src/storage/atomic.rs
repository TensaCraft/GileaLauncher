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
        fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn atomic_write_text(path: &Path, text: &str) -> io::Result<()> {
    atomic_write(path, text.as_bytes())
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
