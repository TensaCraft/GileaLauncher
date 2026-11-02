//! Launcher optional module `horizon`: builds that run Horizon, Minecraft 26.3 rewritten in Rust
//! (github.com/horizon-rust/horizon), started through its Launch API.

pub const ID: &str = "horizon";
/// The build option value (`engine`) of a Horizon build.
pub const ENGINE: &str = "horizon";
/// The Minecraft version Horizon plays.
pub const MINECRAFT: &str = "26.3";

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
