//! Screenshot thumbnails for the Screenshots page: made once per picture (its path, size and
//! time name the file) in the cache, 480 pixels wide, so the page never loads hundreds of
//! full-size pictures.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use image::codecs::jpeg::JpegEncoder;
use launcher_shared::{AppError, AppResult, ErrorCode};
use sha1::{Digest, Sha1};

use super::screenshots::ScreenshotFile;
use crate::storage::atomic::rename_retrying;

/// A thumbnail's width; its height follows the picture.
pub const THUMB_WIDTH: u32 = 480;
const QUALITY: u8 = 80;
/// Pictures decoded at once (a 4K one takes some 33 MB while it is).
const DECODING_MOST: usize = 3;

pub struct Thumbs {
    dir: PathBuf,
    decoding: Mutex<usize>,
    freed: Condvar,
    made: AtomicU64,
}

/// One of the `DECODING_MOST` places to decode in, given back when dropped.
struct Slot<'a>(&'a Thumbs);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.decoding.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

fn failed(shot: &ScreenshotFile, e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Unsupported, format!("no thumbnail of {}: {e}", shot.path.display()))
        .with_param("name", &shot.name)
}

impl Thumbs {
    /// Thumbnails kept in `dir` (made when the first one is).
    pub fn new(dir: PathBuf) -> Thumbs {
        Thumbs { dir, decoding: Mutex::new(0), freed: Condvar::new(), made: AtomicU64::new(0) }
    }

    /// Where the thumbnail of `shot` as it is now lives.
    fn path_of(&self, shot: &ScreenshotFile) -> PathBuf {
        let mut hash = Sha1::new();
        hash.update(shot.path.to_string_lossy().as_bytes());
        hash.update(shot.size.to_le_bytes());
        hash.update(shot.modified_ms.unwrap_or(0).to_le_bytes());
        self.dir.join(format!("{}.jpg", hex::encode(&hash.finalize()[..12])))
    }

    fn slot(&self) -> Slot<'_> {
        let mut busy = self.decoding.lock().unwrap_or_else(|e| e.into_inner());
        while *busy >= DECODING_MOST {
            busy = self.freed.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        Slot(self)
    }

    /// The thumbnail of `shot`, made when there is none yet (blocking).
    pub fn thumbnail(&self, shot: &ScreenshotFile) -> AppResult<PathBuf> {
        let path = self.path_of(shot);
        if path.is_file() {
            return Ok(path);
        }
        let _slot = self.slot();
        // Another request may have made it while this one waited.
        if path.is_file() {
            return Ok(path);
        }
        let picture = image::ImageReader::open(&shot.path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|e| failed(shot, e))?
            .decode()
            .map_err(|e| failed(shot, e))?;
        let small =
            if picture.width() > THUMB_WIDTH { picture.thumbnail(THUMB_WIDTH, u32::MAX) } else { picture };
        fs::create_dir_all(&self.dir).map_err(|e| failed(shot, e))?;
        let turn = self.made.fetch_add(1, Ordering::Relaxed);
        let part = path.with_extension(format!("{}-{turn}.part", std::process::id()));
        let written = File::create(&part).map_err(|e| failed(shot, e)).and_then(|file| {
            let mut out = BufWriter::new(file);
            JpegEncoder::new_with_quality(&mut out, QUALITY)
                .encode_image(&small.to_rgb8())
                .map_err(|e| failed(shot, e))?;
            out.flush().map_err(|e| failed(shot, e))
        });
        match written.and_then(|()| rename_retrying(&part, &path).map_err(|e| failed(shot, e))) {
            Ok(()) => Ok(path),
            Err(e) => {
                let _ = fs::remove_file(&part);
                Err(e)
            }
        }
    }

    /// Drops the thumbnail of `shot` (it was renamed or deleted).
    pub fn forget(&self, shot: &ScreenshotFile) {
        let _ = fs::remove_file(self.path_of(shot));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use image::{Rgb, RgbImage};

    use super::*;
    use crate::content::screenshots::{SCREENSHOTS, find_screenshot};

    fn picture(game: &std::path::Path, name: &str, width: u32, height: u32) -> ScreenshotFile {
        let dir = game.join(SCREENSHOTS);
        fs::create_dir_all(&dir).unwrap();
        RgbImage::from_pixel(width, height, Rgb([20, 160, 120])).save(dir.join(name)).unwrap();
        find_screenshot(game, name).unwrap()
    }

    #[test]
    fn a_thumbnail_is_made_once_and_keeps_the_picture_s_shape() {
        let (game, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let thumbs = Thumbs::new(cache.path().join("thumbs"));
        let shot = picture(game.path(), "wide.png", 1920, 1080);
        let made = thumbs.thumbnail(&shot).unwrap();
        assert_eq!(image::image_dimensions(&made).unwrap(), (480, 270));
        let stamp = fs::metadata(&made).unwrap().modified().unwrap();
        assert_eq!(thumbs.thumbnail(&shot).unwrap(), made, "kept");
        assert_eq!(fs::metadata(&made).unwrap().modified().unwrap(), stamp, "not made again");
        let small = picture(game.path(), "small.png", 320, 200);
        assert_eq!(
            image::image_dimensions(thumbs.thumbnail(&small).unwrap()).unwrap(),
            (320, 200),
            "never enlarged"
        );
        assert_eq!(fs::read_dir(cache.path().join("thumbs")).unwrap().count(), 2, "no part files left");
    }

    #[test]
    fn a_changed_picture_gets_a_new_thumbnail_and_a_gone_one_is_forgotten() {
        let (game, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let thumbs = Thumbs::new(cache.path().to_path_buf());
        let shot = picture(game.path(), "a.png", 960, 540);
        let first = thumbs.thumbnail(&shot).unwrap();
        File::options()
            .write(true)
            .open(&shot.path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_000))
            .unwrap();
        let changed = find_screenshot(game.path(), "a.png").unwrap();
        let second = thumbs.thumbnail(&changed).unwrap();
        assert_ne!(first, second);
        thumbs.forget(&changed);
        assert!(!second.exists());
    }

    #[test]
    fn a_file_that_is_no_picture_has_no_thumbnail() {
        let (game, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = game.path().join(SCREENSHOTS);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("broken.png"), b"not a picture").unwrap();
        let thumbs = Thumbs::new(cache.path().to_path_buf());
        let err = thumbs.thumbnail(&find_screenshot(game.path(), "broken.png").unwrap()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}
