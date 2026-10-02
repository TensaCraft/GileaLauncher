//! Launcher optional module `diagnostics`: what made a game crash. Postponed
//! until after the release: `cargo xtask … --modules all` leaves it out, `--modules diagnostics`
//! builds it in.

pub const ID: &str = "diagnostics";

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
    fn a_missing_dependency_names_the_mod_and_what_it_needs() {
        for raw in [include_str!("../locales/uk_UA.json"), include_str!("../locales/en_US.json")] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            let message = map["launch_diagnostic_missing_mod_dependency"].as_str().unwrap();
            assert!(message.contains("{mod}") && message.contains("{dependency}"), "{message}");
        }
    }
}
