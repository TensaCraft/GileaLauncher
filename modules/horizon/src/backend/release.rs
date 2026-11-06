//! Horizon's signed nightly build: its checksums (`SHA256SUMS`), the signature over them and the
//! archive of this computer's platform.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};

/// Where the newest nightly's files are (a tag that moves to each new nightly).
pub const NIGHTLY: &str = "https://github.com/horizon-rust/horizon/releases/download/nightly";
/// The newest nightly's release, whose title names its build.
pub const NIGHTLY_RELEASE: &str = "https://api.github.com/repos/horizon-rust/horizon/releases/tags/nightly";
/// The tag the channel's signature (`SHA256SUMS.sig`) names.
pub const NIGHTLY_TAG: &str = "nightly";

/// The public keys Horizon signs its releases with (their `crates/horizon-release-sign/release-keys.txt`).
const RELEASE_KEYS: [&str; 1] = ["MU4oiIQcCxkZvbwSjjXs7SySCuM6ywRQ3nGaJYZG0gw="];

/// Whether one of Horizon's keys signed checksums `sums` of release `tag`: `sig` is the base64
/// Ed25519 signature over `horizon release <tag>\n` followed by the checksums.
pub fn signed(tag: &str, sums: &[u8], sig: &str) -> bool {
    let Ok(sig) = STANDARD.decode(sig.trim()) else { return false };
    let mut message = format!("horizon release {tag}\n").into_bytes();
    message.extend_from_slice(sums);
    RELEASE_KEYS
        .iter()
        .filter_map(|key| STANDARD.decode(key).ok())
        .any(|key| UnparsedPublicKey::new(&ED25519, key).verify(&message, &sig).is_ok())
}

/// The build a nightly's release title names, strictly (their `nightly_title_version`):
/// `Nightly 2026-11-05 · 66f3981` is `nightly-2026-11-05-66f3981`.
pub fn title_version(title: &str) -> Option<String> {
    let (date, commit) = title.strip_prefix("Nightly ")?.split_once(" · ")?;
    let date_ok = date.len() == 10
        && date
            .bytes()
            .enumerate()
            .all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    let commit_ok =
        (7..=40).contains(&commit.len()) && commit.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (date_ok && commit_ok).then(|| format!("nightly-{date}-{commit}"))
}

/// Whether Horizon signed the checksums `sums` of the nightly titled `title`. Its own build's
/// signature (`v2`, `SHA256SUMS.v2.sig`) is checked when the title names the build and it has one,
/// so an older nightly cannot pass for tonight's. Otherwise the channel's (`v1`, `SHA256SUMS.sig`)
/// is checked: nightlies before 2026-11-04 carry only that one, and Horizon drops it around
/// January 2027 (their `docs/security.md`).
pub fn nightly_signed(title: Option<&str>, sums: &[u8], v2: Option<&str>, v1: Option<&str>) -> bool {
    match (title.and_then(title_version), v2) {
        (Some(build), Some(sig)) => signed(&build, sums, sig),
        _ => v1.is_some_and(|sig| signed(NIGHTLY_TAG, sums, sig)),
    }
}

/// The SHA-256 the checksums `sums` give file `name` (`<sha256>  <name>` lines).
pub fn checksum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let ok = file.trim_start_matches([' ', '*']) == name
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| hash.to_ascii_lowercase())
    })
}

/// The archive of the game for platform `os` / `arch` (as `std::env::consts` names them).
pub fn archive_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("horizon-windows-x64.zip"),
        ("windows", "aarch64") => Some("horizon-windows-arm64.zip"),
        ("linux", "x86_64") => Some("horizon-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Some("horizon-macos-arm64.zip"),
        ("macos", "x86_64") => Some("horizon-macos-x64.zip"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nightly 2026-11-05 · 66f3981, as published.
    const SUMS: &[u8] = include_bytes!("../../fixtures/SHA256SUMS");
    const SIG: &str = include_str!("../../fixtures/SHA256SUMS.sig");
    const SIG_V2: &str = include_str!("../../fixtures/SHA256SUMS.v2.sig");
    const TITLE: &str = "Nightly 2026-11-05 · 66f3981";

    #[test]
    fn a_nightly_s_title_names_its_build() {
        assert_eq!(title_version(TITLE).as_deref(), Some("nightly-2026-11-05-66f3981"));
        let long = format!("Nightly 2026-11-05 · {}", "a".repeat(40));
        assert_eq!(title_version(&long), Some(format!("nightly-2026-11-05-{}", "a".repeat(40))));
        for wrong in [
            "Nightly 2026-11-05",
            "Nightly 2026-11-05 · 66F3981",
            "Nightly 2026-11-05 · 66f39",
            "Nightly 26-11-05 · 66f3981",
            "nightly 2026-11-05 · 66f3981",
            "Nightly 2026-11-05 · 66f3981 extra",
        ] {
            assert_eq!(title_version(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_nightly_is_taken_by_its_own_build_s_signature_else_by_the_channel_s() {
        assert!(nightly_signed(Some(TITLE), SUMS, Some(SIG_V2), None), "its build's signature");
        assert!(
            !nightly_signed(Some("Nightly 2026-11-04 · 66f3981"), SUMS, Some(SIG_V2), Some(SIG)),
            "another night's title: the channel's signature does not stand in"
        );
        assert!(!nightly_signed(Some(TITLE), SUMS, Some(SIG), Some(SIG)), "a v2 that is no v2");
        assert!(nightly_signed(None, SUMS, Some(SIG_V2), Some(SIG)), "no title: the channel's");
        assert!(nightly_signed(Some(TITLE), SUMS, None, Some(SIG)), "a nightly from before v2");
        assert!(!nightly_signed(None, SUMS, Some(SIG_V2), None));
        assert!(!nightly_signed(Some(TITLE), SUMS, None, None));
    }

    #[test]
    fn a_nightly_s_checksums_are_taken_only_as_horizon_signed_them() {
        assert!(signed(NIGHTLY_TAG, SUMS, SIG));
        assert!(!signed("nightly-2026-11-01", SUMS, SIG), "the signature names its tag");
        let mut tampered = SUMS.to_vec();
        tampered[0] ^= 1;
        assert!(!signed(NIGHTLY_TAG, &tampered, SIG));
        assert!(!signed(NIGHTLY_TAG, SUMS, "not base64!"));
        assert!(!signed(NIGHTLY_TAG, SUMS, ""));
    }

    #[test]
    fn an_archive_s_checksum_is_read_by_its_name() {
        let sums = std::str::from_utf8(SUMS).unwrap();
        assert_eq!(
            checksum(sums, "horizon-windows-x64.zip").as_deref(),
            Some("0e3157a74c56249573f42624d64bc87774324dec1cd20ba6518c784b7293eda0")
        );
        assert_eq!(checksum(sums, "horizon-windows-x64"), None, "the whole name");
        assert_eq!(checksum("zz  a.zip\n", "a.zip"), None, "a hash is 64 hex digits");
        assert_eq!(checksum(&format!("{}  *a.zip", "A".repeat(64)), "a.zip"), Some("a".repeat(64)));
    }

    #[test]
    fn each_platform_has_its_archive() {
        assert_eq!(archive_for("windows", "x86_64"), Some("horizon-windows-x64.zip"));
        assert_eq!(archive_for("linux", "x86_64"), Some("horizon-linux-x86_64.tar.gz"));
        assert_eq!(archive_for("macos", "aarch64"), Some("horizon-macos-arm64.zip"));
        assert_eq!(archive_for("windows", "aarch64"), Some("horizon-windows-arm64.zip"));
        assert_eq!(archive_for("linux", "aarch64"), None);
    }
}
