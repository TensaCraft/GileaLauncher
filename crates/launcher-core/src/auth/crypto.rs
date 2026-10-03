//! Token encryption compatible with the original launcher: Fernet ciphertext with an `enc::`
//! prefix, key in `<app_state>/profile-token.key`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fernet::Fernet;
use launcher_shared::{AppError, AppResult, ErrorCode};

use crate::storage::atomic::atomic_write;

pub const KEY_FILE: &str = "profile-token.key";
pub const ENC_PREFIX: &str = "enc::";

pub struct TokenCipher {
    key_path: PathBuf,
    fernet: Mutex<Option<Fernet>>,
}

impl TokenCipher {
    pub fn new(state_dir: &Path) -> Self {
        Self { key_path: state_dir.join(KEY_FILE), fernet: Mutex::new(None) }
    }

    /// Loads the key, creating it (POSIX `0600`) when missing and replacing it when invalid.
    fn fernet(&self) -> AppResult<Fernet> {
        let mut slot = self.fernet.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(fernet) = slot.as_ref() {
            return Ok(fernet.clone());
        }
        let fernet = load_or_create_key(&self.key_path)
            .map_err(|e| AppError::new(ErrorCode::CredentialStorageUnavailable, e.to_string()))?;
        *slot = Some(fernet.clone());
        Ok(fernet)
    }

    pub fn encrypt(&self, plain: &str) -> AppResult<String> {
        Ok(format!("{ENC_PREFIX}{}", self.fernet()?.encrypt(plain.as_bytes())))
    }

    /// `None` when the value is not ours to read (not sealed, wrong key, tampered, not UTF-8).
    pub fn decrypt(&self, value: &str) -> Option<String> {
        let token = value.strip_prefix(ENC_PREFIX)?;
        let bytes = self.fernet().ok()?.decrypt(token).ok()?;
        String::from_utf8(bytes).ok()
    }

    /// Whether the key can be read now. A key a scanner holds for a moment is no wrong key: the
    /// tokens are not lost, only out of reach until it lets go.
    pub fn key_available(&self) -> bool {
        self.fernet().is_ok()
    }
}

fn load_or_create_key(path: &Path) -> io::Result<Fernet> {
    match fs::read(path) {
        Ok(bytes) => {
            if let Some(fernet) = std::str::from_utf8(&bytes).ok().and_then(|text| Fernet::new(text.trim())) {
                return Ok(fernet);
            }
            tracing::warn!("Replacing an invalid token key at {}", path.display());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let key = Fernet::generate_key();
    atomic_write(path, key.as_bytes())?;
    if let Err(e) = restrict(path) {
        tracing::warn!("Unable to restrict access to {}: {e}", path.display());
    }
    Fernet::new(&key).ok_or_else(|| io::Error::other("the generated key is invalid"))
}

#[cfg(unix)]
fn restrict(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    /// Sealed by Python `cryptography` (the original launcher's library) with `KEY`.
    const PY_TOKEN: &str = "enc::gAAAAABlU_EAAQIDBAUGBwgJCgsMDQ4PEBtaStBOEQHC3xBvW8xpPjGtn-FaEiz18KvF4qu1QhmmkJgVJYDNjz3rGgtlv-5Wzz86emxP4JxBcUTWgyine50=";

    fn with_key(dir: &Path, key: &str) -> TokenCipher {
        fs::write(dir.join(KEY_FILE), key).unwrap();
        TokenCipher::new(dir)
    }

    #[test]
    fn reads_tokens_sealed_by_the_original_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = with_key(dir.path(), KEY);
        assert_eq!(cipher.decrypt(PY_TOKEN).as_deref(), Some("M.C555_BAY.refresh-token"));
    }

    #[test]
    fn round_trip_uses_the_enc_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = TokenCipher::new(dir.path());
        let sealed = cipher.encrypt("secret").unwrap();
        assert!(sealed.starts_with("enc::gAAAAA"), "{sealed}");
        assert!(!sealed.contains("secret"));
        assert_eq!(cipher.decrypt(&sealed).as_deref(), Some("secret"));
    }

    #[test]
    fn creates_a_private_key_file_and_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let sealed = TokenCipher::new(dir.path()).encrypt("a").unwrap();
        let key = fs::read_to_string(dir.path().join(KEY_FILE)).unwrap();
        assert!(Fernet::new(key.trim()).is_some());
        assert_eq!(TokenCipher::new(dir.path()).decrypt(&sealed).as_deref(), Some("a"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn wrong_key_never_returns_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let sealed = TokenCipher::new(other.path()).encrypt("secret").unwrap();
        let cipher = with_key(dir.path(), KEY);
        assert_eq!(cipher.decrypt(&sealed), None);
        assert_eq!(cipher.decrypt("plain-text"), None);
        assert_eq!(cipher.decrypt("enc::garbage"), None);
    }

    #[test]
    fn invalid_key_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = with_key(dir.path(), "garbage");
        let sealed = cipher.encrypt("x").unwrap();
        assert_eq!(cipher.decrypt(&sealed).as_deref(), Some("x"));
        assert_ne!(fs::read_to_string(dir.path().join(KEY_FILE)).unwrap(), "garbage");
    }

    #[test]
    fn unusable_key_location_is_an_error_not_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(KEY_FILE)).unwrap();
        let err = TokenCipher::new(dir.path()).encrypt("secret").unwrap_err();
        assert_eq!(err.code, ErrorCode::CredentialStorageUnavailable);
        assert!(!err.detail.contains("secret"));
    }

    #[test]
    fn a_non_utf8_key_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(KEY_FILE), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let cipher = TokenCipher::new(dir.path());
        let sealed = cipher.encrypt("x").unwrap();
        assert_eq!(cipher.decrypt(&sealed).as_deref(), Some("x"));
    }
}
