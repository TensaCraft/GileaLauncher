//! Files a provider may not hand to other apps (on CurseForge, their authors' choice): the user
//! downloads them by hand, and the launcher finds the copies in their Downloads folder — by
//! size, then SHA-1 — whatever name the browser gave them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};

use crate::net::downloader::{ExpectedHash, HashKind, hash_file};

/// A file to find: its usual name, its size and its SHA-1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted<'a> {
    pub name: &'a str,
    pub size: u64,
    pub sha1: &'a str,
}

/// The user's Downloads folder.
pub fn downloads_dir() -> Option<PathBuf> {
    dirs::download_dir()
}

/// The endings browsers give a file while they still download it (Firefox `.part`, Chrome
/// `.crdownload`, Safari `.download`, Opera `.opdownload`…): renamed once done, so never a copy.
const DOWNLOADING: [&str; 6] = ["part", "crdownload", "download", "partial", "opdownload", "tmp"];

fn still_downloading(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DOWNLOADING.iter().any(|d| e.eq_ignore_ascii_case(d)))
}

/// A copy of each of `wanted` in `dir` (not its subfolders), when there is one: the file of its
/// name first, then any of its size whose SHA-1 is its.
pub fn find_copies(dir: &Path, wanted: &[Wanted<'_>]) -> Vec<Option<PathBuf>> {
    let sizes: Vec<(PathBuf, u64)> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let meta = e.metadata().ok().filter(|m| m.is_file())?;
                    Some((e.path(), meta.len())).filter(|(path, _)| !still_downloading(path))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut digests: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut sha1_of = |path: &Path| {
        digests.entry(path.to_path_buf()).or_insert_with(|| hash_file(path, HashKind::Sha1).ok()).clone()
    };
    wanted
        .iter()
        .map(|file| {
            let named =
                (!file.name.is_empty() && !file.name.contains(['/', '\\'])).then(|| dir.join(file.name));
            let mut candidates: Vec<&PathBuf> =
                sizes.iter().filter(|(_, size)| *size == file.size).map(|(path, _)| path).collect();
            // The file of its own name before the others of its size.
            candidates.sort_by_key(|path| Some(*path) != named.as_ref());
            candidates
                .into_iter()
                .find(|path| sha1_of(path).is_some_and(|h| h.eq_ignore_ascii_case(file.sha1)))
                .cloned()
        })
        .collect()
}

/// Copies the user's copy `from` to `dest`; a copy that is no longer the file (`size`, `hash`)
/// is refused and leaves nothing there. `name` names the file in the error.
pub fn take_copy(from: &Path, dest: &Path, size: u64, hash: &ExpectedHash, name: &str) -> AppResult<()> {
    // Renamed since it was found (a browser finishing its download): the same file under its
    // new name in the same folder.
    let renamed;
    let from = if from.is_file() {
        from
    } else {
        renamed = from.parent().and_then(|dir| same_file_in(dir, size, hash)).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("{} is gone", from.display())).with_param("name", name)
        })?;
        renamed.as_path()
    };
    let io_err = |e: std::io::Error| AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", from.display()));
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    fs::copy(from, dest).map_err(io_err)?;
    let same = fs::metadata(dest).is_ok_and(|m| m.len() == size)
        && hash_file(dest, hash.kind).is_ok_and(|h| h.eq_ignore_ascii_case(&hash.hex));
    if !same {
        let _ = fs::remove_file(dest);
        return Err(AppError::new(ErrorCode::IntegrityMismatch, format!("{} changed", from.display()))
            .with_param("name", name));
    }
    Ok(())
}

/// A finished file in `dir` of `size` bytes and hash `hash`.
fn same_file_in(dir: &Path, size: u64, hash: &ExpectedHash) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|path| {
        !still_downloading(path)
            && fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size)
            && hash_file(path, hash.kind).is_ok_and(|h| h.eq_ignore_ascii_case(&hash.hex))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_taken_only_while_it_is_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("held.jar");
        fs::write(&from, b"hello").unwrap();
        let hash = ExpectedHash::sha1(HELLO_SHA1);
        let dest = dir.path().join("game/mods/held.jar");
        take_copy(&from, &dest, 5, &hash, "held.jar").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        fs::write(&from, b"jello").unwrap();
        let other = dir.path().join("game/mods/again.jar");
        let e = take_copy(&from, &other, 5, &hash, "held.jar").unwrap_err();
        assert_eq!((e.code, e.params["name"].as_str()), (ErrorCode::IntegrityMismatch, "held.jar"));
        assert!(!other.exists(), "a wrong copy does not stay");
    }

    const HELLO_SHA1: &str = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";

    #[test]
    fn a_copy_is_found_by_its_size_and_sha1_whatever_its_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("held (1).jar"), b"hello").unwrap();
        fs::write(dir.path().join("other.jar"), b"jello").unwrap();
        fs::write(dir.path().join("big.jar"), b"hello, world").unwrap();
        let found = find_copies(
            dir.path(),
            &[
                Wanted { name: "held.jar", size: 5, sha1: HELLO_SHA1 },
                Wanted { name: "gone.jar", size: 5, sha1: &"0".repeat(40) },
            ],
        );
        assert_eq!(found, [Some(dir.path().join("held (1).jar")), None]);
    }

    #[test]
    fn a_file_the_browser_still_downloads_is_no_copy() {
        // Firefox writes `A8B-smCw.jar.part` and renames it once done; Chrome `….crdownload`. The
        // bytes may be all there before the rename: taken then, the file is gone by the copy.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("A8B-smCw.jar.part"), b"hello").unwrap();
        fs::write(dir.path().join("held.jar.crdownload"), b"hello").unwrap();
        fs::write(dir.path().join("Unconfirmed 1.crdownload"), b"hello").unwrap();
        let wanted = [Wanted { name: "held.jar", size: 5, sha1: HELLO_SHA1 }];
        assert_eq!(find_copies(dir.path(), &wanted), [None]);
        fs::rename(dir.path().join("A8B-smCw.jar.part"), dir.path().join("held.jar")).unwrap();
        assert_eq!(find_copies(dir.path(), &wanted), [Some(dir.path().join("held.jar"))]);
    }

    #[test]
    fn a_copy_renamed_after_it_was_found_is_taken_under_its_new_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("held.jar"), b"hello").unwrap();
        let dest = dir.path().join("game/mods/held.jar");
        let gone = dir.path().join("A8B-smCw.jar.part");
        take_copy(&gone, &dest, 5, &ExpectedHash::sha1(HELLO_SHA1), "held.jar").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        let nowhere = dir.path().join("game/mods/other.jar");
        let e = take_copy(&gone, &nowhere, 5, &ExpectedHash::sha1(&"0".repeat(40)), "other.jar").unwrap_err();
        assert_eq!((e.code, e.params["name"].as_str()), (ErrorCode::NotFound, "other.jar"), "{e:?}");
    }

    #[test]
    fn the_file_of_its_own_name_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a copy.jar"), b"hello").unwrap();
        fs::write(dir.path().join("held.jar"), b"hello").unwrap();
        let found = find_copies(dir.path(), &[Wanted { name: "held.jar", size: 5, sha1: HELLO_SHA1 }]);
        assert_eq!(found, [Some(dir.path().join("held.jar"))]);
        assert_eq!(
            find_copies(
                &dir.path().join("missing"),
                &[Wanted { name: "held.jar", size: 5, sha1: HELLO_SHA1 }]
            ),
            [None],
            "no folder, no copy"
        );
    }
}
