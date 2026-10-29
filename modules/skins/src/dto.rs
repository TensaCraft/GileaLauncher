//! What the skins module's backend and interface say to each other.

use launcher_shared::{AppError, Text};
use serde::{Deserialize, Serialize};

/// Sent after any change of the library, a skin or a cape: the page and the avatars load again.
pub const CHANGED_EVENT: &str = "app://module/skins/changed";

/// The parameter an error made by the module names its message with.
pub const MESSAGE: &str = "message";

/// What the player reads for `e`: the module's own message when it named one, else the
/// launcher's for its code.
pub fn error_text(e: &AppError) -> Text {
    let mut params = e.params.clone();
    let key = params.remove(MESSAGE).unwrap_or_else(|| e.code.i18n_key().to_string());
    params.entry("error".to_string()).or_insert_with(|| e.detail.clone());
    Text::Key { key, params }
}

/// A skin's arm width: Steve's four pixels or Alex's three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinVariant {
    #[default]
    Classic,
    Slim,
}

impl SkinVariant {
    /// The name Minecraft Services uses.
    pub fn as_str(self) -> &'static str {
        match self {
            SkinVariant::Classic => "classic",
            SkinVariant::Slim => "slim",
        }
    }
}

/// An official (Microsoft) profile of the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub key: String,
    pub name: String,
    pub is_default: bool,
    pub reauth_required: bool,
}

/// A skin of the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySkin {
    pub id: String,
    pub name: String,
    pub variant: SkinVariant,
    /// `file`, `player:<name>`, `link:<address>` or `account:<name>`.
    pub source: String,
    pub sha256: String,
    pub added_ms: u64,
    /// A front view of the whole body, `data:image/png;base64,…`.
    pub thumb: String,
}

/// The skin an account wears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentSkin {
    pub sha256: String,
    pub variant: SkinVariant,
    /// The skin itself, `data:image/png;base64,…`.
    pub data: String,
    /// The library's copy of it, if there is one.
    pub library_id: Option<String>,
    /// One of Minecraft's own skins (Steve, Alex…): shown, never added to the library.
    #[serde(default)]
    pub default: bool,
}

/// A cape an account has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cape {
    pub id: String,
    pub alias: String,
    pub active: bool,
    /// The cape's texture, `data:image/png;base64,…`.
    pub data: String,
}

/// What an account wears and has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountState {
    pub profile: String,
    /// `None`: the account wears Minecraft's default skin.
    pub skin: Option<CurrentSkin>,
    pub capes: Vec<Cape>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileArgs {
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdArgs {
    pub id: String,
}

/// Where a skin added to the library comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SkinSource {
    /// A PNG file on this computer.
    File { path: String },
    /// A player's skin as Mojang has it now, by nickname.
    Player { name: String },
    /// A PNG at a web address.
    Link { url: String },
}

/// A skin to add to the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportArgs {
    pub source: SkinSource,
    /// Its name in the library; when blank, the file's, the player's or the link's.
    #[serde(default)]
    pub name: Option<String>,
    /// Its model; when none, the one its arms suggest.
    #[serde(default)]
    pub variant: Option<SkinVariant>,
}

impl ImportArgs {
    /// From `source`, named after it, on the model its arms suggest.
    pub fn new(source: SkinSource) -> ImportArgs {
        ImportArgs { source, name: None, variant: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameArgs {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantArgs {
    pub id: String,
    pub variant: SkinVariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyArgs {
    pub profile: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapeArgs {
    pub profile: String,
    /// `None`: no cape.
    pub cape: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_are_lowercase() {
        assert_eq!(serde_json::to_string(&SkinVariant::Slim).unwrap(), "\"slim\"");
        assert_eq!(serde_json::from_str::<SkinVariant>("\"classic\"").unwrap(), SkinVariant::Classic);
        assert_eq!(SkinVariant::Slim.as_str(), "slim");
    }

    #[test]
    fn a_state_round_trips() {
        let state = AccountState {
            profile: "Notch_UA".into(),
            skin: Some(CurrentSkin {
                sha256: "ab".into(),
                variant: SkinVariant::Slim,
                data: "data:image/png;base64,AA==".into(),
                library_id: None,
                default: true,
            }),
            capes: vec![Cape {
                id: "c1".into(),
                alias: "Migrator".into(),
                active: true,
                data: String::new(),
            }],
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(serde_json::from_value::<AccountState>(value).unwrap(), state);
        let cape: CapeArgs =
            serde_json::from_value(serde_json::json!({"profile": "p", "cape": null})).unwrap();
        assert_eq!(cape.cape, None);
        let older = serde_json::json!({"sha256": "ab", "variant": "classic", "data": "", "library_id": null});
        assert!(!serde_json::from_value::<CurrentSkin>(older).unwrap().default, "a custom skin by default");
    }

    #[test]
    fn a_source_says_its_kind() {
        let link = ImportArgs::new(SkinSource::Link { url: "https://x.ua/a.png".into() });
        let value = serde_json::to_value(&link).unwrap();
        assert_eq!(value["source"], serde_json::json!({"kind": "link", "url": "https://x.ua/a.png"}));
        assert_eq!(serde_json::from_value::<ImportArgs>(value).unwrap(), link);
        let bare = serde_json::json!({"source": {"kind": "player", "name": "Alex"}});
        let args: ImportArgs = serde_json::from_value(bare).unwrap();
        assert_eq!((args.name, args.variant), (None, None), "named and modelled by the skin itself");
    }

    #[test]
    fn an_error_reads_as_the_module_message_it_names() {
        use launcher_shared::ErrorCode;
        let rate = AppError::new(ErrorCode::Network, "HTTP 429").with_param(MESSAGE, "skins_error_rate");
        let Text::Key { key, params } = error_text(&rate) else { panic!("a key") };
        assert_eq!(key, "skins_error_rate");
        assert!(!params.contains_key(MESSAGE));
        let io = AppError::new(ErrorCode::Io, "denied");
        let Text::Key { key, .. } = error_text(&io) else { panic!("a key") };
        assert_eq!(key, "error_io");
    }
}
