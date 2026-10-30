//! Files the player drops on the window. The page reads them (the window takes no file drop of its
//! own: the build list's drag to reorder needs the page's), so each comes as its name and bytes;
//! it is kept under the cache's `drops/` until the step it was dropped for has used it, then goes
//! with the next day's sweep.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use launcher_shared::{AppError, AppResult, ErrorCode};

/// The folder under the cache that holds dropped files.
pub const DROPS_DIR: &str = "drops";

/// How long a dropped file is kept: it is used at once, so a day is plenty.
const KEPT: Duration = Duration::from_secs(24 * 60 * 60);

/// The longest file name a drop keeps (bytes): Windows takes 255 characters in a name.
const NAME_MAX: usize = 255;

/// The file name of `raw` (what the page said the file was called): its last part, without any
/// folder. Never `.`, `..`, empty, a Windows device name, nor one Windows would read otherwise (a
/// stream after `:`, a dot or space it drops at the end) or refuse (too long).
pub fn dropped_name(raw: &str) -> Option<String> {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim_start();
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_lowercase();
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > NAME_MAX
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| c.is_control() || c == ':')
        || crate::content::files::RESERVED.contains(&stem.as_str());
    (!bad).then(|| name.to_string())
}

/// Keeps `bytes` as the dropped file `name` under `cache/drops/` and gives its path; drops older
/// than a day go first.
pub fn stage(cache_dir: &Path, name: &str, bytes: &[u8]) -> AppResult<PathBuf> {
    let name = dropped_name(name)
        .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, format!("not a file name: {name:?}")))?;
    if bytes.is_empty() {
        return Err(
            AppError::new(ErrorCode::InvalidInput, format!("{name} is empty")).with_param("name", name)
        );
    }
    let root = cache_dir.join(DROPS_DIR);
    sweep(&root, SystemTime::now());
    let dir = root.join(uuid::Uuid::new_v4().simple().to_string());
    let path = dir.join(&name);
    let io = |e: std::io::Error| {
        AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
            .with_param("path", path.to_string_lossy())
    };
    fs::create_dir_all(&dir).map_err(io)?;
    if let Err(e) = fs::write(&path, bytes) {
        let _ = fs::remove_dir_all(&dir);
        return Err(io(e));
    }
    Ok(path)
}

/// Removes the drops under `root` older than a day at `now` (a drop is a folder with its one file:
/// the file's time is the drop's, the folder's own when it has none).
fn sweep(root: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let newest = fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|file| file.metadata().and_then(|m| m.modified()).ok())
            .max()
            .or_else(|| entry.metadata().and_then(|m| m.modified()).ok());
        if newest.and_then(|at| now.duration_since(at).ok()).is_some_and(|age| age > KEPT) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_name_is_the_file_name_alone() {
        assert_eq!(dropped_name("sodium-0.6.jar").as_deref(), Some("sodium-0.6.jar"));
        assert_eq!(dropped_name("C:\\Users\\me\\skin.png").as_deref(), Some("skin.png"));
        assert_eq!(dropped_name("../../evil.jar").as_deref(), Some("evil.jar"));
        for bad in ["", "  ", ".", "..", "dir/", "a\u{0}b.jar"] {
            assert_eq!(dropped_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_name_windows_would_change_or_misread_is_refused() {
        let long = format!("{}.jar", "a".repeat(252));
        for bad in ["a.jar:stream", "x.jar.", "x.jar ", "con.jar", "NUL", "lpt1.zip", long.as_str()] {
            assert_eq!(dropped_name(bad), None, "{bad:?}");
        }
        assert_eq!(dropped_name(&format!("{}.jar", "a".repeat(251))).map(|n| n.len()), Some(255));
        assert_eq!(dropped_name("Мій скін (2).png").as_deref(), Some("Мій скін (2).png"));
        assert_eq!(dropped_name("console.jar").as_deref(), Some("console.jar"), "only the device names");
    }

    #[test]
    fn an_empty_drop_is_refused_and_leaves_nothing() {
        let cache = tempfile::tempdir().unwrap();
        assert_eq!(stage(cache.path(), "a.jar", b"").unwrap_err().code, ErrorCode::InvalidInput);
        let root = cache.path().join(DROPS_DIR);
        assert!(!root.exists() || fs::read_dir(&root).unwrap().next().is_none(), "no folder is left");
    }

    #[test]
    fn the_same_name_twice_is_two_files() {
        let cache = tempfile::tempdir().unwrap();
        let a = stage(cache.path(), "a.jar", b"one").unwrap();
        let b = stage(cache.path(), "a.jar", b"two").unwrap();
        assert_ne!(a, b);
        assert_eq!((fs::read(&a).unwrap(), fs::read(&b).unwrap()), (b"one".to_vec(), b"two".to_vec()));
    }

    #[test]
    fn an_empty_drop_folder_goes_with_the_sweep() {
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path().join(DROPS_DIR);
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        sweep(&root, SystemTime::now());
        assert!(empty.exists(), "a fresh one may be a drop on its way");
        sweep(&root, SystemTime::now() + KEPT + Duration::from_secs(60));
        assert!(!empty.exists(), "a day-old empty one goes");
    }

    #[test]
    fn a_drop_is_kept_under_the_cache_and_old_ones_go() {
        let cache = tempfile::tempdir().unwrap();
        let old = cache.path().join(DROPS_DIR).join("old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("x.jar"), b"x").unwrap();
        let day_ago = SystemTime::now() - KEPT - Duration::from_secs(60);
        fs::File::options().write(true).open(old.join("x.jar")).unwrap().set_modified(day_ago).unwrap();

        let path = stage(cache.path(), "../pack.mrpack", b"pack").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"pack");
        assert_eq!(path.file_name().unwrap(), "pack.mrpack");
        assert!(path.starts_with(cache.path().join(DROPS_DIR)), "{}", path.display());
        assert!(!old.exists(), "a day-old drop is swept");
        assert_eq!(stage(cache.path(), "..", b"x").unwrap_err().code, ErrorCode::InvalidInput);
    }
}
