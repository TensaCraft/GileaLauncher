mod about;
pub mod activity;
mod interface;
pub mod java;
mod launcher;
mod logs;
mod storage;

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::module::ModuleSection;
use ui_kit::{NavEntry, SettingsNav};

use crate::modules::use_module_parts;

use crate::shell::{PageHeader, use_header};
use crate::store::use_store;

/// A section of the settings menu: `group` 0 is "General", 1 "System"; a module's `view` draws it.
#[derive(Clone, Copy)]
pub struct SectionDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub group: u8,
    pub view: Option<fn() -> AnyView>,
}

const CORE: [(&str, &str, &str, u8); 6] = [
    ("launcher", "tune", "settings_tab_launcher", 0),
    ("interface", "dashboard", "settings_tab_interface", 0),
    ("storage", "folder", "settings_tab_storage", 0),
    // One word, so the menu keeps it on one line (its sections say memory, GPU and Java).
    ("java", "speed", "settings_tab_java", 0),
    ("activity", "history", "activity_center", 1),
    ("about", "info", "settings_tab_about", 1),
];

/// The menu: the core's sections with the modules' after the ones they name (at the end of the
/// first group when that one is missing).
pub fn sections(extra: &[ModuleSection]) -> Vec<SectionDef> {
    let mut all: Vec<SectionDef> = CORE
        .iter()
        .map(|&(id, icon, label, group)| SectionDef { id, icon, label, group, view: None })
        .collect();
    for section in extra {
        let (at, group) = match all.iter().position(|s| s.id == section.after) {
            Some(i) => (i + 1, all[i].group),
            None => (all.iter().rposition(|s| s.group == 0).map_or(0, |i| i + 1), 0),
        };
        let def = SectionDef {
            id: section.id,
            icon: section.icon,
            label: section.label,
            group,
            view: Some(section.view),
        };
        all.insert(at, def);
    }
    all
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let header = use_header();
    let active = RwSignal::new("launcher");
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    let page_header = view! { <PageHeader title_key="settings_title" /> };
    Effect::new(move |_| {
        if let Some(info) = store.info.get() {
            header.subtitle.set(Some(i18n.tp("about_version", &[("version", info.version.clone())])));
        }
    });

    let parts = use_module_parts();
    let defs = Signal::derive(move || sections(&parts.with(|p| p.sections.clone())));
    let nav = move || {
        let mut entries = Vec::new();
        let mut group = None;
        for def in defs.get() {
            if group != Some(def.group) {
                group = Some(def.group);
                let title = if def.group == 0 { "settings_group_general" } else { "settings_group_system" };
                entries.push(NavEntry::Group(t(title)));
            }
            entries.push(NavEntry::Item { id: def.id, icon: def.icon, label: t(def.label) });
        }
        entries
    };

    view! {
        {page_header}
        <div class="settings">
            {move || view! { <SettingsNav entries=nav() active=active /> }}
            <div class="settings__body">
                {move || match active.get() {
                    "interface" => view! { <interface::InterfaceSection /> }.into_any(),
                    "storage" => view! { <storage::StorageSection /> }.into_any(),
                    "java" => view! { <java::JavaSection /> }.into_any(),
                    "activity" => view! { <activity::ActivitySection /> }.into_any(),
                    "about" => view! { <about::AboutSection /> }.into_any(),
                    id => match defs.with(|d| d.iter().find(|s| s.id == id).and_then(|s| s.view)) {
                        Some(module_view) => module_view(),
                        None => view! { <launcher::LauncherSection /> }.into_any(),
                    },
                }}
            </div>
        </div>
    }
}

/// Local signal mirroring one boolean setting from the store.
pub(crate) fn mirror_bool(read: fn(&launcher_shared::SettingsSnapshot) -> bool) -> RwSignal<bool> {
    let store = use_store();
    let signal = RwSignal::new(read(&store.settings.get_untracked()));
    Effect::new(move |_| signal.set(read(&store.settings.get())));
    signal
}

#[cfg(test)]
mod tests {
    use ui_kit::module::ModuleSection;

    use super::*;

    #[test]
    fn a_module_section_goes_after_the_section_it_names() {
        let backups = ModuleSection {
            module: "backups",
            id: "backups",
            icon: "backup",
            label: "settings_tab_backups",
            after: "java",
            view: || ().into_any(),
        };
        let ids: Vec<&str> = sections(&[backups]).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["launcher", "interface", "storage", "java", "backups", "activity", "about"]);
        let core: Vec<&str> = sections(&[]).into_iter().map(|s| s.id).collect();
        assert_eq!(core, vec!["launcher", "interface", "storage", "java", "activity", "about"]);
    }

    #[test]
    fn the_java_section_is_one_word() {
        // «Java та продуктивність» wrapped to two lines in the menu (owner, 2026-09-29).
        let java = sections(&[]).into_iter().find(|s| s.id == "java").unwrap();
        assert_eq!(java.label, "settings_tab_java");
    }
}
