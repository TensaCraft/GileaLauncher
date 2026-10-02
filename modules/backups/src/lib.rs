//! Launcher optional module `backups`.

pub const ID: &str = "backups";

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
}
