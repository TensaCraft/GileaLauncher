//! A build's screenshots: `.png`, `.jpg` and `.jpeg` files directly in
//! `<game>/screenshots`, newest first. Links and folders are skipped.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use launcher_shared::{AppError, AppResult, ErrorCode};

use super::inventory::ends_with_ci;

/// The folder of a build's screenshots.
pub const SCREENSHOTS: &str = "screenshots";
const IMAGES: [&str; 3] = [".png", ".jpg", ".jpeg"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified_ms: Option<u64>,
}

pub fn is_image(name: &str) -> bool {
    IMAGES.iter().any(|ext| ends_with_ci(name, ext))
}

/// Screenshot `name` of the game folder, as the list would have it: a plain name of an image file
/// right in `<game>/screenshots` (no path, no link, no Windows drive or stream), found without
/// listing the others.
pub fn find_screenshot(game_dir: &Path, name: &str) -> Option<ScreenshotFile> {
    let path_like = name.contains(['/', '\\', '\0']) || (cfg!(windows) && name.contains(':'));
    let plain = !name.is_empty() && name != "." && name != ".." && !path_like;
    plain.then(|| shot_at(name.to_string(), game_dir.join(SCREENSHOTS).join(name)))?
}

/// The screenshot at `path` named `name`, when it is an image file (a link is not followed).
fn shot_at(name: String, path: PathBuf) -> Option<ScreenshotFile> {
    let meta = fs::symlink_metadata(&path).ok()?;
    (meta.is_file() && is_image(&name)).then(|| ScreenshotFile {
        modified_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        size: meta.len(),
        name,
        path,
    })
}

/// The MIME type of a screenshot file.
pub fn image_type(name: &str) -> &'static str {
    if ends_with_ci(name, ".png") { "image/png" } else { "image/jpeg" }
}

/// Whether `a` and `b` name one file (its real path, case included, as the system gives it).
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// The longest name (without its extension) a screenshot may be given.
pub const NAME_MOST: usize = 120;
/// Names Windows keeps for devices, with any extension.
const DEVICES: [&str; 26] = [
    "con", "prn", "aux", "nul", "conin$", "conout$", "com0", "com1", "com2", "com3", "com4", "com5", "com6",
    "com7", "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The file name a screenshot `old` gets when the user asks for `wanted`: trimmed, its own
/// extension kept (typed again, it is not doubled). `FileNameInvalid` for a name no system takes.
pub fn new_name(old: &str, wanted: &str) -> AppResult<String> {
    let ext = old.rfind('.').map_or("", |at| &old[at..]);
    let wanted = wanted.trim();
    let stem = match wanted.len().checked_sub(ext.len()) {
        Some(at)
            if !ext.is_empty() && wanted.is_char_boundary(at) && wanted[at..].eq_ignore_ascii_case(ext) =>
        {
            wanted[..at].trim_end()
        }
        _ => wanted,
    };
    let bad_char = stem
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'));
    // `con.txt` is the device as much as `con`.
    let base = stem.split('.').next().unwrap_or(stem).trim_end().to_ascii_lowercase();
    let device = DEVICES.contains(&base.as_str());
    if stem.is_empty()
        || stem.chars().all(|c| c == '.')
        || stem.ends_with(['.', ' '])
        || bad_char
        || device
        || stem.chars().count() > NAME_MOST
    {
        return Err(AppError::new(ErrorCode::FileNameInvalid, format!("not a file name: {wanted:?}")));
    }
    Ok(format!("{stem}{ext}"))
}

/// Renames `shot` to the name `wanted` gives (`new_name`) beside it; another file of that name
/// is never replaced (`FileNameTaken`). The same name changes nothing.
pub fn rename_screenshot(shot: &ScreenshotFile, wanted: &str) -> AppResult<ScreenshotFile> {
    let name = new_name(&shot.name, wanted)?;
    if name == shot.name {
        return Ok(shot.clone());
    }
    let to = shot.path.with_file_name(&name);
    // A name that differs only in case is this very file where names ignore case (Windows,
    // macOS); elsewhere it may be another one, never replaced.
    if fs::symlink_metadata(&to).is_ok() && !same_file(&shot.path, &to) {
        return Err(AppError::new(ErrorCode::FileNameTaken, format!("{} exists", to.display()))
            .with_param("name", &name));
    }
    fs::rename(&shot.path, &to).map_err(|e| {
        AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", shot.path.display()))
            .with_param("name", &shot.name)
    })?;
    shot_at(name, to).ok_or_else(|| AppError::new(ErrorCode::NotFound, "the renamed screenshot is gone"))
}

/// The picture of `shot` as RGBA pixels, with its width and height (copying it to the
/// clipboard).
pub fn rgba(shot: &ScreenshotFile) -> AppResult<(u32, u32, Vec<u8>)> {
    let picture = image::open(&shot.path).map_err(|e| {
        AppError::new(ErrorCode::Unsupported, format!("{}: {e}", shot.path.display()))
            .with_param("name", &shot.name)
    })?;
    let pixels = picture.to_rgba8();
    Ok((pixels.width(), pixels.height(), pixels.into_raw()))
}

/// The screenshots in `<game>/screenshots`, newest first (then by name).
pub fn list_screenshots(game_dir: &Path) -> Vec<ScreenshotFile> {
    let Ok(read) = fs::read_dir(game_dir.join(SCREENSHOTS)) else { return Vec::new() };
    let mut shots: Vec<ScreenshotFile> =
        read.flatten().filter_map(|e| shot_at(e.file_name().into_string().ok()?, e.path())).collect();
    shots.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.name.cmp(&b.name)));
    shots
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn screenshots_are_images_newest_first() {
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(dir.join("folder.png")).unwrap();
        for (name, secs) in
            [("old.png", 100), ("new.JPG", 300), ("mid.jpeg", 200), ("notes.txt", 400), ("b.png", 100)]
        {
            let path = dir.join(name);
            fs::write(&path, b"img").unwrap();
            let at = UNIX_EPOCH + Duration::from_secs(secs);
            fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
        }
        let shots = list_screenshots(game.path());
        let names: Vec<&str> = shots.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["new.JPG", "mid.jpeg", "b.png", "old.png"]);
        assert_eq!((shots[0].modified_ms, shots[0].size), (Some(300_000), 3));
        assert!(list_screenshots(&game.path().join("none")).is_empty());
        assert_eq!(
            (image_type("a.PNG"), image_type("b.jpeg"), image_type("c.JPG")),
            ("image/png", "image/jpeg", "image/jpeg")
        );
        assert!(is_image("X.JpEg") && !is_image("x.png.txt"));
    }

    #[test]
    fn one_screenshot_is_found_without_listing_the_rest() {
        // Each picture the page shows asks for one file: found directly, and only one the list
        // would have (an image right in the folder, no link, no path).
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(dir.join("folder.png")).unwrap();
        fs::write(dir.join("a.png"), b"img").unwrap();
        fs::write(dir.join("notes.txt"), b"t").unwrap();
        fs::write(dir.join("sub").join("inner.png"), b"i").unwrap();
        fs::write(game.path().join("outside.png"), b"o").unwrap();
        let found = find_screenshot(game.path(), "a.png").unwrap();
        assert_eq!(Some(&found), list_screenshots(game.path()).iter().find(|s| s.name == "a.png"));
        for name in [
            "",
            ".",
            "..",
            "../outside.png",
            "..\\outside.png",
            "sub/inner.png",
            "sub\\inner.png",
            "sub",
            "folder.png",
            "notes.txt",
            "missing.png",
            "a.png:stream",
            "/a.png",
        ] {
            assert_eq!(find_screenshot(game.path(), name), None, "{name}");
        }
    }

    #[test]
    fn a_new_name_keeps_the_picture_s_kind() {
        assert_eq!(
            renamed("2026-10-03_17.21.png", "  Аеродром на світанку "),
            Ok("Аеродром на світанку.png".into())
        );
        assert_eq!(renamed("a.jpeg", "b.JPEG"), Ok("b.jpeg".into()), "a typed extension is not doubled");
        assert_eq!(renamed("a.png", "b.jpg"), Ok("b.jpg.png".into()), "another kind stays part of the name");
        for wanted in [
            "",
            "  ",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a:b",
            "a*b",
            "a?b",
            "a\"b",
            "a<b",
            "a>b",
            "a|b",
            "tab\tname",
            "CON",
            "nul",
            "Com1",
            "LPT9",
            "end.",
            "end. ",
        ] {
            assert_eq!(renamed("a.png", wanted), Err(()), "{wanted:?}");
        }
        assert!(renamed("a.png", &"я".repeat(120)).is_ok());
        // Windows takes a device name with any extension, and these too.
        for device in ["con.txt", "Aux.log", "COM0", "lpt0", "CONIN$", "conout$.x"] {
            assert_eq!(renamed("a.png", device), Err(()), "{device:?}");
        }
        assert!(renamed("a.png", "console").is_ok() && renamed("a.png", "con-tour").is_ok());
        assert_eq!(renamed("a.png", &"я".repeat(121)), Err(()));
    }

    fn renamed(old: &str, wanted: &str) -> Result<String, ()> {
        new_name(old, wanted).map_err(|_| ())
    }

    #[test]
    fn a_name_that_differs_only_in_case_renames_the_same_file() {
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("base.png"), b"b").unwrap();
        let shot = find_screenshot(game.path(), "base.png").unwrap();
        let moved = rename_screenshot(&shot, "BASE").unwrap();
        assert_eq!(moved.name, "BASE.png");
        assert_eq!(fs::read(&moved.path).unwrap(), b"b");
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_differs_only_in_case_never_replaces_another_file() {
        // Files whose names differ only in case are two files on Linux.
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("base.png"), b"lower").unwrap();
        fs::write(dir.join("Base.png"), b"upper").unwrap();
        let shot = find_screenshot(game.path(), "base.png").unwrap();
        assert_eq!(rename_screenshot(&shot, "Base").unwrap_err().code, ErrorCode::FileNameTaken);
        assert_eq!(fs::read(dir.join("Base.png")).unwrap(), b"upper");
    }

    #[test]
    fn a_screenshot_is_renamed_beside_the_others_and_never_over_one() {
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.png"), b"a").unwrap();
        fs::write(dir.join("taken.png"), b"t").unwrap();
        let shot = find_screenshot(game.path(), "a.png").unwrap();
        let moved = rename_screenshot(&shot, "Мій кадр").unwrap();
        assert_eq!(moved.name, "Мій кадр.png");
        assert_eq!(fs::read(dir.join("Мій кадр.png")).unwrap(), b"a");
        assert!(!dir.join("a.png").exists());
        let err = rename_screenshot(&moved, "taken").unwrap_err();
        assert_eq!(
            (err.code, err.params.get("name").map(String::as_str)),
            (ErrorCode::FileNameTaken, Some("taken.png"))
        );
        assert_eq!(fs::read(dir.join("taken.png")).unwrap(), b"t", "nothing replaced");
        assert_eq!(rename_screenshot(&moved, "a/b").unwrap_err().code, ErrorCode::FileNameInvalid);
        assert_eq!(
            rename_screenshot(&moved, "Мій кадр").unwrap().name,
            "Мій кадр.png",
            "the same name is no change"
        );
    }
}
