//! CurseForge's fingerprint of a file: MurmurHash2 (seed 1) of its bytes without whitespace
//! (tab, line feed, carriage return, space). CurseForge finds a file by it.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const M: u32 = 0x5bd1_e995;
/// How much of a file is read at a time: memory stays small for a pack of a gigabyte.
const PIECE: usize = 256 * 1024;

fn whitespace(byte: u8) -> bool {
    matches!(byte, 9 | 10 | 13 | 32)
}

/// MurmurHash2 fed byte by byte (whitespace already left out): its length is known first.
struct Murmur {
    hash: u32,
    word: [u8; 4],
    filled: usize,
}

impl Murmur {
    fn new(len: u64) -> Murmur {
        Murmur { hash: 1u32 ^ len as u32, word: [0; 4], filled: 0 }
    }

    fn push(&mut self, byte: u8) {
        self.word[self.filled] = byte;
        self.filled += 1;
        if self.filled == 4 {
            let mut k = u32::from_le_bytes(self.word);
            k = k.wrapping_mul(M);
            k ^= k >> 24;
            k = k.wrapping_mul(M);
            self.hash = self.hash.wrapping_mul(M) ^ k;
            self.filled = 0;
        }
    }

    fn finish(self) -> u32 {
        let (mut hash, rest) = (self.hash, &self.word[..self.filled]);
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
}

/// The fingerprint of `bytes`.
pub fn fingerprint(bytes: &[u8]) -> u32 {
    let len = bytes.iter().filter(|b| !whitespace(**b)).count();
    let mut murmur = Murmur::new(len as u64);
    bytes.iter().copied().filter(|b| !whitespace(*b)).for_each(|b| murmur.push(b));
    murmur.finish()
}

/// The fingerprint of the file at `path`, read in pieces twice (its length without whitespace
/// first): never the whole file in memory.
pub fn fingerprint_file(path: &Path) -> io::Result<u32> {
    let mut file = BufReader::with_capacity(PIECE, File::open(path)?);
    let mut piece = vec![0u8; PIECE];
    let mut len = 0u64;
    loop {
        let read = file.read(&mut piece)?;
        if read == 0 {
            break;
        }
        len += piece[..read].iter().filter(|b| !whitespace(**b)).count() as u64;
    }
    file.seek(SeekFrom::Start(0))?;
    let mut murmur = Murmur::new(len);
    loop {
        let read = file.read(&mut piece)?;
        if read == 0 {
            break;
        }
        piece[..read].iter().copied().filter(|b| !whitespace(*b)).for_each(|b| murmur.push(b));
    }
    Ok(murmur.finish())
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
        let value = fingerprint_file(path)?;
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
    fn a_file_is_fingerprinted_as_its_bytes_without_holding_them() {
        // Big resource packs are read in pieces: the same value, in little memory, across every
        // piece boundary and every length of the last word.
        let tmp = tempfile::tempdir().unwrap();
        for len in [0usize, 1, 3, 4, 7, 65_535, 65_536, 65_537, 300_003] {
            let bytes: Vec<u8> = (0..len).map(|i| [b'a', b' ', b'\n', b'z', 7, b'\t', 200][i % 7]).collect();
            let path = tmp.path().join(format!("f{len}.zip"));
            fs::write(&path, &bytes).unwrap();
            assert_eq!(fingerprint_file(&path).unwrap(), fingerprint(&bytes), "{len} bytes");
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
