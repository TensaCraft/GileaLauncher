//! The launcher's own skin library, shared by every profile: `<app state>/skins/library.json` and
//! a PNG per skin, `<app state>/skins/<id>.png`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::skins::SkinTexture;
use launcher_core::storage::atomic::{atomic_write, rename_retrying};
use launcher_core::storage::json::write_json_file;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::dto::{MESSAGE, SkinVariant};

const RECORD: &str = "library.json";
/// Where an unreadable record is put aside.
const BROKEN: &str = "library.json.broken";
/// The longest name, in characters.
pub const NAME_MAX: usize = 48;

/// A skin of the library as its record keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub variant: SkinVariant,
    pub sha256: String,
    pub added_ms: u64,
    /// `file`, `player:<name>`, `link:<address>` or `account:<name>`.
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    skins: Vec<Entry>,
}

fn io_error(path: &Path, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `name` trimmed: 1–48 characters, else `skins_error_name`.
pub fn checked_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("no skin name {name:?}"))
            .with_param("max", NAME_MAX.to_string())
            .with_param(MESSAGE, "skins_error_name"));
    }
    Ok(name.to_string())
}

/// An id names a file of the library: letters and digits only.
fn checked_id(id: &str) -> AppResult<&str> {
    if !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id)
    } else {
        Err(missing(id))
    }
}

fn missing(id: &str) -> AppError {
    AppError::new(ErrorCode::NotFound, format!("no skin {id} in the library"))
        .with_param(MESSAGE, "skins_error_gone")
}

/// The record, or an empty one; an unreadable record is put aside as `library.json.broken`.
fn read_record(dir: &Path) -> Record {
    let path = dir.join(RECORD);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return Record::default(),
    };
    match serde_json::from_slice::<Record>(&bytes) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!("The skin library's record is unreadable ({e}); it is put aside");
            if let Err(e) = rename_retrying(&path, &dir.join(BROKEN)) {
                tracing::warn!("Unable to put the skin library's record aside: {e}");
            }
            Record::default()
        }
    }
}

pub struct Library {
    dir: PathBuf,
    record: Mutex<Record>,
}

impl Library {
    pub fn open(dir: PathBuf) -> Library {
        let record = read_record(&dir);
        Library { dir, record: Mutex::new(record) }
    }

    fn lock(&self) -> MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn png_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.png"))
    }

    fn has_png(&self, entry: &Entry) -> bool {
        self.png_path(&entry.id).is_file()
    }

    /// Writes `record` and only then keeps it.
    fn save(&self, held: &mut Record, record: Record) -> AppResult<()> {
        fs::create_dir_all(&self.dir).map_err(|e| io_error(&self.dir, e))?;
        let path = self.dir.join(RECORD);
        let value = serde_json::to_value(&record).map_err(|e| AppError::internal(e.to_string()))?;
        write_json_file(&path, &value, 2).map_err(|e| io_error(&path, e))?;
        *held = record;
        Ok(())
    }

    /// `change` made to entry `id`, written.
    fn change(&self, id: &str, change: impl FnOnce(&mut Entry)) -> AppResult<Entry> {
        let id = checked_id(id)?;
        let mut held = self.lock();
        let mut record = held.clone();
        let entry = record.skins.iter_mut().find(|e| e.id == id).ok_or_else(|| missing(id))?;
        change(entry);
        let changed = entry.clone();
        self.save(&mut held, record)?;
        Ok(changed)
    }

    /// Every skin whose file is there, the newest first.
    pub fn list(&self) -> Vec<Entry> {
        self.lock().skins.iter().rev().filter(|e| self.has_png(e)).cloned().collect()
    }

    /// Adds `image`; a skin already there (by its hash) is returned as it is, with `false`.
    pub fn add(
        &self,
        image: &SkinTexture,
        name: &str,
        variant: SkinVariant,
        source: &str,
    ) -> AppResult<(Entry, bool)> {
        let name = checked_name(name)?;
        let mut held = self.lock();
        if let Some(entry) = held.skins.iter().find(|e| e.sha256 == image.hash && self.has_png(e)) {
            return Ok((entry.clone(), false));
        }
        let entry = Entry {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name,
            variant,
            sha256: image.hash.clone(),
            added_ms: now_ms(),
            source: source.to_string(),
        };
        let path = self.png_path(&entry.id);
        atomic_write(&path, &image.png).map_err(|e| io_error(&path, e))?;
        let mut record = held.clone();
        // An entry whose file is gone gives way to the new one.
        record.skins.retain(|e| e.sha256 != image.hash);
        record.skins.push(entry.clone());
        if let Err(e) = self.save(&mut held, record) {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        Ok((entry, true))
    }

    pub fn rename(&self, id: &str, name: &str) -> AppResult<Entry> {
        let name = checked_name(name)?;
        self.change(id, |e| e.name = name)
    }

    pub fn set_variant(&self, id: &str, variant: SkinVariant) -> AppResult<Entry> {
        self.change(id, |e| e.variant = variant)
    }

    pub fn get(&self, id: &str) -> AppResult<Entry> {
        let id = checked_id(id)?;
        self.lock().skins.iter().find(|e| e.id == id && self.has_png(e)).cloned().ok_or_else(|| missing(id))
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        let id = checked_id(id)?;
        let mut held = self.lock();
        let mut record = held.clone();
        let before = record.skins.len();
        record.skins.retain(|e| e.id != id);
        if record.skins.len() == before {
            return Err(missing(id));
        }
        self.save(&mut held, record)?;
        let path = self.png_path(id);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_error(&path, e)),
            _ => Ok(()),
        }
    }

    /// The skin's PNG.
    pub fn png(&self, id: &str) -> AppResult<Vec<u8>> {
        let id = checked_id(id)?;
        fs::read(self.png_path(id)).map_err(|_| missing(id))
    }

    /// The skin with hash `sha256`, if its file is there.
    pub fn find_sha(&self, sha256: &str) -> Option<Entry> {
        self.lock().skins.iter().find(|e| e.sha256 == sha256 && self.has_png(e)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::dto::MESSAGE;

    /// A skin told apart from others by `mark`.
    fn skin(mark: u8) -> SkinTexture {
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([90, 60, 40, 255]));
        image.put_pixel(9, 9, Rgba([mark, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        SkinTexture::read(&png.into_inner()).unwrap()
    }

    fn message(result: AppResult<impl std::fmt::Debug>) -> Option<String> {
        result.err().and_then(|e| e.params.get(MESSAGE).cloned())
    }

    #[test]
    fn the_same_skin_twice_is_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("skins"));
        let (first, added) = library.add(&skin(1), "Мій", SkinVariant::Classic, "file").unwrap();
        assert!(added);
        assert_eq!(first.id.len(), 32);
        let (again, added) = library.add(&skin(1), "Інша назва", SkinVariant::Slim, "player:Notch").unwrap();
        assert!(!added);
        assert_eq!(again, first, "the second import returns the first");
        assert_eq!(library.list(), std::slice::from_ref(&first));
        assert_eq!(library.png(&first.id).unwrap(), skin(1).png);
        assert_eq!(library.find_sha(&skin(1).hash), Some(first));
        assert_eq!(library.find_sha(&skin(2).hash), None);
    }

    #[test]
    fn rename_variant_and_delete_persist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skins");
        let library = Library::open(root.clone());
        let (one, _) = library.add(&skin(1), "Один", SkinVariant::Classic, "file").unwrap();
        let (two, _) = library.add(&skin(2), "Два", SkinVariant::Classic, "account:Notch_UA").unwrap();
        assert_eq!(library.rename(&one.id, "Перший").unwrap().name, "Перший");
        assert_eq!(library.set_variant(&one.id, SkinVariant::Slim).unwrap().variant, SkinVariant::Slim);
        let reopened = Library::open(root.clone());
        let kept = reopened.list().into_iter().find(|e| e.id == one.id).unwrap();
        assert_eq!((kept.name.as_str(), kept.variant), ("Перший", SkinVariant::Slim));
        reopened.delete(&two.id).unwrap();
        assert!(!root.join(format!("{}.png", two.id)).exists(), "its file is gone");
        let ids: Vec<String> = Library::open(root).list().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, [one.id]);
        assert!(library.png("../library").is_err(), "an id names no other file");
        assert!(reopened.rename(&two.id, "Нема").is_err());
    }

    #[test]
    fn the_newest_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("skins"));
        let (one, _) = library.add(&skin(1), "Один", SkinVariant::Classic, "file").unwrap();
        let (two, _) = library.add(&skin(2), "Два", SkinVariant::Classic, "file").unwrap();
        assert_eq!(library.list(), [two, one]);
    }

    #[test]
    fn a_broken_record_reads_as_empty_and_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skins");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("library.json"), "{").unwrap();
        let library = Library::open(root.clone());
        assert!(library.list().is_empty());
        assert_eq!(std::fs::read_to_string(root.join("library.json.broken")).unwrap(), "{");
        library.add(&skin(1), "Новий", SkinVariant::Classic, "file").unwrap();
        assert_eq!(Library::open(root).list().len(), 1, "a new record is written");
    }

    #[test]
    fn a_missing_png_is_dropped_from_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skins");
        let library = Library::open(root.clone());
        let (entry, _) = library.add(&skin(1), "Зник", SkinVariant::Classic, "file").unwrap();
        std::fs::remove_file(root.join(format!("{}.png", entry.id))).unwrap();
        assert!(library.list().is_empty());
        assert_eq!(library.find_sha(&entry.sha256), None);
        let (again, added) = library.add(&skin(1), "Знову", SkinVariant::Classic, "file").unwrap();
        assert!(added, "the skin comes back as a new entry");
        assert_eq!(library.list(), [again]);
    }

    #[test]
    fn names_are_trimmed_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("skins"));
        let (entry, _) = library.add(&skin(1), "  Стів  ", SkinVariant::Classic, "file").unwrap();
        assert_eq!(entry.name, "Стів");
        for bad in ["", "   ", &"ї".repeat(49)] {
            assert_eq!(
                message(library.add(&skin(2), bad, SkinVariant::Classic, "file")).as_deref(),
                Some("skins_error_name"),
                "{bad:?}"
            );
            assert_eq!(message(library.rename(&entry.id, bad)).as_deref(), Some("skins_error_name"));
        }
        assert_eq!(library.rename(&entry.id, &"ї".repeat(48)).unwrap().name.chars().count(), 48);
    }
}
