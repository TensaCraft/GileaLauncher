//! Offline accounts.

use launcher_shared::{AppError, AppResult, ErrorCode};
use md5::{Digest, Md5};

pub const MAX_NAME_CHARS: usize = 16;

/// The original launcher's offline UUID: the raw MD5 of `OfflinePlayer:<name>` printed as a UUID,
/// without the version/variant bits Java's `nameUUIDFromBytes` sets. Kept byte-exact so existing
/// worlds keep their player data.
pub fn offline_uuid(name: &str) -> String {
    let hex = hex::encode(Md5::digest(format!("OfflinePlayer:{name}").as_bytes()));
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Trimmed nickname of 1–16 visible ASCII characters: Minecraft 1.20.3+ lets no other name in, not
/// even in single player (`StringUtil.isValidPlayerName`). Profiles made before stay as they are.
pub fn validate_offline_name(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    let count = name.chars().count();
    let valid = (1..=MAX_NAME_CHARS).contains(&count) && name.chars().all(|c| c.is_ascii_graphic());
    if valid {
        Ok(name.to_string())
    } else {
        Err(AppError::new(
            ErrorCode::ProfileNameInvalid,
            "offline nickname must be 1-16 visible ASCII characters",
        )
        .with_param("max", MAX_NAME_CHARS.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_the_original_launcher() {
        // Python: uuid.UUID(hashlib.md5(f"OfflinePlayer:{name}".encode()).hexdigest())
        assert_eq!(offline_uuid("Steve"), "5627dd98-e6be-bc21-f8a8-e92344183641");
        assert_eq!(offline_uuid("Player_01"), "f2d90b7a-21fe-4bcb-0d1b-250b3b440256");
        assert_eq!(offline_uuid("Іван"), "b2ea5442-e57b-ab91-9f29-9c36647c4944");
    }

    #[test]
    fn offline_names() {
        assert_eq!(validate_offline_name("  Steve  ").unwrap(), "Steve");
        assert_eq!(validate_offline_name("Player_01").unwrap(), "Player_01");
        assert_eq!(validate_offline_name(&"x".repeat(16)).unwrap().len(), 16);
        // Minecraft 1.20.3+ lets in names of visible ASCII only, single player too.
        for bad in ["", "   ", "two words", "tab\tname", &"x".repeat(17), "Іван", "Steve·", "名前"] {
            assert_eq!(
                validate_offline_name(bad).unwrap_err().code,
                ErrorCode::ProfileNameInvalid,
                "{bad:?}"
            );
        }
    }
}
