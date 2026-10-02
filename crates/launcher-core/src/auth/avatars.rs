//! Player heads for profile cards, cached for a day.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::CONTENT_TYPE;
use sha2::{Digest, Sha256};

use crate::storage::atomic::atomic_write;

pub const AVATAR_SIZE: u32 = 64;
pub const AVATAR_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Avatar {
    Png(Vec<u8>),
    /// Nothing could be downloaded or cached: the webview may still load this address itself.
    Remote(String),
}

impl Avatar {
    /// For `<img src>`: a data URL for bytes, or the remote address.
    pub fn to_src(&self) -> String {
        match self {
            Avatar::Png(bytes) => format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
            Avatar::Remote(url) => url.clone(),
        }
    }
}

pub fn cache_file(dir: &Path, identifier: &str) -> PathBuf {
    let digest = hex::encode(Sha256::digest(identifier.as_bytes()));
    dir.join(format!("{}.png", &digest[..16]))
}

/// Percent-encodes a URL path segment (RFC 3986 unreserved characters stay as they are).
pub fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn is_fresh(file: &Path, ttl: Duration) -> bool {
    fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < ttl)
}

pub struct AvatarCache {
    dir: PathBuf,
    sources: Vec<String>,
    client: reqwest::Client,
    timeout: Duration,
    /// One download per identifier at a time; later callers find the fresh file.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AvatarCache {
    pub fn new(dir: PathBuf, sources: Vec<String>, client: reqwest::Client, timeout: Duration) -> Self {
        AvatarCache { dir, sources, client, timeout, locks: Mutex::new(HashMap::new()) }
    }

    fn lock_for(&self, identifier: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(identifier.to_string()).or_default().clone()
    }

    /// A fresh cached head, else the first host that returns an image, else the stale cache,
    /// else the last host's address.
    pub async fn get(&self, identifier: &str) -> Option<Avatar> {
        let identifier = match identifier.trim() {
            "" => "Steve",
            trimmed => trimmed,
        };
        let lock = self.lock_for(identifier);
        let _guard = lock.lock().await;
        let file = cache_file(&self.dir, identifier);
        if is_fresh(&file, AVATAR_TTL)
            && let Ok(bytes) = fs::read(&file)
            && !bytes.is_empty()
        {
            return Some(Avatar::Png(bytes));
        }
        let mut last_url = None;
        for template in &self.sources {
            let url = template
                .replace("{id}", &encode_segment(identifier))
                .replace("{size}", &AVATAR_SIZE.to_string());
            if let Some(bytes) = self.fetch(&url).await {
                if let Err(e) = atomic_write(&file, &bytes) {
                    tracing::debug!("Unable to cache an avatar: {e}");
                }
                return Some(Avatar::Png(bytes));
            }
            last_url = Some(url);
        }
        match fs::read(&file) {
            Ok(bytes) if !bytes.is_empty() => Some(Avatar::Png(bytes)),
            _ => last_url.map(Avatar::Remote),
        }
    }

    async fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let response = self.client.get(url).timeout(self.timeout).send().await.ok()?;
        let is_image = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("image/"));
        if response.status().as_u16() != 200 || !is_image {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (!bytes.is_empty()).then(|| bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_for_the_img_tag() {
        assert_eq!(Avatar::Png(vec![1, 2, 3]).to_src(), "data:image/png;base64,AQID");
        assert_eq!(Avatar::Remote("https://x/y".into()).to_src(), "https://x/y");
    }

    #[test]
    fn cache_names_are_stable_and_short() {
        let dir = Path::new("cache");
        let a = cache_file(dir, "Steve");
        assert_eq!(a, cache_file(dir, "Steve"));
        assert_ne!(a, cache_file(dir, "Alex"));
        assert_eq!(a.file_name().unwrap().to_string_lossy().len(), 16 + ".png".len());
    }

    #[test]
    fn identifiers_are_percent_encoded() {
        assert_eq!(encode_segment("Steve_1-a.b~"), "Steve_1-a.b~");
        assert_eq!(encode_segment("a b/ї"), "a%20b%2F%D1%97");
    }
}
