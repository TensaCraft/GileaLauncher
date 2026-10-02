//! CurseForge's fingerprint of a file: MurmurHash2 (seed 1) of its bytes without whitespace
//! (tab, line feed, carriage return, space). CurseForge finds a file by it.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The fingerprint of `bytes`.
pub fn fingerprint(bytes: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    let data: Vec<u8> = bytes.iter().copied().filter(|b| !matches!(b, 9 | 10 | 13 | 32)).collect();
    let mut hash = 1u32 ^ data.len() as u32;
    let (chunks, rest) = data.as_chunks::<4>();
    for chunk in chunks {
        let mut k = u32::from_le_bytes(*chunk);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        hash = hash.wrapping_mul(M) ^ k;
    }
    if rest.len() >= 3 {
        hash ^= u32::from(rest[2]) << 16;
    }
    if rest.len() >= 2 {
        hash ^= u32::from(rest[1]) << 8;
    }
    if !rest.is_empty() {
        hash ^= u32::from(rest[0]);
        hash = hash.wrapping_mul(M);
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^ (hash >> 15)
}

/// A file's size and time, and its fingerprint then.
type Known = (u64, Option<SystemTime>, u32);

/// Fingerprints by file, kept while the file's size and time stay.
#[derive(Default)]
pub struct FingerprintCache {
    known: Mutex<HashMap<PathBuf, Known>>,
}

impl FingerprintCache {
    pub fn of(&self, path: &Path) -> io::Result<u32> {
        let meta = fs::metadata(path)?;
        let stamp = (meta.len(), meta.modified().ok());
        if let Some((len, time, value)) = self.known.lock().unwrap_or_else(|e| e.into_inner()).get(path)
            && (*len, *time) == stamp
        {
            return Ok(*value);
        }
        let value = fingerprint(&fs::read(path)?);
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_path_buf(), (stamp.0, stamp.1, value));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_match_curseforge_s() {
        // Values of CurseForge's algorithm; a real Sodium jar (fingerprint 254184734) agreed.
        for (bytes, expected) in [
            (&b""[..], 1_540_447_798u32),
            (b"a", 626_045_324),
            (b"ab", 1_692_487_918),
            (b"abc", 1_621_425_345),
            (b"abcd", 3_376_380_438),
            (b"hello world", 2_824_650_221),
            (b"hello\tworld\r\n", 2_824_650_221),
            (b"The quick brown fox jumps over the lazy dog", 3_751_777_527),
        ] {
            assert_eq!(fingerprint(bytes), expected, "{:?}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn a_changed_file_is_fingerprinted_again() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.jar");
        fs::write(&path, b"abc").unwrap();
        let cache = FingerprintCache::default();
        assert_eq!(cache.of(&path).unwrap(), 1_621_425_345);
        fs::write(&path, b"abcd").unwrap();
        assert_eq!(cache.of(&path).unwrap(), 3_376_380_438);
    }
}
