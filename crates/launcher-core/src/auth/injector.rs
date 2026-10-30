//! authlib-injector, the Java agent that points the game at another auth server: the release the
//! launcher uses, fetched through the core downloader and checked by its digest, and the arguments
//! a launch adds before the game's main class. Its licence (AGPL with the "AUTHLIB-INJECTOR"
//! exception) lets the launcher use the unmodified jar as an agent.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use launcher_shared::{AppError, AppResult, ErrorCode};

use super::service::AgentServer;
use crate::net::downloader::{DownloadTask, Downloader, ExpectedHash};

/// A release of the agent: where it can be had (in order) and what it must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub version: String,
    pub urls: Vec<String>,
    pub sha256: String,
    pub size: u64,
}

/// The release the launcher uses: 1.2.8 (2026-07-11), which knows Minecraft 26.3. Its own site
/// first, then the BMCLAPI mirror, which serves the same files.
pub fn pinned() -> Artifact {
    let file = "artifact/56/authlib-injector-1.2.8.jar";
    Artifact {
        version: "1.2.8".into(),
        urls: vec![
            format!("https://authlib-injector.yushi.moe/{file}"),
            format!("https://bmclapi2.bangbang93.com/mirrors/authlib-injector/{file}"),
        ],
        sha256: "9c7f4343e6c82034958ffb48c14a2cb0c85928be7283103ce17da00c6d5a7b10".into(),
        size: 349_681,
    }
}

/// The agent under `dir` (`agents/authlib-injector-<version>.jar`): kept when it is there with
/// the right digest, else fetched from the first address that gives the right file.
pub async fn ensure_jar(downloader: &Downloader, dir: &Path, artifact: &Artifact) -> AppResult<PathBuf> {
    let dest = dir.join("agents").join(format!("authlib-injector-{}.jar", artifact.version));
    let mut last = None;
    for url in &artifact.urls {
        let mut task = DownloadTask::new(url.clone(), &dest);
        task.size = Some(artifact.size);
        task.hash = Some(ExpectedHash::sha256(&artifact.sha256));
        match downloader.download_one(task, &|_| {}).await {
            Ok(()) => return Ok(dest),
            Err(e) => {
                tracing::warn!("authlib-injector from {url}: {}", e.detail);
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::new(ErrorCode::DownloadFailed, "authlib-injector has no address")))
}

/// Where the game can load `jar` from: the JVM reads `-javaagent:<jar>=<options>` up to the first
/// `=`, so a jar whose path has one is handed over from a copy in the temporary folder.
pub fn usable_path(jar: &Path) -> AppResult<PathBuf> {
    if !jar.to_string_lossy().contains('=') {
        return Ok(jar.to_path_buf());
    }
    let name = jar.file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("authlib-injector.jar"));
    let copy = std::env::temp_dir().join("launcher-agents").join(name);
    if copy.to_string_lossy().contains('=') {
        return Err(AppError::new(
            ErrorCode::Io,
            format!("the agent's path and the temporary folder both have '=': {}", jar.display()),
        ));
    }
    let io = |e: std::io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", copy.display()));
    std::fs::create_dir_all(copy.parent().unwrap_or(Path::new("."))).map_err(io)?;
    std::fs::copy(jar, &copy).map_err(io)?;
    Ok(copy)
}

/// The JVM arguments that load the agent for `server`, its metadata handed over so the game
/// starts without asking the server.
pub fn agent_arguments(jar: &Path, server: &AgentServer) -> Vec<String> {
    vec![
        format!("-javaagent:{}={}", jar.display(), server.api_root),
        format!("-Dauthlibinjector.yggdrasil.prefetched={}", STANDARD.encode(server.metadata.as_bytes())),
    ]
}
