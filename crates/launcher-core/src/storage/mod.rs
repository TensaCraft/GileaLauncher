//! Durable file storage primitives.

pub mod atomic;
pub mod config;
pub mod journal;
pub mod json;
pub mod transaction;
pub mod versions;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Process-wide lock shared by every store that writes the same file.
pub fn path_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let key = normalize_key(path);
    let mut map = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

fn normalize_key(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if cfg!(windows) { PathBuf::from(absolute.to_string_lossy().to_lowercase()) } else { absolute }
}
