//! UI parts of optional modules compiled into this build (cargo features `mod-*`).

use launcher_shared::{AppInfo, BuildDto};
use leptos::prelude::*;
use ui_kit::module::{
    DeleteOption, ModuleAlertAction, ModuleBuildAction, ModuleContentNotice, ModuleHomeCards, ModuleOverlay,
    ModuleSection, ModuleSupportAction, ModuleTab, UiModule,
};

use crate::shell::sidebar::backend_modules;

/// What the modules built into this UI add besides pages; each part shows only while its module's
/// backend is there too.
#[derive(Debug, Clone, Default)]
pub struct ModuleParts {
    pub tabs: Vec<ModuleTab>,
    pub sections: Vec<ModuleSection>,
    pub delete_options: Vec<DeleteOption>,
    pub overlays: Vec<ModuleOverlay>,
    pub home_cards: Vec<ModuleHomeCards>,
    pub build_actions: Vec<ModuleBuildAction>,
    pub alert_actions: Vec<ModuleAlertAction>,
    pub content_notices: Vec<ModuleContentNotice>,
    pub support_actions: Vec<ModuleSupportAction>,
}

impl ModuleParts {
    pub fn of(modules: &[Box<dyn UiModule>]) -> ModuleParts {
        ModuleParts {
            tabs: modules.iter().flat_map(|m| m.content_tabs()).collect(),
            sections: modules.iter().flat_map(|m| m.settings_sections()).collect(),
            delete_options: modules.iter().flat_map(|m| m.delete_options()).collect(),
            overlays: modules.iter().flat_map(|m| m.overlays()).collect(),
            home_cards: modules.iter().flat_map(|m| m.home_cards()).collect(),
            build_actions: modules.iter().flat_map(|m| m.build_actions()).collect(),
            alert_actions: modules.iter().flat_map(|m| m.alert_actions()).collect(),
            content_notices: modules.iter().flat_map(|m| m.content_notices()).collect(),
            support_actions: modules.iter().flat_map(|m| m.support_actions()).collect(),
        }
    }

    /// Only the parts whose module's backend `info` lists.
    pub fn present(&self, info: Option<&AppInfo>) -> ModuleParts {
        self.present_among(&backend_modules(info))
    }

    /// Only the parts of the modules `on`.
    pub fn present_among(&self, on: &[String]) -> ModuleParts {
        let has = |module: &str| on.iter().any(|m| m == module);
        ModuleParts {
            tabs: self.tabs.iter().copied().filter(|t| has(t.module)).collect(),
            sections: self.sections.iter().copied().filter(|s| has(s.module)).collect(),
            delete_options: self.delete_options.iter().copied().filter(|o| has(o.module)).collect(),
            overlays: self.overlays.iter().copied().filter(|o| has(o.module)).collect(),
            home_cards: self.home_cards.iter().copied().filter(|c| has(c.module)).collect(),
            build_actions: self.build_actions.iter().copied().filter(|a| has(a.module)).collect(),
            alert_actions: self.alert_actions.iter().copied().filter(|a| has(a.module)).collect(),
            content_notices: self.content_notices.iter().copied().filter(|n| has(n.module)).collect(),
            support_actions: self.support_actions.iter().copied().filter(|a| has(a.module)).collect(),
        }
    }

    /// The translation keys of the warnings for `build`'s provider tabs.
    pub fn notices_for(&self, build: &BuildDto) -> Vec<&'static str> {
        self.content_notices.iter().filter(|n| (n.applies)(build)).map(|n| n.text).collect()
    }
}

/// The modules' windows over the whole app, those whose backend is there.
#[component]
pub fn ModuleOverlays() -> impl IntoView {
    let parts = use_module_parts();
    let shown = Memo::new(move |_| parts.with(|p| p.overlays.iter().map(|o| o.module).collect::<Vec<_>>()));
    move || {
        let modules = shown.get();
        parts.with_untracked(|p| {
            p.overlays.iter().filter(|o| modules.contains(&o.module)).map(|o| (o.view)()).collect_view()
        })
    }
}

/// The parts the modules add, those whose backend is there.
pub fn use_module_parts() -> Signal<ModuleParts> {
    let all = use_context::<ModuleParts>().unwrap_or_default();
    let store = crate::store::use_store();
    Signal::derive(move || store.info.with(|i| all.present(i.as_ref())))
}

// `vec![]` cannot hold `#[cfg]`-gated elements, hence push-after-new.
#[allow(clippy::vec_init_then_push)]
pub fn ui_modules() -> Vec<Box<dyn UiModule>> {
    #[allow(unused_mut)]
    let mut modules: Vec<Box<dyn UiModule>> = Vec::new();
    #[cfg(feature = "mod-tensa")]
    modules.push(module_tensa::ui::ui_module());
    #[cfg(feature = "mod-modrinth")]
    modules.push(module_modrinth::ui::ui_module());
    #[cfg(feature = "mod-curseforge")]
    modules.push(module_curseforge::ui::ui_module());
    #[cfg(feature = "mod-backups")]
    modules.push(module_backups::ui::ui_module());
    #[cfg(feature = "mod-reports")]
    modules.push(module_reports::ui::ui_module());
    #[cfg(feature = "mod-diagnostics")]
    modules.push(module_diagnostics::ui::ui_module());
    modules
}

/// Logs modules whose UI and backend halves are out of sync (built with different profiles).
pub fn report_mismatch(info: &AppInfo, ui: &[Box<dyn UiModule>]) {
    let backend: Vec<&str> = info.modules.iter().map(|m| m.id.as_str()).collect();
    for m in ui {
        if !backend.contains(&m.id()) {
            web_sys::console::warn_1(&format!("UI module '{}' has no backend in this build", m.id()).into());
        }
    }
    for id in &backend {
        if !ui.iter().any(|m| m.id() == *id) {
            web_sys::console::warn_1(&format!("Backend module '{id}' has no UI in this build").into());
        }
    }
}

#[cfg(test)]
mod tests {
    use ui_kit::module::BuildActionRun;

    use super::*;

    struct WithOverlay;

    impl UiModule for WithOverlay {
        fn id(&self) -> &'static str {
            "watcher"
        }
        fn overlays(&self) -> Vec<ModuleOverlay> {
            vec![ModuleOverlay { module: "watcher", view: || ().into_any() }]
        }
        fn home_cards(&self) -> Vec<ModuleHomeCards> {
            vec![ModuleHomeCards { module: "watcher", view: || ().into_any() }]
        }
        fn build_actions(&self) -> Vec<ModuleBuildAction> {
            vec![ModuleBuildAction {
                module: "watcher",
                id: "watch",
                icon: "visibility",
                label: "watch",
                run: BuildActionRun::Command("watch"),
                applies: |_| true,
            }]
        }
        fn alert_actions(&self) -> Vec<ModuleAlertAction> {
            vec![ModuleAlertAction { module: "watcher", view: |_| ().into_any() }]
        }
        fn content_notices(&self) -> Vec<ModuleContentNotice> {
            vec![ModuleContentNotice {
                module: "watcher",
                text: "watched_content_goes",
                applies: |b| b.client.as_deref() == Some("Watched"),
            }]
        }
    }

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
    fn content_notices_show_for_the_builds_they_apply_to() {
        let all = ModuleParts::of(&[Box::new(WithOverlay) as Box<dyn UiModule>]);
        let on = all.present_among(&["watcher".to_string()]);
        assert_eq!(on.notices_for(&build(Some("Watched"))), ["watched_content_goes"]);
        assert!(on.notices_for(&build(Some("Fabric"))).is_empty(), "not for other builds");
        assert!(
            all.present_among(&[]).notices_for(&build(Some("Watched"))).is_empty(),
            "no backend, no warning"
        );
    }

    #[test]
    fn overlays_show_only_with_their_backend() {
        let all = ModuleParts::of(&[Box::new(WithOverlay) as Box<dyn UiModule>]);
        let shown = |on: &[&str]| {
            let on: Vec<String> = on.iter().map(|m| m.to_string()).collect();
            all.present_among(&on).overlays.iter().map(|o| o.module).collect::<Vec<_>>()
        };
        assert_eq!(shown(&["watcher"]), ["watcher"]);
        assert!(shown(&["backups"]).is_empty(), "no backend, no window");
    }

    #[test]
    fn home_cards_and_build_actions_show_only_with_their_backend() {
        let all = ModuleParts::of(&[Box::new(WithOverlay) as Box<dyn UiModule>]);
        let on =
            |modules: &[&str]| all.present_among(&modules.iter().map(|m| m.to_string()).collect::<Vec<_>>());
        assert_eq!((on(&["watcher"]).home_cards.len(), on(&["watcher"]).build_actions.len()), (1, 1));
        assert!(on(&[]).home_cards.is_empty() && on(&[]).build_actions.is_empty());
    }

    #[test]
    fn alert_actions_show_only_with_their_backend() {
        let all = ModuleParts::of(&[Box::new(WithOverlay) as Box<dyn UiModule>]);
        let on =
            |modules: &[&str]| all.present_among(&modules.iter().map(|m| m.to_string()).collect::<Vec<_>>());
        assert_eq!(on(&["watcher"]).alert_actions.len(), 1);
        assert!(on(&["backups"]).alert_actions.is_empty(), "no backend, no button");
    }
}
