//! The server builds module's UI: cards on Home for the server builds not installed yet, a
//! settings section with the Home switch, "Force sync" in a server build's menu, and a warning on
//! a server build's provider tabs that what is added to the server's folders goes at its next sync.

mod home;
mod settings;

use launcher_shared::BuildDto;
use leptos::prelude::*;
use ui_kit::module::{
    BuildActionRun, ModuleBuildAction, ModuleContentNotice, ModuleHomeCards, ModuleSection, UiModule,
};

/// The server's client, as its builds name it (`backend::identity::CLIENT`).
const CLIENT: &str = "TensaCraft";

/// A build the server manages goes by the server's client.
pub fn is_server_build(build: &BuildDto) -> bool {
    build.client.as_deref().is_some_and(|c| c.trim().eq_ignore_ascii_case(CLIENT))
}

pub struct TensaModuleUi;

impl UiModule for TensaModuleUi {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn home_cards(&self) -> Vec<ModuleHomeCards> {
        vec![ModuleHomeCards { module: crate::ID, view: || view! { <home::ServerBuildCards /> }.into_any() }]
    }

    fn settings_sections(&self) -> Vec<ModuleSection> {
        vec![ModuleSection {
            module: crate::ID,
            id: "tensa",
            icon: "rocket_launch",
            label: "settings_tab_tensa",
            after: "interface",
            view: || view! { <settings::ServerBuildsSettings /> }.into_any(),
        }]
    }

    fn build_actions(&self) -> Vec<ModuleBuildAction> {
        vec![ModuleBuildAction {
            module: crate::ID,
            id: "force_sync",
            icon: "sync",
            label: "tensacraft_force_sync",
            run: BuildActionRun::Command("force_sync"),
            applies: is_server_build,
        }]
    }

    fn content_notices(&self) -> Vec<ModuleContentNotice> {
        vec![ModuleContentNotice {
            module: crate::ID,
            text: "tensa_added_content_synced_away",
            applies: is_server_build,
        }]
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../../locales/en_US.json")),
            _ => None,
        }
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(TensaModuleUi)
}

#[cfg(test)]
mod tests {
    use launcher_shared::BuildDto;
    use ui_kit::module::UiModule;

    use super::*;

    fn build(client: Option<&str>) -> BuildDto {
        BuildDto {
            key: "aero".into(),
            version_id: "aero".into(),
            name: "Aero".into(),
            version: None,
            loader: None,
            client: client.map(str::to_string),
            loader_version: None,
            game_dir: String::new(),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
        }
    }

    #[test]
    fn the_module_adds_home_cards_a_section_and_a_menu_action() {
        let ui = TensaModuleUi;
        assert_eq!(ui.home_cards().iter().map(|c| c.module).collect::<Vec<_>>(), [crate::ID]);
        let sections = ui.settings_sections();
        assert_eq!(
            sections.iter().map(|s| (s.id, s.label, s.after)).collect::<Vec<_>>(),
            [("tensa", "settings_tab_tensa", "interface")]
        );
        let actions = ui.build_actions();
        assert_eq!(
            actions.iter().map(|a| (a.id, a.label)).collect::<Vec<_>>(),
            [("force_sync", "tensacraft_force_sync")]
        );
        assert!(matches!(actions[0].run, BuildActionRun::Command("force_sync")));
    }

    #[test]
    fn a_server_build_warns_that_added_content_is_synced_away() {
        let notices = TensaModuleUi.content_notices();
        assert_eq!(
            notices.iter().map(|n| (n.module, n.text)).collect::<Vec<_>>(),
            [(crate::ID, "tensa_added_content_synced_away")]
        );
        assert!((notices[0].applies)(&build(Some("TensaCraft"))));
        assert!(!(notices[0].applies)(&build(Some("Fabric"))) && !(notices[0].applies)(&build(None)));
    }

    #[test]
    fn the_force_sync_action_is_for_server_builds_only() {
        assert!(is_server_build(&build(Some("TensaCraft"))));
        assert!(is_server_build(&build(Some(" tensacraft "))));
        assert!(!is_server_build(&build(Some("Fabric"))));
        assert!(!is_server_build(&build(None)));
    }

    #[test]
    fn the_settings_section_is_named_after_the_module() {
        // The module's own section: everything Tensa has to set lives there.
        for raw in [include_str!("../../locales/uk_UA.json"), include_str!("../../locales/en_US.json")] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            assert_eq!(map["settings_tab_tensa"], "Tensa");
        }
    }
}
