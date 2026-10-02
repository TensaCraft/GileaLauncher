//! Launcher accounts as the UI sees them. Tokens never leave `launcher-core`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDto {
    /// Key in `profiles.json` (the player name).
    pub key: String,
    pub name: String,
    /// Minecraft UUID: 32 hex digits for Microsoft, dashed for offline profiles.
    pub id: String,
    pub kind: AccountKind,
    pub is_default: bool,
    pub reauth_required: bool,
    #[serde(default)]
    pub reauth_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesSnapshot {
    /// Default first, then by name.
    pub profiles: Vec<ProfileDto>,
}

impl ProfilesSnapshot {
    pub fn default_profile(&self) -> Option<&ProfileDto> {
        self.profiles.iter().find(|p| p.is_default)
    }
}

/// Default profile first, then by name ignoring case.
pub fn sort_profiles(list: &mut [ProfileDto]) {
    list.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// Microsoft sign-in progress (`app://auth`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthState {
    #[default]
    Idle,
    /// Waiting for the user in the browser.
    Browser,
    /// The browser flow is unavailable: the user enters `user_code` at `verification_uri`.
    DeviceCode { user_code: String, verification_uri: String, open_url: String },
    /// Signed in with Microsoft; talking to Xbox Live and Minecraft.
    Finishing,
}
