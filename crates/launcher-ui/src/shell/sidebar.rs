use leptos::prelude::*;
use leptos_router::hooks::use_location;
use ui_kit::i18n::use_i18n;
use ui_kit::module::ModulePage;
use ui_kit::{Icon, sound};

use super::{has_contacts, use_support};
use crate::pages::modpacks::MODPACKS_PATH;
use crate::profiles::avatar::ProfileAvatar;
use crate::providers::{modpack_providers, providers};
use crate::store::use_store;

/// A sidebar item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavItem {
    pub path: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
}

const fn item(path: &'static str, icon: &'static str, label: &'static str) -> NavItem {
    NavItem { path, icon, label }
}

/// Every module's pages (provided by the app).
#[derive(Clone, Default)]
pub struct ModulePages(pub Vec<ModulePage>);

/// Every build's screenshots.
pub const SCREENSHOTS_PATH: &str = "/screenshots";

/// The sidebar: Home, Builds, Modpacks (while a provider offers them), the pages of modules whose
/// backend is here, Screenshots, Settings.
pub fn nav_items(pages: &[ModulePage], modpacks: bool, backend: &[String]) -> Vec<NavItem> {
    let mut items = vec![item("/", "home", "home_title"), item("/builds", "layers", "builds_title")];
    if modpacks {
        items.push(item(MODPACKS_PATH, "webhook", "modpacks_title"));
    }
    items.extend(
        pages.iter().filter(|p| backend.iter().any(|m| m == p.module)).map(|p| item(p.path, p.icon, p.label)),
    );
    items.push(item(SCREENSHOTS_PATH, "photo_library", "screenshots_title"));
    items.push(item("/settings", "settings", "settings_title"));
    items
}

/// The module page at `current` (or under it), when its backend is here.
pub fn page_for<'a>(pages: &'a [ModulePage], backend: &[String], current: &str) -> Option<&'a ModulePage> {
    pages.iter().find(|p| backend.iter().any(|m| m == p.module) && is_active(current, p.path))
}

/// The ids of the modules whose backend this launcher has.
pub fn backend_modules(info: Option<&launcher_shared::AppInfo>) -> Vec<String> {
    info.iter().flat_map(|i| i.modules.iter().map(|m| m.id.clone())).collect()
}

/// Whether the profile button calls for a first profile: none was made, the list is known, and
/// the user is not on the profiles page already.
pub fn calls_for_profile(loaded: bool, profiles: usize, on_profiles: bool) -> bool {
    loaded && profiles == 0 && !on_profiles
}

pub fn is_active(current: &str, path: &str) -> bool {
    if path == "/" { current == "/" } else { current == path || current.starts_with(&format!("{path}/")) }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let help = use_support();
    let location = use_location();
    let contacts = has_contacts();
    let default_profile = move || store.profiles.with(|list| list.iter().find(|p| p.is_default).cloned());
    let nudge = move || {
        calls_for_profile(
            store.profiles_loaded.get(),
            store.profiles.with(Vec::len),
            is_active(&location.pathname.get(), "/profiles"),
        )
    };
    let pages = StoredValue::new(use_context::<ModulePages>().unwrap_or_default().0);
    let items = move || {
        let (backend, modpacks) = store
            .info
            .with(|i| (backend_modules(i.as_ref()), !modpack_providers(&providers(i.as_ref())).is_empty()));
        pages.with_value(|p| nav_items(p, modpacks, &backend))
    };
    // Labels hide in the compact sidebar; the tooltip layer names the item then.
    let tip =
        move |key: &'static str| move || store.settings.with(|s| s.compact_sidebar).then(|| i18n.t(key));
    view! {
        <aside class="sidebar" data-window-drag="">
            // No logo and no name (owner, 2026-10-02): the launcher names no brand.
            <nav class="sidebar__nav">
                {move || items().into_iter().map(|NavItem { path, icon, label }| view! {
                    <a
                        href=path
                        class="sidebar__item"
                        class:is-active=move || is_active(&location.pathname.get(), path)
                        data-tip=tip(label)
                        on:click=move |_| sound::play_click()
                    >
                        <Icon name=icon />
                        <span class="sidebar__label">{move || i18n.t(label)}</span>
                    </a>
                }).collect_view()}
            </nav>
            <div class="sidebar__spacer"></div>
            {move || contacts.get().then(|| view! {
                <button type="button" class="sidebar__item is-support" data-tip=tip("support_title") on:click=move |_| {
                    sound::play_click();
                    help.open();
                }>
                    <Icon name="support_agent" />
                    <span class="sidebar__label">{move || i18n.t("support_title")}</span>
                </button>
            })}
            <a
                href="/profiles"
                class="sidebar__item sidebar__user"
                class:is-active=move || is_active(&location.pathname.get(), "/profiles")
                class:is-nudge=nudge
                data-tip=tip("profile_title")
                on:click=move |_| sound::play_click()
            >
                <ProfileAvatar
                    profile_key=Signal::derive(move || default_profile().map(|p| p.key))
                    size=24
                    fallback="account_circle"
                />
                <span class="sidebar__label">
                    {move || default_profile().map(|p| p.name).unwrap_or_else(|| i18n.t("profile_empty"))}
                </span>
                // No profile yet: a hint beside the button says what to do.
                {move || nudge().then(|| view! { <span class="sidebar__nudge" role="status">{i18n.t("profile_nudge")}</span> })}
            </a>
        </aside>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_button_calls_for_a_first_profile_only() {
        assert!(calls_for_profile(true, 0, false));
        assert!(!calls_for_profile(false, 0, false), "not before the list is known");
        assert!(!calls_for_profile(true, 1, false), "one is there");
        assert!(!calls_for_profile(true, 0, true), "already on the profiles page");
    }

    #[test]
    fn active_matching() {
        assert!(is_active("/", "/"));
        assert!(!is_active("/builds", "/"));
        assert!(is_active("/builds", "/builds"));
        assert!(is_active("/builds/abc", "/builds"));
        assert!(!is_active("/buildsx", "/builds"));
    }

    fn nothing() -> AnyView {
        ().into_any()
    }

    fn page(module: &'static str, path: &'static str) -> ModulePage {
        ModulePage { module, path, icon: "summarize", label: "reports_title", view: nothing }
    }

    #[test]
    fn module_pages_sit_between_builds_and_settings_when_their_backend_is_here() {
        let pages = [page("reports", "/reports"), page("ghost", "/ghost")];
        let backend = vec!["reports".to_string()];
        let paths: Vec<&str> = nav_items(&pages, false, &backend).iter().map(|i| i.path).collect();
        assert_eq!(paths, ["/", "/builds", "/reports", "/screenshots", "/settings"]);
        assert_eq!(page_for(&pages, &backend, "/reports").map(|p| p.path), Some("/reports"));
        assert_eq!(page_for(&pages, &backend, "/reports/x").map(|p| p.path), Some("/reports"));
        assert!(page_for(&pages, &backend, "/ghost").is_none(), "no backend, no page");
        assert!(page_for(&pages, &[], "/reports").is_none());
        assert_eq!(nav_items(&pages, false, &[]).len(), 4, "without backends only the core's items");
    }

    #[test]
    fn modpacks_show_while_a_provider_offers_them() {
        let paths = |modpacks: bool, backend: &[&str]| -> Vec<&'static str> {
            let backend: Vec<String> = backend.iter().map(|m| m.to_string()).collect();
            nav_items(&[page("reports", "/reports")], modpacks, &backend).iter().map(|i| i.path).collect()
        };
        assert_eq!(
            paths(true, &["reports"]),
            ["/", "/builds", "/modpacks", "/reports", "/screenshots", "/settings"]
        );
        assert_eq!(paths(true, &[]), ["/", "/builds", "/modpacks", "/screenshots", "/settings"]);
        assert_eq!(
            paths(false, &["reports"]),
            ["/", "/builds", "/reports", "/screenshots", "/settings"],
            "no provider, no page"
        );
    }
}
