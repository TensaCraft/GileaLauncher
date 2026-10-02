mod support;

use std::path::Path;
use std::time::Duration;

use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::updater::download::{self, DownloadRequest, sha256_file};
use launcher_core::updater::github::GithubClient;
use launcher_core::updater::select::{self, GhAsset, Platform};
use launcher_core::updater::version::Version;
use launcher_shared::ErrorCode;
use mock_github::{MockHandle, NewRelease, Scenario, add_release, sha256_hex};
use support::fake_files::{self, Served};

/// Quick retries; the 400 KB payload streams (and so resumes).
fn downloader() -> Downloader {
    Downloader::new(DownloaderConfig {
        retry_delay: Duration::from_millis(1),
        in_memory_up_to: 64 * 1024,
        ..DownloaderConfig::default()
    })
    .unwrap()
}

/// Partial files left in `dir`.
fn parts(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".part"))
                .collect()
        })
        .unwrap_or_default()
}

fn payload() -> Vec<u8> {
    (0..400_000u32).map(|i| (i % 253) as u8).collect()
}

async fn server(root: &Path, scenario: Scenario) -> MockHandle {
    let src = root.join("payload.bin");
    std::fs::write(&src, payload()).unwrap();
    let name = select::preferred_asset_name(&Platform::current());
    let new =
        NewRelease { version: "0.2.0", prerelease: false, notes: "Fixes", asset_name: &name, source: &src };
    add_release(root, new).unwrap();
    mock_github::start(root.to_path_buf(), scenario, 0).await.unwrap()
}

fn client(server: &MockHandle) -> GithubClient {
    GithubClient::new(&server.base_url, mock_github::REPO, "Launcher/test").unwrap()
}

async fn selected(c: &GithubClient) -> (GhAsset, String) {
    let releases = c.releases().await.unwrap();
    let candidate = select::pick_release(releases, &Version::parse("0.1.0").unwrap(), false).unwrap();
    let platform = Platform::current();
    let asset = match select::pick_asset(&candidate.release.assets, &platform) {
        Some(a) => a.clone(),
        None => select::pick_asset(&c.release_assets(candidate.release.id).await.unwrap(), &platform)
            .unwrap()
            .clone(),
    };
    let sha = asset.sha256().unwrap();
    (asset, sha)
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_scenario_downloads_and_verifies() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::Normal).await;
    let c = client(&srv);
    let (asset, sha) = selected(&c).await;
    let dest = root.path().join("dl").join(&asset.name);
    let last = (0, 0);
    let req =
        DownloadRequest { url: &asset.browser_download_url, dest: &dest, size: asset.size, sha256: &sha };
    let last = std::sync::Mutex::new(last);
    let path =
        download::download(&downloader(), &c, &req, |d, t| *last.lock().unwrap() = (d, t)).await.unwrap();
    assert_eq!(sha256_file(&path).unwrap(), sha256_hex(&payload()));
    assert_eq!(last.into_inner().unwrap(), (asset.size, asset.size));
    assert!(parts(dest.parent().unwrap()).is_empty());
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cut_update_continues_where_it_stopped() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::Drop).await;
    let c = client(&srv);
    let (asset, sha) = selected(&c).await;
    let dest = root.path().join("dl").join(&asset.name);
    let req =
        DownloadRequest { url: &asset.browser_download_url, dest: &dest, size: asset.size, sha256: &sha };
    let seen = std::sync::Mutex::new(Vec::new());
    download::download(&downloader(), &c, &req, |d, _| seen.lock().unwrap().push(d)).await.unwrap();
    let seen = seen.into_inner().unwrap();
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "the second attempt went on from the cut: {seen:?}");
    assert_eq!(seen.last(), Some(&asset.size));
    assert_eq!(sha256_file(&dest).unwrap(), sha);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_hash_deletes_the_file() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::BadHash).await;
    let c = client(&srv);
    let (asset, sha) = selected(&c).await;
    let dest = root.path().join("dl").join(&asset.name);
    let req =
        DownloadRequest { url: &asset.browser_download_url, dest: &dest, size: asset.size, sha256: &sha };
    let err = download::download(&downloader(), &c, &req, |_, _| {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::IntegrityMismatch);
    assert!(!dest.exists() && parts(dest.parent().unwrap()).is_empty());
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_maps_to_minutes() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::RateLimit).await;
    let err = client(&srv).releases().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::RateLimited);
    let minutes: u64 = err.params["minutes"].parse().unwrap();
    assert!((1..=10).contains(&minutes), "{minutes}");
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_error_is_a_network_error() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::ServerError).await;
    let err = client(&srv).releases().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert_eq!(err.params["status"], "500");
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn paged_assets_are_found_on_page_two() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::PagedAssets).await;
    let c = client(&srv);
    let releases = c.releases().await.unwrap();
    assert!(releases[0].assets.is_empty());
    let assets = c.release_assets(releases[0].id).await.unwrap();
    let name = select::preferred_asset_name(&Platform::current());
    assert!(assets.iter().any(|a| a.name == name));
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channels_skip_drafts_and_need_beta() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::Channels).await;
    let c = client(&srv);
    let current = Version::parse("0.1.0").unwrap();
    let stable = select::pick_release(c.releases().await.unwrap(), &current, false).unwrap();
    assert_eq!(stable.version.as_str(), "0.2.0");
    let beta = select::pick_release(c.releases().await.unwrap(), &current, true).unwrap();
    assert_eq!(beta.version.as_str(), "99.0.0-beta.1");
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_hosts_and_bad_settings_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let srv = server(root.path(), Scenario::Normal).await;
    let c = client(&srv);
    let dest = root.path().join("x.exe");
    let req = DownloadRequest {
        url: "https://evil.example.com/x.exe",
        dest: &dest,
        size: 1,
        sha256: &"0".repeat(64),
    };
    assert_eq!(
        download::download(&downloader(), &c, &req, |_, _| {}).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    let other = GithubClient::new(&srv.base_url, "other/repo", "Launcher/test").unwrap();
    assert_eq!(other.releases().await.unwrap_err().params["status"], "404");
    assert!(GithubClient::new("http://example.com", mock_github::REPO, "t").is_err());
    assert!(GithubClient::new("https://api.github.com", "not-a-repo", "t").is_err());
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_redirected_away_from_its_host_is_refused() {
    let host = fake_files::start().await;
    let elsewhere = fake_files::start().await;
    let payload = payload();
    elsewhere.put("Launcher.exe", Served { body: payload.clone(), ..Served::default() });
    host.put(
        "Launcher.exe",
        Served { redirect_to: Some(elsewhere.url("Launcher.exe")), ..Served::default() },
    );
    let c = GithubClient::new(&host.base, mock_github::REPO, "Launcher/test").unwrap();
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("dl").join("Launcher.exe");
    let url = host.url("Launcher.exe");
    let sha = sha256_hex(&payload);
    let req = DownloadRequest { url: &url, dest: &dest, size: payload.len() as u64, sha256: &sha };
    assert!(download::download(&downloader(), &c, &req, |_, _| {}).await.is_err());
    assert!(!dest.exists() && parts(dest.parent().unwrap()).is_empty());
}
