//! The module's UI: the dialog a crash opens and the Diagnostics tab of a build's content.

mod dialog;
mod state;
mod tab;

use leptos::prelude::*;
use ui_kit::TagTone;
use ui_kit::module::{ModuleOverlay, ModuleTab, UiModule};

pub use dialog::{FindingView, more_count, offered};

struct DiagnosticsUi;

impl UiModule for DiagnosticsUi {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../../locales/en_US.json")),
            _ => None,
        }
    }

    fn content_tabs(&self) -> Vec<ModuleTab> {
        vec![ModuleTab {
            module: crate::ID,
            id: "diagnostics",
            icon: "troubleshoot",
            label: "diagnostics_content_tab",
            tone: TagTone::Snapshot,
            after: "settings",
            view: |key| view! { <tab::DiagnosticsPanel key=key /> }.into_any(),
        }]
    }

    fn overlays(&self) -> Vec<ModuleOverlay> {
        vec![ModuleOverlay { module: crate::ID, view: || view! { <dialog::CrashHost /> }.into_any() }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(DiagnosticsUi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_adds_its_tab_after_settings_and_its_crash_window() {
        let ui = ui_module();
        let tabs = ui.content_tabs();
        assert_eq!(
            tabs.iter().map(|t| (t.module, t.id, t.after, t.tone)).collect::<Vec<_>>(),
            [(crate::ID, "diagnostics", "settings", TagTone::Snapshot)]
        );
        assert_eq!(ui.overlays().iter().map(|o| o.module).collect::<Vec<_>>(), [crate::ID]);
    }
}
