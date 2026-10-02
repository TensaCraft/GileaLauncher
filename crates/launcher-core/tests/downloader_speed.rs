//! Download speed on loopback:
//! `cargo test -p launcher-core --release --test downloader_speed -- --ignored --nocapture`.
mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use launcher_core::net::downloader::{DownloadTask, Downloader, DownloaderConfig, ExpectedHash};
use sha1::{Digest, Sha1};
use support::fake_files::{self, FileServer, Served};

fn small_files(server: &FileServer, root: &Path, n: usize, delay: Option<Duration>) -> Vec<DownloadTask> {
    (0..n)
        .map(|i| {
            let body: Vec<u8> = (0..4096).map(|j| ((i * 7 + j) % 251) as u8).collect();
            let name = format!("s{i}");
            server.put(&name, Served { body: body.clone(), delay, ..Served::default() });
            DownloadTask::new(server.url(&name), root.join(format!("{:02x}", i % 256)).join(&name))
                .size(body.len() as u64)
                .hash(ExpectedHash::sha1(&hex::encode(Sha1::digest(&body))))
        })
        .collect()
}

async fn timed(label: &str, workers: usize, tasks: Vec<DownloadTask>, verify: bool) {
    let downloader = Downloader::new(DownloaderConfig { workers, ..DownloaderConfig::default() }).unwrap();
    let started = Instant::now();
    let report = downloader.download_all(tasks, verify, &|_| {}).await.unwrap();
    println!(
        "{label}: {:?} (downloaded {}, skipped {}, failed {})",
        started.elapsed(),
        report.downloaded,
        report.skipped,
        report.failed.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn download_speed() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let small = small_files(&server, &dir.path().join("a"), 3000, None);
    timed("3000 x 4 KiB, fresh, 16", 16, small.clone(), false).await;
    timed("3000 x 4 KiB, present, verify", 16, small, true).await;
    let far = small_files(&server, &dir.path().join("b"), 1000, Some(Duration::from_millis(20)));
    timed("1000 x 4 KiB, 20 ms away, 8", 8, far.clone(), false).await;
    let _ = std::fs::remove_dir_all(dir.path().join("b"));
    timed("1000 x 4 KiB, 20 ms away, 16", 16, far, false).await;
    let big: Vec<DownloadTask> = (0..4)
        .map(|i| {
            let body: Vec<u8> = (0..32 * 1024 * 1024).map(|j| ((i * 13 + j) % 253) as u8).collect();
            let name = format!("b{i}");
            server.put(&name, Served { body: body.clone(), ..Served::default() });
            DownloadTask::new(server.url(&name), dir.path().join("big").join(&name))
                .size(body.len() as u64)
                .hash(ExpectedHash::sha1(&hex::encode(Sha1::digest(&body))))
        })
        .collect();
    timed("4 x 32 MiB, fresh", 16, big, false).await;
}
