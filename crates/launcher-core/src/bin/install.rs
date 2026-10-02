//! Manual check against the real Mojang servers (tests never run it):
//! `cargo run -p launcher-core --bin install -- <minecraft dir> <version> [--verify]`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use launcher_core::java::runtime::JavaRuntimes;
use launcher_core::lock::Coordinator;
use launcher_core::minecraft::InstallProgress;
use launcher_core::minecraft::install::MinecraftInstaller;
use launcher_core::minecraft::manifest::MojangEndpoints;
use launcher_core::minecraft::platform::GamePlatform;
use launcher_core::net::downloader::{Downloader, DownloaderConfig};
use launcher_core::net::meta::{META_TTL, MetaClient};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(version)) = (args.first(), args.get(1)) else {
        eprintln!("usage: install <minecraft dir> <version> [--verify]");
        std::process::exit(2);
    };
    let verify = args.iter().any(|a| a == "--verify");
    let mc = PathBuf::from(dir);
    let platform = GamePlatform::current();
    let endpoints = MojangEndpoints::default();
    let meta = Arc::new(MetaClient::new(Duration::from_secs(30), META_TTL).expect("metadata client"));
    let files = Arc::new(Downloader::new(DownloaderConfig::default()).expect("downloader"));
    let java =
        Arc::new(JavaRuntimes::new(&mc, platform.clone(), endpoints.clone(), meta.clone(), files.clone()));
    let installer =
        MinecraftInstaller::new(&mc, platform, endpoints, meta, files, java, Arc::new(Coordinator::shared()));
    let started = Instant::now();
    let last = Mutex::new(None::<Instant>);
    let progress = |p: InstallProgress| {
        let mut last = last.lock().unwrap();
        if p.download.is_some() && last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *last = Some(Instant::now());
        match p.download {
            Some(d) => println!(
                "{:?} — {}/{} files, {}/{} bytes",
                p.status, d.files_done, d.files_total, d.bytes_done, d.bytes_total
            ),
            None => println!("{:?}", p.status),
        }
    };
    match installer.install(version, verify, &progress).await {
        Ok(done) => {
            println!(
                "installed {} in {:.1} s, java: {:?}",
                done.id,
                started.elapsed().as_secs_f64(),
                done.java
            );
            let check = installer.check(version);
            println!("valid: {} {:?}", check.valid, check.issues);
            if !check.valid {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
