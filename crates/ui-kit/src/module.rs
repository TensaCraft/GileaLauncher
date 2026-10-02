//! UI side of optional modules: their translations, pages, content tabs, settings sections,
//! switches of deleting a build and windows over the whole app; what the app lets them do
//! (`ModuleHost`). Content and modpacks come through the provider contract
//! (`launcher_shared::provider`), whose UI is the app's.

use leptos::prelude::*;

use crate::TagTone;

/// A module's page with its sidebar item.
#[derive(Debug, Clone, Copy)]
pub struct ModulePage {
    /// Its module: the page and its item are hidden when the module's backend is missing.
    pub module: &'static str,
    /// Its address, e.g. `/reports`.
    pub path: &'static str,
    pub icon: &'static str,
    /// Translation key of its sidebar label (and its header).
    pub label: &'static str,
    pub view: fn() -> AnyView,
}

/// A tab a module adds to a build's content, placed after tab `after` (or the nearest tab before
/// it that the build has).
#[derive(Debug, Clone, Copy)]
pub struct ModuleTab {
    pub module: &'static str,
    pub id: &'static str,
    pub icon: &'static str,
    /// Translation key of its label.
    pub label: &'static str,
    /// Its icon's colour, one the core tabs do not use.
    pub tone: TagTone,
    pub after: &'static str,
    /// Its panel for the build with this key.
    pub view: fn(String) -> AnyView,
}

/// A section a module adds to the launcher's settings, placed after section `after`.
#[derive(Debug, Clone, Copy)]
pub struct ModuleSection {
    pub module: &'static str,
    pub id: &'static str,
    pub icon: &'static str,
    /// Translation key of its label.
    pub label: &'static str,
    pub after: &'static str,
    pub view: fn() -> AnyView,
}

/// A switch a module adds to deleting a build (off by default); when on, the module's `command`
/// runs with `{"key": <build key>}` before the build goes.
#[derive(Debug, Clone, Copy)]
pub struct DeleteOption {
    pub module: &'static str,
    /// Translation keys of its label and description.
    pub label: &'static str,
    pub desc: &'static str,
    pub command: &'static str,
}

/// A window a module keeps over the whole app (a dialog that opens on the module's event).
#[derive(Debug, Clone, Copy)]
pub struct ModuleOverlay {
    pub module: &'static str,
    pub view: fn() -> AnyView,
}

/// Cards a module adds to Home after the builds' (its own component).
#[derive(Debug, Clone, Copy)]
pub struct ModuleHomeCards {
    pub module: &'static str,
    pub view: fn() -> AnyView,
}

/// What a module's entry in a build's menu does.
#[derive(Debug, Clone, Copy)]
pub enum BuildActionRun {
    /// Runs the module's command with `{"key": <build key>}`.
    Command(&'static str),
    /// Opens the module's own window for the build.
    Open(fn(launcher_shared::BuildDto)),
}

/// An entry a module adds to a build's menu, for the builds `applies` accepts.
#[derive(Debug, Clone, Copy)]
pub struct ModuleBuildAction {
    pub module: &'static str,
    pub id: &'static str,
    pub icon: &'static str,
    /// Translation key of its label.
    pub label: &'static str,
    pub run: BuildActionRun,
    pub applies: fn(&launcher_shared::BuildDto) -> bool,
}

/// A warning a module shows above a provider's tab of a build's content, for the builds `applies`
/// accepts (e.g. content added to a server's build goes at its next sync).
#[derive(Debug, Clone, Copy)]
pub struct ModuleContentNotice {
    pub module: &'static str,
    /// Translation key of its text.
    pub text: &'static str,
    pub applies: fn(&launcher_shared::BuildDto) -> bool,
}

/// A button a module adds to an alert the user can report, before «Open Discord».
#[derive(Debug, Clone, Copy)]
pub struct ModuleAlertAction {
    pub module: &'static str,
    pub view: fn(launcher_shared::Alert) -> AnyView,
}

/// What the app lets a module's UI do beyond its own parts.
#[derive(Clone, Copy)]
pub struct ModuleHost {
    /// Opens tab `.1` of build `.0`'s content.
    pub open_content: Callback<(String, String)>,
    /// Starts build `key` the way its Play button does.
    pub launch: Callback<String>,
}

/// How many cards each module shows on Home right now: Home says it has no builds only when no
/// card at all is there.
#[derive(Clone, Copy)]
pub struct HomeCards(RwSignal<std::collections::BTreeMap<&'static str, usize>>);

impl HomeCards {
    pub fn new() -> HomeCards {
        HomeCards(RwSignal::new(std::collections::BTreeMap::new()))
    }

    /// Module `module` shows `count` cards.
    pub fn set(&self, module: &'static str, count: usize) {
        self.0.update(|counts| {
            counts.insert(module, count);
        });
    }

    /// Module `module` shows none (its cards are gone).
    pub fn clear(&self, module: &'static str) {
        let _ = self.0.try_update(|counts| counts.remove(module));
    }

    /// Some module shows a card (tracked).
    pub fn any(&self) -> bool {
        self.0.with(|counts| counts.values().any(|&n| n > 0))
    }
}

impl Default for HomeCards {
    fn default() -> HomeCards {
        HomeCards::new()
    }
}

/// Home's count of the modules' cards, for Home to provide.
pub fn provide_home_cards() -> HomeCards {
    let cards = HomeCards::new();
    provide_context(cards);
    cards
}

/// Tells Home how many cards `module` shows, kept up to date while the caller lives.
pub fn report_home_cards(module: &'static str, count: Signal<usize>) {
    let Some(cards) = use_context::<HomeCards>() else { return };
    Effect::new(move |_| cards.set(module, count.get()));
    on_cleanup(move || cards.clear(module));
}

pub fn provide_module_host(host: ModuleHost) {
    provide_context(host);
}

/// The app's offer to modules (`None` outside the app, in tests).
pub fn use_module_host() -> Option<ModuleHost> {
    use_context::<ModuleHost>()
}

pub trait UiModule: 'static {
    fn id(&self) -> &'static str;

    /// Raw JSON dictionary for `lang` (`uk_UA` / `en_US`), merged over the core translations.
    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        let _ = lang;
        None
    }

    /// Pages with a sidebar item, between the core's "Builds" and "Settings".
    fn pages(&self) -> Vec<ModulePage> {
        Vec::new()
    }

    /// Tabs of a build's content.
    fn content_tabs(&self) -> Vec<ModuleTab> {
        Vec::new()
    }

    /// Sections of the launcher's settings.
    fn settings_sections(&self) -> Vec<ModuleSection> {
        Vec::new()
    }

    /// Switches of deleting a build.
    fn delete_options(&self) -> Vec<DeleteOption> {
        Vec::new()
    }

    /// Windows over the whole app.
    fn overlays(&self) -> Vec<ModuleOverlay> {
        Vec::new()
    }

    /// Cards on Home.
    fn home_cards(&self) -> Vec<ModuleHomeCards> {
        Vec::new()
    }

    /// Entries of a build's menu.
    fn build_actions(&self) -> Vec<ModuleBuildAction> {
        Vec::new()
    }

    /// Buttons of an alert the user can report.
    fn alert_actions(&self) -> Vec<ModuleAlertAction> {
        Vec::new()
    }

    /// Warnings above a provider's tab of a build's content.
    fn content_notices(&self) -> Vec<ModuleContentNotice> {
        Vec::new()
    }
}

#[cfg(test)]
mod home_cards_tests {
    use super::HomeCards;

    #[test]
    fn a_module_card_keeps_home_from_being_empty() {
        let cards = HomeCards::new();
        assert!(!cards.any(), "no module shows anything yet");
        cards.set("tensa", 2);
        assert!(cards.any());
        cards.set("other", 0);
        cards.set("tensa", 0);
        assert!(!cards.any(), "modules with no cards leave Home empty");
        cards.set("tensa", 1);
        cards.clear("tensa");
        assert!(!cards.any(), "a module's cards leave with it");
    }
}
