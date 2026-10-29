//! Launcher optional module `skins`: a library of skins and the capes of official profiles.

pub const ID: &str = "skins";

pub mod dto;

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

    #[test]
    fn both_locales_have_the_same_keys() {
        let keys = |raw: &str| {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            map.keys().cloned().collect::<Vec<_>>()
        };
        assert_eq!(keys(include_str!("../locales/uk_UA.json")), keys(include_str!("../locales/en_US.json")));
    }
}
