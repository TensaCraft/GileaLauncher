//! CurseForge's REST API (`api.curseforge.com/v1`): search, mods and their files. Every request
//! carries the launcher's key; once CurseForge refuses it, the API is off for the session.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use launcher_core::net::{api_client_within, send_patiently};
use launcher_shared::branding::{APP_NAME, SUPPORT_URL, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode, Url};
use serde_json::Value;

use super::key::{ApiKey, KEY_HEADER, keyed_host};

pub const BASE: &str = "https://api.curseforge.com";
/// Minecraft on CurseForge.
pub const GAME_ID: u32 = 432;
/// The largest page the API gives.
pub const PAGE: u32 = 50;
const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);
const PROVIDER: &str = "CurseForge";

/// A search of one class of projects.
#[derive(Debug, Clone, Copy)]
pub struct SearchQuery<'a> {
    pub class_id: u32,
    pub text: &'a str,
    pub game_version: Option<&'a str>,
    /// CurseForge's mod loader type.
    pub loader: Option<u32>,
    pub index: u32,
    pub page_size: u32,
}

pub fn user_agent() -> String {
    format!("{APP_NAME}/{VERSION} ({SUPPORT_URL})")
}

fn network(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Network, format!("{PROVIDER}: {e}"))
}

pub struct CurseForgeApi {
    client: Client,
    base: String,
    key: ApiKey,
    /// CurseForge refused the key: no request goes out for the rest of the session.
    rejected: Arc<AtomicBool>,
}

impl CurseForgeApi {
    pub fn new(base: &str, key: ApiKey) -> AppResult<CurseForgeApi> {
        Ok(CurseForgeApi {
            // The key rides in a header: no redirect may take it off CurseForge's own hosts.
            client: api_client_within(&user_agent(), CONNECT, READ, std::sync::Arc::new(keyed_host))?,
            base: base.trim_end_matches('/').to_string(),
            key,
            rejected: Arc::default(),
        })
    }

    /// CurseForge refused the launcher's key in this session.
    pub fn rejected(&self) -> bool {
        self.rejected.load(Ordering::Relaxed)
    }

    /// `GET /v1/mods/search`: one page, the most popular first.
    pub async fn search(&self, query: &SearchQuery<'_>) -> AppResult<Value> {
        let mut pairs = vec![
            ("gameId", GAME_ID.to_string()),
            ("classId", query.class_id.to_string()),
            ("sortField", "2".to_string()),
            ("sortOrder", "desc".to_string()),
            ("index", query.index.to_string()),
            ("pageSize", query.page_size.to_string()),
        ];
        let text = query.text.trim();
        if !text.is_empty() {
            pairs.push(("searchFilter", text.to_string()));
        }
        if let Some(version) = query.game_version {
            pairs.push(("gameVersion", version.to_string()));
        }
        if let Some(loader) = query.loader {
            pairs.push(("modLoaderType", loader.to_string()));
        }
        self.get("/v1/mods/search", &pairs).await
    }

    /// `GET /v1/mods/{id}`: the mod.
    pub async fn mod_info(&self, id: u64) -> AppResult<Value> {
        Ok(self.get(&format!("/v1/mods/{id}"), &[]).await?["data"].take())
    }

    /// `POST /v1/mods`: the mods of `ids` that exist.
    pub async fn mods(&self, ids: &[u64]) -> AppResult<Vec<Value>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let answer = self.post("/v1/mods", &serde_json::json!({ "modIds": ids })).await?;
        Ok(data_list(answer))
    }

    /// `GET /v1/mods/{id}/files`: every file that fits, page by page.
    pub async fn files(
        &self,
        mod_id: u64,
        game_version: Option<&str>,
        loader: Option<u32>,
    ) -> AppResult<Vec<Value>> {
        let mut files = Vec::new();
        loop {
            let mut pairs = vec![("index", files.len().to_string()), ("pageSize", PAGE.to_string())];
            if let Some(version) = game_version {
                pairs.push(("gameVersion", version.to_string()));
            }
            if let Some(loader) = loader {
                pairs.push(("modLoaderType", loader.to_string()));
            }
            let mut answer = self.get(&format!("/v1/mods/{mod_id}/files"), &pairs).await?;
            let total = answer["pagination"]["totalCount"].as_u64().unwrap_or(0) as usize;
            let page = data_list(answer["data"].take());
            let got = page.len();
            files.extend(page);
            // The API pages no further than its 10 000th result.
            if got == 0 || files.len() >= total || files.len() + PAGE as usize > 10_000 {
                return Ok(files);
            }
        }
    }

    /// `POST /v1/mods/files`: the files of `ids` that exist.
    pub async fn files_by_ids(&self, ids: &[u64]) -> AppResult<Vec<Value>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let answer = self.post("/v1/mods/files", &serde_json::json!({ "fileIds": ids })).await?;
        Ok(data_list(answer))
    }

    /// `POST /v1/fingerprints/432`: the files of these fingerprints (`exactMatches`).
    pub async fn fingerprints(&self, prints: &[u32]) -> AppResult<Value> {
        if prints.is_empty() {
            return Ok(serde_json::json!({ "exactMatches": [] }));
        }
        let path = format!("/v1/fingerprints/{GAME_ID}");
        Ok(self.post(&path, &serde_json::json!({ "fingerprints": prints })).await?["data"].take())
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> AppResult<Url> {
        let mut url = Url::parse(&format!("{}{path}", self.base)).map_err(network)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    /// Off for the session once the key was refused.
    fn usable(&self) -> AppResult<()> {
        if self.rejected() { Err(rejected()) } else { Ok(()) }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> AppResult<Value> {
        self.usable()?;
        let request = self.client.get(self.url(path, query)?).header(KEY_HEADER, self.key.header());
        self.answer(send_patiently(request).await.map_err(network)?).await
    }

    async fn post(&self, path: &str, body: &Value) -> AppResult<Value> {
        self.usable()?;
        let request = self
            .client
            .post(self.url(path, &[])?)
            .header(KEY_HEADER, self.key.header())
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        self.answer(send_patiently(request).await.map_err(network)?).await
    }

    async fn answer(&self, response: reqwest::Response) -> AppResult<Value> {
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            self.rejected.store(true, Ordering::Relaxed);
            tracing::warn!(
                "CurseForge refused the launcher's key (HTTP {status}); it is off for this session"
            );
            return Err(rejected());
        }
        if !status.is_success() {
            return Err(network(format!("HTTP {status}")).with_param("status", status.as_u16().to_string()));
        }
        let body = response.bytes().await.map_err(network)?;
        serde_json::from_slice(&body).map_err(network)
    }
}

/// The key was refused: CurseForge is off until the launcher has another.
fn rejected() -> AppError {
    AppError::new(ErrorCode::ProviderKeyRejected, "CurseForge refused the API key")
        .with_param("provider", PROVIDER)
}

/// The `data` list of an answer (or the answer itself when it is the list).
fn data_list(mut answer: Value) -> Vec<Value> {
    let list = if answer.is_array() { answer } else { answer["data"].take() };
    match list {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}
