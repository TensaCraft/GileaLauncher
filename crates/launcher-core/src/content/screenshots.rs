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

/// The MIME type of a screenshot file.
pub fn image_type(name: &str) -> &'static str {
    if ends_with_ci(name, ".png") { "image/png" } else { "image/jpeg" }
}

/// The screenshots in `<game>/screenshots`, newest first (then by name).
pub fn list_screenshots(game_dir: &Path) -> Vec<ScreenshotFile> {
    let Ok(read) = fs::read_dir(game_dir.join(SCREENSHOTS)) else { return Vec::new() };
    let mut shots: Vec<ScreenshotFile> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let path = e.path();
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
        })
        .collect();
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
}
