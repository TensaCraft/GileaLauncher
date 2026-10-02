//! Every download goes through the core's one downloader (`launcher_core::net::downloader`): no
//! other `Downloader` is built, and no other code streams a response body to disk by hand.

use std::fs;
use std::path::Path;

use crate::brand::repo_files;

/// Where a `Downloader` may be built: the app's one instance and the manual-check binaries.
const BUILDS_ONE: [&str; 3] = [
    "crates/launcher-core/src/core_app.rs",
    "crates/launcher-core/src/bin/install.rs",
    "crates/launcher-core/src/bin/launch.rs",
];

/// Where a response body may be read piece by piece: the downloader and the fake servers.
const STREAMS: [&str; 2] = ["crates/launcher-core/src/net/downloader.rs", "crates/mock-github/"];

fn is_test(rel: &str) -> bool {
    rel.contains("/tests/") || rel.starts_with("xtask/")
}

pub fn offenders(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for rel in repo_files(root).into_iter().filter(|r| r.ends_with(".rs") && !is_test(r)) {
        let Ok(text) = fs::read_to_string(root.join(&rel)) else { continue };
        if text.contains("Downloader::new(") && !BUILDS_ONE.contains(&rel.as_str()) {
            found.push(format!("{rel}: builds its own Downloader"));
        }
        if (text.contains(".chunk().await") || text.contains("bytes_stream()"))
            && !STREAMS.iter().any(|s| rel.starts_with(s))
        {
            found.push(format!("{rel}: streams a download by hand"));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_download_goes_through_the_core_downloader() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(offenders(root), Vec::<String>::new());
    }
}
