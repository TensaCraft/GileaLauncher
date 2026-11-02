//! Horizon's signed nightly build: its checksums (`SHA256SUMS`), the signature over them and the
//! archive of this computer's platform.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};

/// Where the newest nightly's files are (a tag that moves to each new nightly).
pub const NIGHTLY: &str = "https://github.com/horizon-rust/horizon/releases/download/nightly";
/// The tag the nightly's signature names.
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
        ("linux", "x86_64") => Some("horizon-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Some("horizon-macos-arm64.zip"),
        ("macos", "x86_64") => Some("horizon-macos-x64.zip"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &[u8] = include_bytes!("../../fixtures/SHA256SUMS");
    const SIG: &str = include_str!("../../fixtures/SHA256SUMS.sig");

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
            Some("b942cd4c5ea42065a3fe2416210865450d5a41aa3bbe868d957bb3529970b59c")
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
        assert_eq!(archive_for("windows", "aarch64"), None);
    }
}
