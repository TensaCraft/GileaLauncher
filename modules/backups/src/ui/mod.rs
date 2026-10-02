//! The backups module's UI: a Backups tab in a build's content, a Backups section in the
//! launcher's settings and a switch to delete a build's backups with it.

mod api;
mod settings;
mod tab;

use leptos::prelude::*;
use ui_kit::TagTone;
use ui_kit::module::{DeleteOption, ModuleSection, ModuleTab, UiModule};

use crate::dto::Kind;

/// The translation key of a backup's kind.
pub fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Auto => "world_backup_kind_auto",
        Kind::Manual => "world_backup_kind_manual",
    }
}

/// Local "dd.mm.yyyy hh:mm" of Unix seconds.
pub fn date_time(seconds: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64 * 1000.0));
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}

pub struct BackupsModuleUi;

impl UiModule for BackupsModuleUi {
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
            id: "backups",
            icon: "backup",
            label: "world_backups_content_tab",
            tone: TagTone::NeoForge,
            after: "shaders",
            view: |key| view! { <tab::BackupsTab key=key /> }.into_any(),
        }]
    }

    fn settings_sections(&self) -> Vec<ModuleSection> {
        vec![ModuleSection {
            module: crate::ID,
            id: "backups",
            icon: "backup",
            label: "settings_tab_backups",
            after: "java",
            view: || view! { <settings::BackupsSettings /> }.into_any(),
        }]
    }

    fn delete_options(&self) -> Vec<DeleteOption> {
        vec![DeleteOption {
            module: crate::ID,
            label: "version_delete_backups",
            desc: "version_delete_backups_desc",
            command: "delete_build",
        }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(BackupsModuleUi)
}

#[cfg(test)]
mod tests {
    use ui_kit::module::UiModule;

    use super::*;
    use crate::dto::Kind;

    #[test]
    fn sizes_and_kinds_read_like_the_launcher_s() {
        assert_eq!(
            (kind_key(Kind::Auto), kind_key(Kind::Manual)),
            ("world_backup_kind_auto", "world_backup_kind_manual")
        );
    }

    #[test]
    fn the_module_adds_a_tab_a_section_and_a_delete_switch() {
        let ui = BackupsModuleUi;
        let tabs = ui.content_tabs();
        assert_eq!(
            tabs.iter().map(|t| (t.module, t.id, t.after)).collect::<Vec<_>>(),
            vec![("backups", "backups", "shaders")]
        );
        let sections = ui.settings_sections();
        assert_eq!(sections.iter().map(|s| (s.id, s.after)).collect::<Vec<_>>(), vec![("backups", "java")]);
        let options = ui.delete_options();
        assert_eq!(
            options.iter().map(|o| (o.label, o.command)).collect::<Vec<_>>(),
            vec![("version_delete_backups", "delete_build")]
        );
    }
}
