//! The auth servers accounts can be on besides Microsoft (`auth_servers.json`): where each one's
//! API is and its metadata, under an id the accounts name. Ely.by is a preset found the first time
//! it is needed; a module (Tensa) registers its own and may move it to another address later,
//! keeping its id and so its accounts. The player never types an address (owner, 2026-10-30).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::http::AuthHttp;
use super::now_secs;
use super::yggdrasil::{self, AuthServerInfo};
use crate::storage::json::{JsonRead, read_json_object, write_json_file};

pub const SERVERS_FILE: &str = "auth_servers.json";
pub const ELY_BY: &str = "elyby";

/// Who put a server in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "module")]
pub enum Origin {
    Preset,
    Module(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthServer {
    pub id: String,
    pub name: String,
    /// What it was found by (a site, or the API root).
    pub address: String,
    pub api_root: String,
    /// The metadata as the server sent it, for the game's prefetch.
    pub metadata: String,
    pub fetched_at: i64,
    pub origin: Origin,
}

/// A server the launcher knows before it is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: String,
    pub address: String,
}

/// The presets of a real launcher.
pub fn presets() -> Vec<Preset> {
    vec![Preset { id: ELY_BY, name: "Ely.by".into(), address: "ely.by".into() }]
}

pub struct AuthServers {
    path: PathBuf,
    presets: Vec<Preset>,
    known: Mutex<BTreeMap<String, AuthServer>>,
}

impl AuthServers {
    pub fn open(state_dir: &Path, presets: Vec<Preset>) -> AuthServers {
        let path = state_dir.join(SERVERS_FILE);
        let known = match read_json_object(&path) {
            Ok(JsonRead::Object(map)) => map
                .into_iter()
                .filter_map(|(id, value)| Some((id, serde_json::from_value::<AuthServer>(value).ok()?)))
                .collect(),
            Ok(JsonRead::Missing) => BTreeMap::new(),
            Ok(JsonRead::Invalid(reason)) => {
                tracing::warn!("Ignoring unreadable {}: {reason}", path.display());
                BTreeMap::new()
            }
            Err(e) => {
                tracing::warn!("Unable to read {}: {e}", path.display());
                BTreeMap::new()
            }
        };
        AuthServers { path, presets, known: Mutex::new(known) }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, AuthServer>> {
        self.known.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<AuthServer> {
        self.lock().get(id).cloned()
    }

    pub fn list(&self) -> Vec<AuthServer> {
        self.lock().values().cloned().collect()
    }

    fn keep(&self, server: AuthServer) -> AppResult<AuthServer> {
        let mut known = self.lock();
        let mut next = known.clone();
        next.insert(server.id.clone(), server.clone());
        let value = Value::Object(
            next.iter().map(|(id, s)| (id.clone(), serde_json::to_value(s).unwrap_or(Value::Null))).collect(),
        );
        write_json_file(&self.path, &value, 2).map_err(|e| {
            AppError::new(ErrorCode::ProfileSaveFailed, format!("{}: {e}", self.path.display()))
        })?;
        *known = next;
        Ok(server)
    }

    fn of(id: &str, address: &str, info: AuthServerInfo, origin: Origin) -> AuthServer {
        AuthServer {
            id: id.to_string(),
            name: info.name,
            address: address.to_string(),
            api_root: info.api_root,
            metadata: info.metadata,
            fetched_at: now_secs(),
            origin,
        }
    }

    /// The server with `id`: known, or a preset found now.
    pub async fn ensure(&self, http: &AuthHttp, id: &str) -> AppResult<AuthServer> {
        if let Some(known) = self.get(id) {
            return Ok(known);
        }
        let preset = self.presets.iter().find(|p| p.id == id).cloned().ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("no auth server {id}")).with_param("server", id)
        })?;
        let info = yggdrasil::resolve(http, &preset.address).await?;
        self.keep(Self::of(id, &preset.address, info, Origin::Preset))
    }

    /// The server `id` at `address` (a module's), found now: a new address keeps the id, so the
    /// accounts on it stay.
    pub async fn register(
        &self,
        http: &AuthHttp,
        id: &str,
        address: &str,
        origin: Origin,
    ) -> AppResult<AuthServer> {
        let info = yggdrasil::resolve(http, address).await?;
        self.keep(Self::of(id, address.trim(), info, origin))
    }

    /// Asks the server for its metadata again (before a launch); the kept copy stays when it does
    /// not answer, so a server down for a moment does not stop the game.
    pub async fn refreshed(&self, http: &AuthHttp, id: &str) -> AppResult<AuthServer> {
        let known = self.ensure(http, id).await?;
        match yggdrasil::resolve(http, &known.address).await {
            Ok(info) => self.keep(Self::of(id, &known.address, info, known.origin)),
            Err(e) => {
                tracing::warn!("Auth server {id} did not answer, its kept metadata is used: {}", e.detail);
                Ok(known)
            }
        }
    }
}
