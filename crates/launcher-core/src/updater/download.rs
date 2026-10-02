//! The release asset's download, through the launcher's one downloader.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::Url;
use sha2::{Digest, Sha256};

use super::github::GithubClient;
use super::select::download_url_allowed;
use crate::net::downloader::{DownloadProgress, DownloadTask, Downloader, ExpectedHash, Origin};

pub struct DownloadRequest<'a> {
    pub url: &'a str,
    pub dest: &'a Path,
    pub size: u64,
    /// Lower-case hex SHA-256 from the release asset.
    pub sha256: &'a str,
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Downloads the release asset through the launcher's downloader: resumed after a cut, retried,
/// its size and SHA-256 checked; a file already there with that digest is kept.
pub async fn download(
    downloader: &Downloader,
    client: &GithubClient,
    req: &DownloadRequest<'_>,
    progress: impl Fn(u64, u64) + Send + Sync,
) -> AppResult<PathBuf> {
    let url = Url::parse(req.url).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
    if !download_url_allowed(client.api_base(), &url) {
        return Err(
            AppError::new(ErrorCode::InvalidInput, "download host is not allowed").with_param("url", req.url)
        );
    }
    // Redirects too must end at the release host.
    let api_base = client.api_base().clone();
    let origin = Origin::new(move |url| download_url_allowed(&api_base, url));
    let task = DownloadTask::new(req.url, req.dest)
        .size(req.size)
        .hash(ExpectedHash::sha256(req.sha256))
        .origin(origin);
    let total = req.size;
    let report = |p: DownloadProgress| {
        let done = if p.files_done == p.files_total { total } else { p.bytes_done };
        progress(done, total);
    };
    downloader.download_one(task, &report).await?;
    Ok(req.dest.to_path_buf())
}
