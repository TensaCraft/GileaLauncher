//! `profiles.json`: accounts keyed by player name, tokens sealed by `TokenCipher`.
//! Every change is built on a copy and written atomically; the in-memory
//! state changes only after the file did.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use launcher_shared::{
    AccountKind, AppError, AppResult, ErrorCode, ProfileDto, ProfilesSnapshot, sort_profiles,
};
use serde_json::{Map, Value, json};

use super::TOKEN_REFRESH_LEEWAY;
use super::crypto::{ENC_PREFIX, TokenCipher};
use super::offline::offline_uuid;
use crate::storage::json::{JsonRead, backup_corrupt_file, read_json_object, write_json_file};

pub const PROFILES_FILE: &str = "profiles.json";
pub const OFFLINE_TOKEN: &str = "offline";
pub const REASON_REAUTH: &str = "reauth_required";
pub const REASON_DECRYPTION_FAILED: &str = "token_decryption_failed";
pub const REASON_REFRESH_MISSING: &str = "refresh_token_missing";
pub const REASON_REFRESH_INVALID: &str = "refresh_token_invalid";
pub const REASON_ENCRYPTION_UNAVAILABLE: &str = "credential_encryption_unavailable";
const TOKEN_FIELDS: [&str; 2] = ["access_token", "refresh_token"];

type Record = Map<String, Value>;

/// A profile with its tokens decrypted. Never sent to the UI as is (see `to_dto`).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredProfile {
    pub key: String,
    pub id: String,
    pub name: String,
    pub kind: AccountKind,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub xuid: Option<String>,
    pub expires_at: Option<i64>,
    pub auth_client_id: Option<String>,
    pub is_default: bool,
    marked: bool,
    stored_reason: Option<String>,
    decryption_failed: bool,
    /// The token key could not be read (for a moment): the tokens are out of reach, not lost.
    key_unavailable: bool,
}

impl StoredProfile {
    pub fn is_offline(&self) -> bool {
        self.kind == AccountKind::Offline
    }

    /// `reauth_required` is already written to the file.
    pub fn is_marked(&self) -> bool {
        self.marked
    }

    /// Why this Microsoft profile needs a new sign-in; priority as in the original: stored reason,
    /// undecryptable tokens, missing refresh token.
    pub fn reauth_reason(&self) -> Option<String> {
        if self.is_offline()
            || self.key_unavailable
            || !(self.marked || self.decryption_failed || self.refresh_token.is_none())
        {
            return None;
        }
        Some(match &self.stored_reason {
            Some(reason) => reason.clone(),
            None if self.decryption_failed => REASON_DECRYPTION_FAILED.to_string(),
            None if self.refresh_token.is_none() => REASON_REFRESH_MISSING.to_string(),
            None => REASON_REAUTH.to_string(),
        })
    }

    /// The token key could not be read: nothing can be signed or renewed until it can.
    pub fn key_unavailable(&self) -> bool {
        self.key_unavailable
    }

    /// A usable access token that stays valid for at least `TOKEN_REFRESH_LEEWAY` seconds.
    pub fn is_fresh(&self, now: i64) -> bool {
        self.is_fresh_for(now, TOKEN_REFRESH_LEEWAY)
    }

    /// A usable access token that stays valid for at least `margin` seconds.
    pub fn is_fresh_for(&self, now: i64, margin: i64) -> bool {
        self.access_token.is_some() && self.expires_at.is_some_and(|t| t > now + margin)
    }

    pub fn to_dto(&self) -> ProfileDto {
        let reason = self.reauth_reason();
        ProfileDto {
            key: self.key.clone(),
            name: self.name.clone(),
            id: self.id.clone(),
            kind: self.kind,
            is_default: self.is_default,
            reauth_required: reason.is_some(),
            reauth_reason: reason,
        }
    }
}

/// A profile to add; tokens in clear text, sealed on save.
#[derive(Debug, Clone, PartialEq)]
pub struct NewProfile {
    pub id: Option<String>,
    pub name: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub xuid: Option<String>,
    pub expires_at: Option<i64>,
    pub auth_client_id: Option<String>,
}

impl NewProfile {
    pub fn offline(name: &str) -> NewProfile {
        NewProfile {
            id: None,
            name: name.to_string(),
            access_token: Some(OFFLINE_TOKEN.to_string()),
            refresh_token: Some(OFFLINE_TOKEN.to_string()),
            xuid: None,
            expires_at: None,
            auth_client_id: None,
        }
    }
}

/// Tokens after a refresh; `refresh_token: None` keeps the stored one.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenUpdate {
    pub id: String,
    pub name: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub xuid: Option<String>,
    pub expires_at: i64,
    pub auth_client_id: String,
}

struct Inner {
    records: BTreeMap<String, Record>,
    /// The file could not be read; it is backed up before the first write.
    file_invalid: bool,
    /// The file could not be opened or read (a scanner or a sync tool held it): it is read again
    /// before it is used, and never written over unread.
    unread: bool,
}

/// The records of a read `profiles.json`.
fn records_of(map: serde_json::Map<String, Value>) -> BTreeMap<String, Record> {
    map.into_iter()
        .filter_map(|(key, value)| match value {
            Value::Object(record) => Some((key, record)),
            _ => None,
        })
        .collect()
}

pub struct ProfileStore {
    path: PathBuf,
    cipher: TokenCipher,
    inner: Mutex<Inner>,
}

fn is_default(record: &Record) -> bool {
    record.get("default").and_then(Value::as_bool).unwrap_or(false)
}

fn not_found(key: &str) -> AppError {
    AppError::new(ErrorCode::NotFound, "no such profile").with_param("profile_key", key)
}

impl ProfileStore {
    pub fn open(state_dir: &Path) -> ProfileStore {
        let path = state_dir.join(PROFILES_FILE);
        let store = ProfileStore {
            path,
            cipher: TokenCipher::new(state_dir),
            inner: Mutex::new(Inner { records: BTreeMap::new(), file_invalid: false, unread: true }),
        };
        store.migrate();
        store
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state, read from the file first if it could not be until now.
    fn loaded(&self) -> MutexGuard<'_, Inner> {
        let mut inner = self.lock();
        if inner.unread {
            match read_json_object(&self.path) {
                Ok(JsonRead::Object(map)) => (inner.records, inner.file_invalid) = (records_of(map), false),
                Ok(JsonRead::Missing) => (inner.records, inner.file_invalid) = (BTreeMap::new(), false),
                Ok(JsonRead::Invalid(reason)) => {
                    tracing::warn!("Ignoring unreadable {}: {reason}", self.path.display());
                    (inner.records, inner.file_invalid) = (BTreeMap::new(), true);
                }
                Err(e) => {
                    tracing::warn!("Unable to read {}: {e}", self.path.display());
                    return inner;
                }
            }
            inner.unread = false;
        }
        inner
    }

    /// Seals plaintext tokens and gives the profiles a default when they have none, in one write.
    /// On any failure the file and memory stay as loaded (plaintext tokens remain readable in memory).
    fn migrate(&self) {
        let mut inner = self.loaded();
        if inner.file_invalid || inner.unread {
            return;
        }
        let mut candidate = inner.records.clone();
        let mut changed = false;
        for record in candidate.values_mut() {
            for field in TOKEN_FIELDS {
                let plain = match record.get(field) {
                    Some(Value::String(v)) if v != OFFLINE_TOKEN && !v.starts_with(ENC_PREFIX) => v.clone(),
                    _ => continue,
                };
                match self.cipher.encrypt(&plain) {
                    Ok(sealed) => {
                        record.insert(field.to_string(), Value::String(sealed));
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!("Cannot seal stored tokens: {}", e.detail);
                        return;
                    }
                }
            }
        }
        // No default (the original launcher could leave several so): the first by name is, as
        // after a delete.
        if !candidate.values().any(is_default)
            && let Some(first) = candidate.keys().min_by_key(|k| k.to_lowercase()).cloned()
            && let Some(record) = candidate.get_mut(&first)
        {
            record.insert("default".to_string(), json!(true));
            changed = true;
        }
        if changed {
            match self.write(&mut inner, &candidate) {
                Ok(()) => inner.records = candidate,
                Err(e) => tracing::warn!("Profiles migration was not saved: {}", e.detail),
            }
        }
    }

    fn write(&self, inner: &mut Inner, candidate: &BTreeMap<String, Record>) -> AppResult<()> {
        let failed = |e: std::io::Error| {
            tracing::error!("Unable to save {}: {e}", self.path.display());
            AppError::new(ErrorCode::ProfileSaveFailed, e.to_string())
        };
        if inner.file_invalid && self.path.is_file() {
            let backup = backup_corrupt_file(&self.path).map_err(failed)?;
            tracing::warn!("Kept the unreadable profiles file as {}", backup.display());
        }
        let value = Value::Object(
            candidate.iter().map(|(key, record)| (key.clone(), Value::Object(record.clone()))).collect(),
        );
        write_json_file(&self.path, &value, 4).map_err(failed)?;
        inner.file_invalid = false;
        Ok(())
    }

    /// Applies `change` to a copy, writes it, and only then replaces the in-memory state.
    fn mutate(&self, change: impl FnOnce(&mut BTreeMap<String, Record>) -> AppResult<()>) -> AppResult<()> {
        let mut inner = self.loaded();
        if inner.unread {
            return Err(AppError::new(
                ErrorCode::ProfileSaveFailed,
                format!("{} could not be read; it is not written over", self.path.display()),
            ));
        }
        let mut candidate = inner.records.clone();
        change(&mut candidate)?;
        self.write(&mut inner, &candidate)?;
        inner.records = candidate;
        Ok(())
    }

    /// Writes both token fields; if sealing is impossible stores none and marks the profile.
    fn seal_tokens(&self, record: &mut Record, access: Option<&str>, refresh: Option<&str>) -> AppResult<()> {
        let seal = |token: Option<&str>| -> AppResult<Value> {
            match token {
                None => Ok(Value::Null),
                Some(OFFLINE_TOKEN) => Ok(json!(OFFLINE_TOKEN)),
                Some(token) => self.cipher.encrypt(token).map(Value::String),
            }
        };
        match (seal(access), seal(refresh)) {
            (Ok(access), Ok(refresh)) => {
                record.insert("access_token".to_string(), access);
                record.insert("refresh_token".to_string(), refresh);
                Ok(())
            }
            (Err(e), _) | (_, Err(e)) => {
                record.insert("access_token".to_string(), Value::Null);
                record.insert("refresh_token".to_string(), Value::Null);
                record.insert("reauth_required".to_string(), json!(true));
                record.insert("reauth_reason".to_string(), json!(REASON_ENCRYPTION_UNAVAILABLE));
                Err(e)
            }
        }
    }

    /// Adds `key` (replacing an existing one only when `replace`) and makes it the default.
    /// If the tokens cannot be sealed the profile is saved without them and the error returned.
    pub fn save(&self, key: &str, profile: NewProfile, replace: bool) -> AppResult<StoredProfile> {
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(AppError::new(ErrorCode::ProfileNameInvalid, "empty profile name"));
        }
        let mut record = Record::new();
        record.insert(
            "id".to_string(),
            json!(profile.id.clone().unwrap_or_else(|| offline_uuid(&profile.name))),
        );
        record.insert("name".to_string(), json!(profile.name));
        if let Some(xuid) = &profile.xuid {
            record.insert("xuid".to_string(), json!(xuid));
        }
        if let Some(expires_at) = profile.expires_at {
            record.insert("expires_at".to_string(), json!(expires_at));
        }
        if let Some(client) = &profile.auth_client_id {
            record.insert("auth_client_id".to_string(), json!(client));
        }
        let sealed =
            self.seal_tokens(&mut record, profile.access_token.as_deref(), profile.refresh_token.as_deref());
        self.mutate(|all| {
            if !replace && all.contains_key(&key) {
                return Err(AppError::new(ErrorCode::ProfileExists, "a profile with this name exists")
                    .with_param("name", &key));
            }
            for other in all.values_mut() {
                other.insert("default".to_string(), json!(false));
            }
            record.insert("default".to_string(), json!(true));
            all.insert(key.clone(), record);
            Ok(())
        })?;
        sealed?;
        self.get(&key).ok_or_else(|| AppError::internal("the saved profile is missing"))
    }

    /// Stores refreshed tokens and clears the reauth markers.
    pub fn update_tokens(&self, key: &str, update: &TokenUpdate) -> AppResult<StoredProfile> {
        let access = self.cipher.encrypt(&update.access_token);
        let refresh = update.refresh_token.as_deref().map(|t| self.cipher.encrypt(t)).transpose();
        let (access, refresh) = match (access, refresh) {
            (Ok(access), Ok(refresh)) => (access, refresh),
            (Err(e), _) | (_, Err(e)) => {
                self.mark_reauth(key, REASON_ENCRYPTION_UNAVAILABLE)?;
                return Err(e);
            }
        };
        self.mutate(|all| {
            let record = all.get_mut(key).ok_or_else(|| not_found(key))?;
            record.insert("id".to_string(), json!(update.id));
            record.insert("name".to_string(), json!(update.name));
            record.insert("access_token".to_string(), json!(access));
            if let Some(refresh) = refresh {
                record.insert("refresh_token".to_string(), json!(refresh));
            }
            record.insert("xuid".to_string(), json!(update.xuid));
            record.insert("expires_at".to_string(), json!(update.expires_at));
            record.insert("auth_client_id".to_string(), json!(update.auth_client_id));
            record.remove("reauth_required");
            record.remove("reauth_reason");
            Ok(())
        })?;
        self.get(key).ok_or_else(|| not_found(key))
    }

    pub fn mark_reauth(&self, key: &str, reason: &str) -> AppResult<()> {
        self.mutate(|all| {
            let record = all.get_mut(key).ok_or_else(|| not_found(key))?;
            record.insert("reauth_required".to_string(), json!(true));
            record.insert("reauth_reason".to_string(), json!(reason));
            Ok(())
        })
    }

    /// Removes a profile; when no default remains the first by name becomes the default
    /// (the original left none).
    pub fn delete(&self, key: &str) -> AppResult<()> {
        self.mutate(|all| {
            all.remove(key).ok_or_else(|| not_found(key))?;
            if !all.values().any(is_default)
                && let Some(next) = all.keys().min_by_key(|k| k.to_lowercase()).cloned()
                && let Some(record) = all.get_mut(&next)
            {
                record.insert("default".to_string(), json!(true));
            }
            Ok(())
        })
    }

    pub fn set_default(&self, key: &str) -> AppResult<()> {
        self.mutate(|all| {
            if !all.contains_key(key) {
                return Err(not_found(key));
            }
            for (name, record) in all.iter_mut() {
                record.insert("default".to_string(), json!(name == key));
            }
            Ok(())
        })
    }

    pub fn get(&self, key: &str) -> Option<StoredProfile> {
        let inner = self.loaded();
        inner.records.get(key).map(|record| self.view(key, record))
    }

    pub fn list(&self) -> Vec<StoredProfile> {
        let inner = self.loaded();
        inner.records.iter().map(|(key, record)| self.view(key, record)).collect()
    }

    pub fn default_profile(&self) -> Option<StoredProfile> {
        self.list().into_iter().find(|p| p.is_default)
    }

    pub fn snapshot(&self) -> ProfilesSnapshot {
        let mut profiles: Vec<ProfileDto> = self.list().iter().map(StoredProfile::to_dto).collect();
        sort_profiles(&mut profiles);
        ProfilesSnapshot { profiles }
    }

    fn view(&self, key: &str, record: &Record) -> StoredProfile {
        let text = |field: &str| record.get(field).and_then(Value::as_str).map(str::to_string);
        let mut decryption_failed = false;
        let mut sealed = false;
        let mut token = |field: &str| match text(field) {
            Some(value) if value.starts_with(ENC_PREFIX) => {
                sealed = true;
                let plain = self.cipher.decrypt(&value);
                decryption_failed |= plain.is_none();
                plain
            }
            other => other,
        };
        let access_token = token("access_token");
        let refresh_token = token("refresh_token");
        let key_unavailable = decryption_failed && sealed && !self.cipher.key_available();
        let decryption_failed = decryption_failed && !key_unavailable;
        let name = text("name").unwrap_or_else(|| key.to_string());
        let offline = text("type").as_deref() == Some(OFFLINE_TOKEN)
            || access_token.as_deref() == Some(OFFLINE_TOKEN)
            || refresh_token.as_deref() == Some(OFFLINE_TOKEN);
        StoredProfile {
            key: key.to_string(),
            id: text("id").unwrap_or_else(|| offline_uuid(&name)),
            kind: if offline { AccountKind::Offline } else { AccountKind::Microsoft },
            access_token,
            refresh_token,
            xuid: text("xuid"),
            expires_at: record
                .get("expires_at")
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))),
            auth_client_id: text("auth_client_id"),
            is_default: is_default(record),
            marked: record.get("reauth_required").and_then(Value::as_bool).unwrap_or(false),
            stored_reason: text("reauth_reason"),
            decryption_failed,
            key_unavailable,
            name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::crypto::KEY_FILE;
    use std::fs;
    use std::sync::Arc;

    fn microsoft(name: &str) -> NewProfile {
        NewProfile {
            id: Some("0123456789abcdef0123456789abcdef".into()),
            name: name.into(),
            access_token: Some("mc-access".into()),
            refresh_token: Some("ms-refresh".into()),
            xuid: Some("2535400000000001".into()),
            expires_at: Some(4_000_000_000),
            auth_client_id: Some("client".into()),
        }
    }

    fn file(dir: &Path) -> String {
        fs::read_to_string(dir.join(PROFILES_FILE)).unwrap()
    }

    /// A scanner or a sync tool held the file at start: it is no corrupt file, and the profiles it
    /// holds are never written over.
    #[cfg(windows)]
    #[test]
    fn a_file_unreadable_for_a_moment_at_start_keeps_its_profiles() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        ProfileStore::open(dir.path()).save("Notch_UA", microsoft("Notch_UA"), true).unwrap();
        let held =
            fs::OpenOptions::new().read(true).share_mode(0).open(dir.path().join(PROFILES_FILE)).unwrap();
        let store = ProfileStore::open(dir.path());
        assert!(store.list().is_empty(), "nothing could be read");
        assert!(store.save("Alex", NewProfile::offline("Alex"), false).is_err(), "no write while unread");
        drop(held);
        store.save("Alex", NewProfile::offline("Alex"), false).unwrap();
        let names: Vec<String> = ProfileStore::open(dir.path()).list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Alex", "Notch_UA"], "the profiles from before are kept");
        let backups = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .count();
        assert_eq!(backups, 0, "a file that was only busy is no corrupt file");
    }

    #[test]
    fn profiles_with_no_default_get_one_at_start() {
        // The original launcher could leave several profiles and none marked: Play would ask for one.
        let dir = tempfile::tempdir().unwrap();
        let record = |name: &str| json!({"name": name, "type": "offline", "access_token": "offline"});
        let all = json!({"steve": record("steve"), "Alex": record("Alex"), "zed": record("zed")});
        fs::write(dir.path().join(PROFILES_FILE), all.to_string()).unwrap();
        let store = ProfileStore::open(dir.path());
        assert_eq!(
            store.default_profile().map(|p| p.key).as_deref(),
            Some("Alex"),
            "first by name, any case"
        );
        assert!(file(dir.path()).contains("\"default\": true"), "and it is saved");
    }

    #[test]
    fn saved_tokens_are_sealed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        let saved = store.save("Notch_UA", microsoft("Notch_UA"), true).unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("ms-refresh"));
        assert!(saved.is_default && !saved.is_offline() && saved.reauth_reason().is_none());
        let text = file(dir.path());
        assert!(!text.contains("ms-refresh") && !text.contains("mc-access"), "{text}");
        assert_eq!(text.matches("enc::").count(), 2);
        let reopened = ProfileStore::open(dir.path());
        assert_eq!(reopened.get("Notch_UA").unwrap().access_token.as_deref(), Some("mc-access"));
    }

    #[test]
    fn new_profile_becomes_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        store.save("Alex", NewProfile::offline("Alex"), false).unwrap();
        store.save("Mia", NewProfile::offline("Mia"), false).unwrap();
        assert!(store.get("Mia").unwrap().is_default);
        assert!(!store.get("Alex").unwrap().is_default);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.profiles[0].key, "Mia");
        assert_eq!(snapshot.profiles[0].id, crate::auth::offline::offline_uuid("Mia"));
    }

    #[test]
    fn deleting_the_default_promotes_another() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        for name in ["zed", "Alex", "Mia"] {
            store.save(name, NewProfile::offline(name), false).unwrap();
        }
        store.delete("Mia").unwrap();
        assert!(store.get("Alex").unwrap().is_default);
        assert_eq!(store.delete("nobody").unwrap_err().code, ErrorCode::NotFound);
        store.set_default("zed").unwrap();
        assert!(store.get("zed").unwrap().is_default && !store.get("Alex").unwrap().is_default);
    }

    #[test]
    fn a_lone_profile_is_promoted_on_load() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(PROFILES_FILE),
            r#"{"Solo": {"name": "Solo", "access_token": "offline", "refresh_token": "offline"}}"#,
        )
        .unwrap();
        let store = ProfileStore::open(dir.path());
        assert!(store.get("Solo").unwrap().is_default);
        assert!(file(dir.path()).contains("\"default\": true"));
    }

    #[test]
    fn plaintext_tokens_are_sealed_on_load() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(PROFILES_FILE),
            r#"{"Notch_UA": {"id": "0123456789abcdef0123456789abcdef", "name": "Notch_UA",
               "access_token": "plain-access", "refresh_token": "plain-refresh", "default": true}}"#,
        )
        .unwrap();
        let store = ProfileStore::open(dir.path());
        let text = file(dir.path());
        assert!(!text.contains("plain-refresh") && text.contains("enc::"), "{text}");
        assert_eq!(store.get("Notch_UA").unwrap().refresh_token.as_deref(), Some("plain-refresh"));
    }

    #[test]
    fn unreadable_ciphertext_needs_sign_in_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let foreign = TokenCipher::new(other.path()).encrypt("x").unwrap();
        let json = serde_json::json!({
            "Notch_UA": {"name": "Notch_UA", "access_token": foreign, "refresh_token": foreign, "default": true},
            "Steve": {"name": "Steve", "access_token": "offline", "refresh_token": "offline"}
        });
        fs::write(dir.path().join(PROFILES_FILE), json.to_string()).unwrap();
        let store = ProfileStore::open(dir.path());
        let profile = store.get("Notch_UA").unwrap();
        assert_eq!(profile.access_token, None);
        assert_eq!(profile.reauth_reason().as_deref(), Some(REASON_DECRYPTION_FAILED));
        assert!(profile.to_dto().reauth_required);
        store.set_default("Steve").unwrap();
        assert!(file(dir.path()).contains(&foreign), "the ciphertext survives unrelated edits");
    }

    #[test]
    fn encryption_failure_never_writes_tokens() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(KEY_FILE)).unwrap();
        let store = ProfileStore::open(dir.path());
        let err = store.save("Notch_UA", microsoft("Notch_UA"), true).unwrap_err();
        assert_eq!(err.code, ErrorCode::CredentialStorageUnavailable);
        let text = file(dir.path());
        assert!(!text.contains("ms-refresh") && !text.contains("mc-access"), "{text}");
        let saved = store.get("Notch_UA").unwrap();
        assert_eq!(saved.reauth_reason().as_deref(), Some(REASON_ENCRYPTION_UNAVAILABLE));
    }

    #[test]
    fn failed_save_keeps_memory_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        fs::create_dir(dir.path().join(PROFILES_FILE)).unwrap();
        let err = store.save("Steve", NewProfile::offline("Steve"), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProfileSaveFailed);
        assert!(store.list().is_empty());
    }

    #[test]
    fn corrupt_file_is_backed_up_before_the_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROFILES_FILE);
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let store = ProfileStore::open(dir.path());
        assert!(store.list().is_empty());
        assert_eq!(fs::read(&path).unwrap(), [0xff, 0xfe, 0x00], "not rewritten on load");
        store.save("Steve", NewProfile::offline("Steve"), false).unwrap();
        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("profiles.json.corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), [0xff, 0xfe, 0x00]);
    }

    #[test]
    fn existing_name_is_not_replaced_by_an_offline_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        store.save("Notch_UA", microsoft("Notch_UA"), true).unwrap();
        let err = store.save("Notch_UA", NewProfile::offline("Notch_UA"), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProfileExists);
        assert_eq!(err.params.get("name").map(String::as_str), Some("Notch_UA"));
        assert_eq!(store.get("Notch_UA").unwrap().refresh_token.as_deref(), Some("ms-refresh"));
    }

    #[test]
    fn concurrent_changes_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ProfileStore::open(dir.path()));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    let name = format!("P{i}");
                    store.save(&name, NewProfile::offline(&name), false).unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(ProfileStore::open(dir.path()).list().len(), 8);
    }

    #[test]
    fn tokens_update_and_reauth_marks() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path());
        store.save("Notch_UA", microsoft("Notch_UA"), true).unwrap();
        store.mark_reauth("Notch_UA", REASON_REFRESH_INVALID).unwrap();
        let marked = store.get("Notch_UA").unwrap();
        assert!(marked.is_marked());
        assert_eq!(marked.to_dto().reauth_reason.as_deref(), Some(REASON_REFRESH_INVALID));
        let update = TokenUpdate {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Notch_UA".into(),
            access_token: "mc-access-2".into(),
            refresh_token: None,
            xuid: Some("2535400000000001".into()),
            expires_at: 5_000_000_000,
            auth_client_id: "client".into(),
        };
        let updated = store.update_tokens("Notch_UA", &update).unwrap();
        assert_eq!(updated.access_token.as_deref(), Some("mc-access-2"));
        assert_eq!(updated.refresh_token.as_deref(), Some("ms-refresh"), "kept when none is returned");
        assert!(updated.reauth_reason().is_none() && !updated.is_marked());
        assert!(updated.is_fresh(4_999_999_000) && !updated.is_fresh(4_999_999_800));
    }
}
