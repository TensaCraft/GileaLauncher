use launcher_shared::{ProfileDto, sort_profiles};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use ui_kit::i18n::use_i18n;
use ui_kit::module::ModulePage;
use ui_kit::{Icon, MenuEntry, sound, use_context_menu};

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

/// The sidebar: Home, Modpacks (while a provider offers them), Builds, the pages of modules whose
/// backend is here, Screenshots, the modules' pages asked to be last (Sync), Settings.
pub fn nav_items(pages: &[ModulePage], modpacks: bool, backend: &[String]) -> Vec<NavItem> {
    let mut items = vec![item("/", "home", "home_title")];
    if modpacks {
        items.push(item(MODPACKS_PATH, "widgets", "modpacks_title"));
    }
    items.push(item("/builds", "layers", "builds_title"));
    let present = |last: bool| {
        pages
            .iter()
            .filter(move |p| p.before_settings == last && backend.iter().any(|m| m == p.module))
            .map(|p| item(p.path, p.icon, p.label))
    };
    items.extend(present(false));
    items.push(item(SCREENSHOTS_PATH, "photo_library", "screenshots_title"));
    items.extend(present(true));
    items.push(item("/settings", "settings", "settings_title"));
    items
}

/// The profile menu's entries past the profiles.
pub const MENU_ADD: &str = "menu:add";
pub const MENU_ALL: &str = "menu:all";

/// The profile button's right-click menu: each profile (the active one first, then by name) with
/// whether it is the active one, then «Додати акаунт» and «Усі профілі»; nothing with no profile
/// (the add dialog opens then). Each is `(id, name, active)`.
pub fn profile_menu(list: &[ProfileDto]) -> Vec<(String, String, bool)> {
    if list.is_empty() {
        return Vec::new();
    }
    let mut rows = list.to_vec();
    sort_profiles(&mut rows);
    let mut entries: Vec<(String, String, bool)> =
        rows.into_iter().map(|p| (p.key, p.name, p.is_default)).collect();
    entries.extend([MENU_ADD, MENU_ALL].map(|id| (id.to_string(), String::new(), false)));
    entries
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
    // No profile yet: the profile button calls for one (the hint layer draws its hint).
    let hints = crate::hints::use_hints();
    let nudge = move || hints.may_show(crate::hints::PROFILE);
    let mark = move || {
        nudge().then(|| hints.mark(crate::hints::PROFILE, i18n.t("profile_nudge"), ui_kit::NudgeSide::Right))
    };
    // Right-click on the profile button: switch profiles at once; with none, «Додати акаунт».
    let menu = use_context_menu();
    let add = crate::profiles::add::use_add_account();
    let profile_actions = crate::profiles::actions::use_profile_actions();
    let profile_menu_open = move |ev: MouseEvent| {
        ev.prevent_default();
        let list = store.profiles.get_untracked();
        let rows = profile_menu(&list);
        if rows.is_empty() {
            add.open();
            return;
        }
        let entries: Vec<MenuEntry> = rows
            .into_iter()
            .flat_map(|(id, name, active)| match id.as_str() {
                MENU_ADD => {
                    vec![MenuEntry::Separator, MenuEntry::item(id, i18n.t("add_account")).icon("person_add")]
                }
                MENU_ALL => vec![MenuEntry::item(id, i18n.t("profile_title")).icon("manage_accounts")],
                _ => vec![MenuEntry::item(id, name).icon(if active {
                    "check_circle"
                } else {
                    "account_circle"
                })],
            })
            .collect();
        menu.open_with(ev.client_x(), ev.client_y(), entries, move |id: String| match id.as_str() {
            MENU_ADD => add.open(),
            MENU_ALL => store.go("/profiles"),
            key => {
                if let Some(profile) = list.iter().find(|p| p.key == key && !p.is_default).cloned() {
                    profile_actions.set_default(profile);
                }
            }
        });
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
                data-nudge=move || mark().map(|m| m.id)
                data-nudge-text=move || mark().map(|m| m.text)
                data-nudge-side=move || mark().map(|m| m.side.attr())
                data-tip=tip("profile_title")
                on:click=move |_| sound::play_click()
                on:contextmenu=profile_menu_open
            >
                <ProfileAvatar
                    profile_key=Signal::derive(move || default_profile().map(|p| p.key))
                    size=24
                    fallback="account_circle"
                />
                <span class="sidebar__label">
                    {move || default_profile().map(|p| p.name).unwrap_or_else(|| i18n.t("profile_empty"))}
                </span>
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
        ModulePage {
            module,
            path,
            icon: "summarize",
            label: "reports_title",
            view: nothing,
            before_settings: false,
        }
    }

    fn profile(key: &str, default: bool) -> ProfileDto {
        ProfileDto {
            key: key.into(),
            name: key.into(),
            id: String::new(),
            kind: launcher_shared::AccountKind::Offline,
            is_default: default,
            reauth_required: false,
            reauth_reason: None,
            server: None,
            server_name: None,
        }
    }

    /// Right-click on the profile button: the profiles (the active one first, ticked), then «Додати
    /// акаунт» and «Усі профілі»; with none, nothing to list (the add dialog opens instead).
    #[test]
    fn the_profile_menu_lists_the_active_one_first_then_add_and_all() {
        let list = [profile("Steve", false), profile("Alex", true), profile("Bob", false)];
        let ids: Vec<(String, bool)> =
            profile_menu(&list).into_iter().map(|(key, _, on)| (key, on)).collect();
        assert_eq!(
            ids,
            [
                ("Alex".to_string(), true),
                ("Bob".to_string(), false),
                ("Steve".to_string(), false),
                (MENU_ADD.to_string(), false),
                (MENU_ALL.to_string(), false),
            ]
        );
        assert!(profile_menu(&[]).is_empty());
    }

    /// A page its module puts last of all but Settings (Sync, owner 2026-10-30).
    #[test]
    fn a_page_asked_to_sits_just_before_settings() {
        let pages = [ModulePage { before_settings: true, ..page("sync", "/sync") }, page("skins", "/skins")];
        let backend = vec!["sync".to_string(), "skins".to_string()];
        let paths: Vec<&str> = nav_items(&pages, true, &backend).iter().map(|i| i.path).collect();
        assert_eq!(paths, ["/", "/modpacks", "/builds", "/skins", "/screenshots", "/sync", "/settings"]);
        let without: Vec<&str> =
            nav_items(&pages, true, &["skins".to_string()]).iter().map(|i| i.path).collect();
        assert_eq!(
            without,
            ["/", "/modpacks", "/builds", "/skins", "/screenshots", "/settings"],
            "no backend, no page"
        );
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
            ["/", "/modpacks", "/builds", "/reports", "/screenshots", "/settings"]
        );
        assert_eq!(paths(true, &[]), ["/", "/modpacks", "/builds", "/screenshots", "/settings"]);
        assert_eq!(
            paths(false, &["reports"]),
            ["/", "/builds", "/reports", "/screenshots", "/settings"],
            "no provider, no page"
        );
    }
}
