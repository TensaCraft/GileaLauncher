//! CurseForge's files their authors keep from other apps, found elsewhere: the copy the user
//! downloaded by hand into their Downloads folder, else the same bytes on Modrinth (by SHA-1).
//! What is in neither the user downloads from the file's page (`launcher_shared::provider::HeldFile`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use launcher_core::content::held::{Wanted, find_copies};
use launcher_core::net::api_client;
use launcher_core::net::downloader::ExpectedHash;
use launcher_shared::AppResult;
use launcher_shared::provider::HeldFile;
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};

use super::api::user_agent;
use super::catalog::{InstallFile, Source, file_page, secure_url, text};

/// Modrinth's API.
pub const MODRINTH: &str = "https://api.modrinth.com";
const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);

/// Where held files are looked for.
#[derive(Debug, Clone, Default)]
pub struct Elsewhere {
    /// Modrinth's API; `None`: not asked.
    pub modrinth: Option<String>,
    /// The user's Downloads folder.
    pub downloads: Option<PathBuf>,
}

/// A held file as CurseForge describes it: what it is and how it is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub title: String,
    pub file_name: String,
    pub page: Option<String>,
    pub size: u64,
    pub sha1: String,
}

impl Held {
    /// CurseForge's file `file` of `project` that it gives no address for; `None` without a size
    /// or a SHA-1 to know a copy by.
    pub fn of(project: &Value, file: &Value) -> Option<Held> {
        let sha1 = file["hashes"].as_array()?.iter().find_map(|h| {
            let value = text(h, "value").trim().to_ascii_lowercase();
            (h["algo"].as_u64() == Some(1)
                && value.len() == 40
                && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(value)
        })?;
        let size = file["fileLength"].as_u64().filter(|s| *s > 0)?;
        let site = text(&project["links"], "websiteUrl");
        let title =
            Some(text(project, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| text(file, "fileName"));
        Some(Held {
            title,
            file_name: text(file, "fileName"),
            page: site
                .starts_with("https://")
                .then(|| file_page(&site, file["id"].as_u64().unwrap_or_default())),
            size,
            sha1,
        })
    }

    /// What the user downloads by hand.
    pub fn to_dto(&self) -> HeldFile {
        HeldFile {
            title: self.title.clone(),
            file_name: self.file_name.clone(),
            url: self.page.clone(),
            size: self.size,
            sha1: self.sha1.clone(),
        }
    }

    /// The file to install from where it was found.
    pub fn install_file(&self, from: Source) -> InstallFile {
        InstallFile {
            url: String::new(),
            filename: self.file_name.clone(),
            size: self.size,
            hash: ExpectedHash::sha1(&self.sha1),
            from,
        }
    }
}

pub struct Finder {
    client: Client,
    elsewhere: Elsewhere,
}

impl Finder {
    pub fn new(elsewhere: Elsewhere) -> AppResult<Finder> {
        Ok(Finder { client: api_client(&user_agent(), CONNECT, READ)?, elsewhere })
    }

    /// Where each of `held` is found, by its SHA-1: the user's copy first (no download), then
    /// Modrinth. Those found nowhere are left out.
    pub async fn find(&self, held: &[Held]) -> HashMap<String, InstallFile> {
        let mut found: HashMap<String, InstallFile> = HashMap::new();
        if held.is_empty() {
            return found;
        }
        if let Some(dir) = self.elsewhere.downloads.clone() {
            let wanted: Vec<(String, u64, String)> =
                held.iter().map(|h| (h.file_name.clone(), h.size, h.sha1.clone())).collect();
            let copies = tokio::task::spawn_blocking(move || {
                let wanted: Vec<Wanted<'_>> =
                    wanted.iter().map(|(name, size, sha1)| Wanted { name, size: *size, sha1 }).collect();
                find_copies(&dir, &wanted)
            })
            .await
            .unwrap_or_default();
            for (h, copy) in held.iter().zip(copies) {
                if let Some(path) = copy {
                    found.insert(h.sha1.clone(), h.install_file(Source::Copy(path)));
                }
            }
        }
        let rest: Vec<&Held> = held.iter().filter(|h| !found.contains_key(&h.sha1)).collect();
        if let (Some(base), false) = (self.elsewhere.modrinth.as_deref(), rest.is_empty()) {
            match self.on_modrinth(base, &rest).await {
                Ok(urls) => {
                    for h in rest {
                        if let Some(url) = urls.get(&h.sha1) {
                            found.insert(
                                h.sha1.clone(),
                                InstallFile { url: url.clone(), ..h.install_file(Source::Elsewhere) },
                            );
                        }
                    }
                }
                Err(e) => tracing::warn!("Modrinth cannot be asked for CurseForge's held files now: {e}"),
            }
        }
        found
    }

    /// Modrinth's address of each of `held` it has, by SHA-1 (`POST /v2/version_files`).
    async fn on_modrinth(&self, base: &str, held: &[&Held]) -> Result<HashMap<String, String>, String> {
        let hashes: Vec<&str> = held.iter().map(|h| h.sha1.as_str()).collect();
        let url = format!("{}/v2/version_files", base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(json!({"hashes": hashes, "algorithm": "sha1"}).to_string())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let answer: Value = serde_json::from_slice(&response.bytes().await.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut urls = HashMap::new();
        for h in held {
            let files = answer[h.sha1.as_str()]["files"].as_array().into_iter().flatten();
            let same = files
                .filter(|f| f["hashes"]["sha1"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(&h.sha1)))
                .filter(|f| f["size"].as_u64().is_none_or(|s| s == h.size))
                .map(|f| text(f, "url"))
                .find(|url| secure_url(url));
            if let Some(url) = same {
                urls.insert(h.sha1.clone(), url);
            }
        }
        Ok(urls)
    }
}
