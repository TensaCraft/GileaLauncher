//! Launcher optional module `horizon`: builds that run Horizon, Minecraft 26.3 rewritten in Rust
//! (github.com/horizon-rust/horizon), started through its Launch API.

use launcher_shared::ContentKind;
use launcher_shared::provider::ProviderInfo;

pub const ID: &str = "horizon";
/// The build option value (`engine`) of a Horizon build.
pub const ENGINE: &str = "horizon";
/// The Minecraft version Horizon plays.
pub const MINECRAFT: &str = "26.3";
/// A Horizon build's client, as cards and lists name it.
pub const CLIENT: &str = "Horizon";

/// Horizon's registry as a source of its builds' shader packs and resource packs: the shader packs
/// it lists are its builds' only ones.
pub fn provider_info() -> ProviderInfo {
    ProviderInfo {
        id: ID.into(),
        name: "Horizon".into(),
        icon: "rocket_launch".into(),
        content: vec![ContentKind::ShaderPacks, ContentKind::ResourcePacks],
        updates: Vec::new(),
        modpacks: false,
        modpack_updates: false,
        content_clients: vec![CLIENT.into()],
        content_label: Some("horizon_registry_source".into()),
        pack_file: None,
    }
}

pub mod dto;

#[cfg(feature = "backend")]
pub mod backend;
#[cfg(feature = "ui")]
pub mod ui;

#[cfg(test)]
mod tests {
    use launcher_shared::provider::Need;

    use super::*;

    #[test]
    fn locales_define_module_name_and_description() {
        for raw in [include_str!("../locales/uk_UA.json"), include_str!("../locales/en_US.json")] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            assert!(map.contains_key(&format!("module_{}_name", super::ID)));
            assert!(map.contains_key(&format!("module_{}_desc", super::ID)));
        }
    }

    #[test]
    fn the_registry_gives_horizon_builds_shader_packs_and_resource_packs_alone() {
        let info = provider_info();
        assert!(info.offers(Need::Content(ContentKind::ShaderPacks)));
        assert!(info.offers(Need::Content(ContentKind::ResourcePacks)));
        assert!(!info.offers(Need::Content(ContentKind::Mods)) && !info.offers(Need::Modpacks));
        assert!(info.serves(Some("Horizon")) && !info.serves(Some("Fabric")) && !info.serves(None));
        for raw in [include_str!("../locales/uk_UA.json"), include_str!("../locales/en_US.json")] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            for key in [info.content_label.as_deref().unwrap(), "horizon_shader_installed"] {
                assert!(map.contains_key(key), "{key}");
            }
        }
    }
}
