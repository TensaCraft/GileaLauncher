//! Launcher optional module `tensa`.

use launcher_shared::ContentKind;
use launcher_shared::provider::ProviderInfo;

pub const ID: &str = "tensa";
/// The client the server's builds name.
pub const CLIENT: &str = "TensaCraft";

/// The server builds as a provider: modpacks (the catalog) for every build, and the mods the
/// server allows (its whitelist) as «Необов'язкові» in its own builds' Mods tab.
pub fn provider_info() -> ProviderInfo {
    ProviderInfo {
        id: ID.into(),
        name: "TensaCraft".into(),
        icon: "rocket_launch".into(),
        content: vec![ContentKind::Mods],
        updates: vec![ContentKind::Mods],
        modpacks: true,
        modpack_updates: false,
        content_clients: vec![CLIENT.into()],
        content_label: Some("tensa_optional_source".into()),
        pack_file: None,
    }
}

#[cfg(feature = "backend")]
pub mod backend;
#[cfg(feature = "ui")]
pub mod ui;

#[cfg(test)]
mod tests {
    #[test]
    fn locales_define_module_name_and_description() {
        for raw in [include_str!("../locales/uk_UA.json"), include_str!("../locales/en_US.json")] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            assert!(map.contains_key(&format!("module_{}_name", super::ID)));
            assert!(map.contains_key(&format!("module_{}_desc", super::ID)));
        }
    }
}
