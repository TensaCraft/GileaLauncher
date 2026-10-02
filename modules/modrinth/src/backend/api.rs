//! Modrinth's REST API v2: search, projects, versions and identifying files by
//! their hashes.

use std::time::Duration;

use launcher_core::net::api_client;
use launcher_shared::branding::{APP_NAME, SUPPORT_URL, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Url};
use serde_json::{Map, Value, json};

pub const BASE: &str = "https://api.modrinth.com/v2";
const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);

/// Modrinth asks clients to name themselves and a contact.
pub fn user_agent() -> String {
    format!("{APP_NAME}/{VERSION} ({SUPPORT_URL})")
}

/// Each hash once, in order.
fn unique(hashes: &[String]) -> Vec<&str> {
    let mut unique: Vec<&str> = Vec::new();
    for hash in hashes {
        if !unique.contains(&hash.as_str()) {
            unique.push(hash.as_str());
        }
    }
    unique
}

fn network(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Network, format!("Modrinth: {e}"))
}

pub struct ModrinthApi {
    client: Client,
    base: String,
}

impl ModrinthApi {
    pub fn new(base: &str) -> AppResult<ModrinthApi> {
        Ok(ModrinthApi {
            client: api_client(&user_agent(), CONNECT, READ)?,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    /// The API address of `segments` (each escaped), with `query` when it has pairs.
    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> AppResult<Url> {
        let mut url = Url::parse(&self.base).map_err(network)?;
        url.path_segments_mut().map_err(|()| network("the API address cannot have a path"))?.extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    async fn answer(response: reqwest::Response) -> AppResult<Value> {
        let status = response.status();
        if !status.is_success() {
            return Err(network(format!("HTTP {status}")).with_param("status", status.as_u16().to_string()));
        }
        let body = response.bytes().await.map_err(network)?;
        serde_json::from_slice(&body).map_err(network)
    }

    async fn get(&self, url: Url) -> AppResult<Value> {
        Self::answer(self.client.get(url).send().await.map_err(network)?).await
    }

    async fn post(&self, url: Url, body: &Value) -> AppResult<Value> {
        let request = self.client.post(url).header(CONTENT_TYPE, "application/json").body(body.to_string());
        Self::answer(request.send().await.map_err(network)?).await
    }

    /// `GET /version/{id}`.
    pub async fn version(&self, id: &str) -> AppResult<Value> {
        self.get(self.url(&["version", id], &[])?).await
    }

    /// `GET /project/{id}`.
    pub async fn project(&self, id: &str) -> AppResult<Value> {
        self.get(self.url(&["project", id], &[])?).await
    }

    /// `GET /projects?ids=[…]`: the projects Modrinth knows among `ids`, asked 100 at a time.
    pub async fn projects(&self, ids: &[String]) -> AppResult<Vec<Value>> {
        let mut found = Vec::new();
        for batch in unique(ids).chunks(100) {
            let ids = serde_json::to_string(batch).map_err(network)?;
            if let Value::Array(projects) = self.get(self.url(&["projects"], &[("ids", ids)])?).await? {
                found.extend(projects);
            }
        }
        Ok(found)
    }

    /// `POST /version_files`: the version of each SHA-512 digest Modrinth knows, asked 100 at a time.
    pub async fn versions_by_hashes(&self, hashes: &[String]) -> AppResult<Map<String, Value>> {
        let mut found = Map::new();
        for batch in unique(hashes).chunks(100) {
            let body = json!({"hashes": batch, "algorithm": "sha512"});
            if let Value::Object(answer) = self.post(self.url(&["version_files"], &[])?, &body).await? {
                found.extend(answer);
            }
        }
        Ok(found)
    }

    /// `POST /version_files/update`: for each SHA-512, its project's newest version for `loaders` ×
    /// `game_versions` (a filter left empty is not sent), asked 100 at a time.
    pub async fn latest_versions(
        &self,
        hashes: &[String],
        loaders: &[&str],
        game_versions: &[&str],
    ) -> AppResult<Map<String, Value>> {
        let mut found = Map::new();
        for batch in unique(hashes).chunks(100) {
            let mut body = json!({"hashes": batch, "algorithm": "sha512"});
            if !loaders.is_empty() {
                body["loaders"] = json!(loaders);
            }
            if !game_versions.is_empty() {
                body["game_versions"] = json!(game_versions);
            }
            if let Value::Object(answer) =
                self.post(self.url(&["version_files", "update"], &[])?, &body).await?
            {
                found.extend(answer);
            }
        }
        Ok(found)
    }

    /// `GET /search`, by relevance.
    pub async fn search(&self, query: &str, facets: &str, offset: u32, limit: u32) -> AppResult<Value> {
        let pairs = [
            ("query", query.to_string()),
            ("facets", facets.to_string()),
            ("index", "relevance".to_string()),
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
        ];
        self.get(self.url(&["search"], &pairs)?).await
    }

    /// `GET /project/{id}/version`, narrowed to a loader and a Minecraft version when given.
    pub async fn project_versions(
        &self,
        project_id: &str,
        loader: Option<&str>,
        game_version: Option<&str>,
    ) -> AppResult<Value> {
        let mut pairs = Vec::new();
        if let Some(loader) = loader {
            pairs.push(("loaders", serde_json::json!([loader]).to_string()));
        }
        if let Some(version) = game_version {
            pairs.push(("game_versions", serde_json::json!([version]).to_string()));
        }
        self.get(self.url(&["project", project_id, "version"], &pairs)?).await
    }
}
