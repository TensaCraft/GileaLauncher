//! Test fixture for the update helper (never shipped). Started normally it writes its tag into
//! `fixture-out.txt` next to itself; with `--sleep-ms N` it only sleeps (a process to wait for).
//! The tag comes from `../Resources/fixture-tag.txt` (macOS bundles) or from `UAFIXTURE:<tag>`
//! appended to the executable file.

use std::path::Path;
use std::time::Duration;

fn appended_tag(bytes: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"UAFIXTURE:";
    let at = bytes.windows(MARK.len()).rposition(|w| w == MARK)?;
    let rest = &bytes[at + MARK.len()..];
    let end = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).trim().to_string())
}

fn bundle_tag(exe: &Path) -> Option<String> {
    let file = exe.parent()?.parent()?.join("Resources").join("fixture-tag.txt");
    std::fs::read_to_string(file).ok().map(|t| t.trim().to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--sleep-ms") {
        let ms = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(1000);
        std::thread::sleep(Duration::from_millis(ms));
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let tag = bundle_tag(&exe)
        .or_else(|| std::fs::read(&exe).ok().and_then(|b| appended_tag(&b)))
        .unwrap_or_else(|| "untagged".into());
    if let Some(dir) = exe.parent() {
        let _ = std::fs::write(dir.join("fixture-out.txt"), tag);
    }
}
