//! A build's screenshots: `.png`, `.jpg` and `.jpeg` files directly in
//! `<game>/screenshots`, newest first. Links and folders are skipped.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

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
}
