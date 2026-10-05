//! Build content: a build's workspace — its installed mods, resource packs and
//! shader packs, its screenshots, its settings, deleting it — with Play in the header. Content
//! providers (Modrinth) add their sources beside the installed content; other modules add tabs
//! of their own.

mod delete;
mod installed;
mod screenshots;

use launcher_shared::{BuildDto, ContentItem, ContentKind, mods_supported};
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_query_map};
use ui_kit::i18n::use_i18n;
use ui_kit::module::ModuleTab;
use ui_kit::{ActionTone, ChipDef, ChipTabs, IconAction, SegOption, Segmented, TagTone};

use self::delete::DeletePanel;
use self::installed::InstalledPanel;
use self::screenshots::ScreenshotsPanel;
use crate::builds::launch::use_launch_flow;
use crate::builds::query_escape;
use crate::modules::use_module_parts;
use crate::pages::build_settings::BuildSettingsPanel;
use crate::providers::panel::ProviderPanel;
use crate::providers::{content_providers, providers};
use crate::shell::{PageHeader, use_header};
use crate::store::use_store;

/// The page's tabs in the original's order: id, icon, label.
pub const TABS: [(&str, &str, &str); 6] = [
    ("mods", "extension", "mods_content_tab"),
    ("resourcepacks", "palette", "resourcepacks"),
    ("shaders", "wb_sunny", "shaders_content_tab"),
    ("screenshots", "photo_library", "version_screenshots_content_tab"),
    ("settings", "tune", "version_settings_content_tab"),
    ("delete", "delete_outline", "version_delete_content_tab"),
];

/// `/builds/content?build=<key>&tab=<tab>`.
pub fn content_path(key: &str, tab: &str) -> String {
    format!("/builds/content?build={}&tab={}", query_escape(key), query_escape(tab))
}

/// The tab of `kind` (the original's tab keys).
pub fn tab_of(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mods",
        ContentKind::ResourcePacks => "resourcepacks",
        ContentKind::ShaderPacks => "shaders",
    }
}

/// A tab's icon (the mods tab's for anything else): a link to a tab shows the tab's own icon.
pub fn tab_icon(tab: &str) -> &'static str {
    TABS.iter().find(|(id, ..)| *id == tab).map_or(TABS[0].1, |(_, icon, _)| icon)
}

/// A tab the page has (a module's among them); anything else opens mods.
pub fn normalize_tab(raw: &str, extra: &[ModuleTab]) -> &'static str {
    TABS.iter().map(|(id, ..)| *id).chain(extra.iter().map(|t| t.id)).find(|id| *id == raw).unwrap_or("mods")
}

/// The build's tabs with the modules' among them: each after the tab it names or, when the build
/// has no such tab, after the nearest tab before that one it has.
pub fn with_module_tabs(
    mut tabs: Vec<(&'static str, &'static str, &'static str)>,
    extra: &[ModuleTab],
) -> Vec<(&'static str, &'static str, &'static str)> {
    for tab in extra {
        let order = |id: &str| TABS.iter().position(|(t, ..)| *t == id);
        let anchor = order(tab.after).unwrap_or(TABS.len());
        let at = tabs
            .iter()
            .rposition(|(id, ..)| *id == tab.after || order(id).is_some_and(|o| o <= anchor))
            .map_or(0, |i| i + 1);
        tabs.insert(at, (tab.id, tab.icon, tab.label));
    }
    tabs
}

/// Each tab's icon colour (the loader chips' palette): mods blue, packs violet, shaders gold,
/// screenshots green, settings the primary teal, deleting orange-red.
pub fn tab_tone(tab: &str) -> TagTone {
    match tab {
        "mods" => TagTone::Forge,
        "resourcepacks" => TagTone::Quilt,
        "shaders" => TagTone::Fabric,
        "screenshots" => TagTone::Vanilla,
        "delete" => TagTone::Beta,
        _ => TagTone::Neutral,
    }
}

/// The tabs a build of `client` has: mods and shader packs need a mod loader (vanilla Minecraft
/// runs neither).
pub fn tabs_for(client: Option<&str>) -> Vec<(&'static str, &'static str, &'static str)> {
    let loader = mods_supported(client);
    TABS.iter().copied().filter(|(id, ..)| loader || !matches!(*id, "mods" | "shaders")).collect()
}

/// `tab` when a build of `client` has it, else its first tab.
pub fn tab_for(tab: &str, client: Option<&str>) -> &'static str {
    let tabs = tabs_for(client);
    tabs.iter().find(|(id, ..)| *id == tab).or(tabs.first()).map_or("resourcepacks", |(id, ..)| id)
}

/// The content a tab lists, if it lists content.
pub fn kind_of_tab(tab: &str) -> Option<ContentKind> {
    ContentKind::ALL.into_iter().find(|kind| tab_of(*kind) == tab)
}

/// The choice that shows what the build has.
pub const INSTALLED: &str = "installed";

/// The translation keys of an installed-content tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabTexts {
    pub search: &'static str,
    pub empty: &'static str,
    pub enable: &'static str,
    pub disable: &'static str,
    pub confirm_delete: &'static str,
}

pub fn texts(kind: ContentKind) -> TabTexts {
    match kind {
        ContentKind::Mods => TabTexts {
            search: "search_mods",
            empty: "no_mods_installed",
            enable: "enable_mod",
            disable: "disable_mod",
            confirm_delete: "confirm_delete_mod",
        },
        ContentKind::ResourcePacks => TabTexts {
            search: "search_resourcepacks",
            empty: "no_resourcepacks_installed",
            enable: "enable_resourcepack",
            disable: "disable_resourcepack",
            confirm_delete: "confirm_delete_resourcepack",
        },
        ContentKind::ShaderPacks => TabTexts {
            search: "search_shaders",
            empty: "no_shaderpacks_installed",
            enable: "enable_shaderpack",
            disable: "disable_shaderpack",
            confirm_delete: "confirm_delete_shaderpack",
        },
    }
}

/// A mod's own name, else the file name.
pub fn display_name(item: &ContentItem) -> String {
    item.name.clone().unwrap_or_else(|| item.filename.clone())
}

/// What a search looks for: `query` without outer spaces, in lower case (made once per list).
pub fn search_text(query: &str) -> String {
    query.trim().to_lowercase()
}

/// The name, file name, mod id or version contains `wanted` (a `search_text`; case is ignored).
pub fn matches(item: &ContentItem, wanted: &str) -> bool {
    wanted.is_empty()
        || [
            item.name.as_deref(),
            Some(item.filename.as_str()),
            item.mod_id.as_deref(),
            item.version.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|v| v.to_lowercase().contains(wanted))
}

/// At most `max` characters, with "…" when cut.
pub fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>().trim_end())
    }
}

/// "version • description" of a mod (either may be missing).
pub fn subtitle(item: &ContentItem) -> String {
    [item.version.clone(), item.description.as_deref().map(|d| short(d, 100))]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" • ")
}

/// The page follows its address: another build is another page (the router keeps a route when
/// only its query changes).
#[component]
pub fn ContentRoute() -> impl IntoView {
    let query_map = use_query_map();
    let build = Memo::new(move |_| query_map.with(|q| q.get("build").unwrap_or_default()));
    move || view! { <ContentPage key=build.get() /> }
}

#[component]
pub fn ContentPage(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let header = use_header();
    let flow = use_launch_flow();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let query_map = use_query_map();
    let parts = use_module_parts();
    let module_tabs = move || parts.with(|p| p.tabs.clone());
    let tab = RwSignal::new(normalize_tab(
        &query_map.with_untracked(|q| q.get("tab").unwrap_or_default()),
        &parts.get_untracked().tabs,
    ));
    // A memo: the list is searched once for all its readers, and an equal build wakes none.
    let build: Signal<Option<BuildDto>> = {
        let key = key.clone();
        Memo::new(move |_| store.builds.with(|b| b.iter().find(|b| b.key == key).cloned())).into()
    };
    // The address and the open tab agree: a link to another tab of this build (the crash
    // dialog's, say) opens it, and a picked tab replaces the address.
    Effect::new(move |_| {
        let wanted = query_map.with(|q| q.get("tab").unwrap_or_default());
        let wanted = normalize_tab(&wanted, &parts.get_untracked().tabs);
        if tab.get_untracked() != wanted {
            tab.set(wanted);
        }
    });
    {
        let (key, navigate) = (key.clone(), use_navigate());
        Effect::new(move |_| {
            let current = tab.get();
            if query_map.with_untracked(|q| q.get("tab")).as_deref() != Some(current) {
                navigate(
                    &content_path(&key, current),
                    NavigateOptions { replace: true, ..Default::default() },
                );
            }
        });
    }
    Effect::new(move |_| {
        let name = build.with(|b| b.as_ref().map(|b| b.name.clone()));
        header.subtitle.set(name);
    });
    // A build deleted here or from its menu leaves the page.
    Effect::new(move |was_there: Option<bool>| {
        let there = build.with(|b| b.is_some());
        if was_there == Some(true) && !there {
            store.go("/builds");
        }
        there
    });
    // Settings stay mounted once opened, so unsaved edits survive other tabs.
    let settings_seen = RwSignal::new(false);
    Effect::new(move |_| {
        if tab.get() == "settings" {
            settings_seen.set(true);
        }
    });
    // The picked source stays across tabs; where a tab does not offer it, "Installed" shows.
    let source = RwSignal::new(INSTALLED.to_string());
    // How many the open tab has installed: a count on "Installed" (the installed list tells it).
    let installed_count = RwSignal::new(None::<usize>);
    Effect::new(move |_| {
        tab.track();
        installed_count.set(None);
    });
    // The providers of this tab's kind this build can use.
    let offered = Memo::new(move |_| {
        let Some(kind) = kind_of_tab(tab.get()) else { return Vec::new() };
        let client = build.with(|b| b.as_ref().and_then(|b| b.client.clone()));
        let all = store.info.with(|i| providers(i.as_ref()));
        content_providers(&all, kind, client.as_deref())
    });
    let active = Memo::new(move |_| {
        let picked = source.get();
        if offered.with(|ps| ps.iter().any(|p| p.id == picked)) { picked } else { INSTALLED.to_string() }
    });
    let source_options = Signal::derive(move || {
        let mut options = vec![
            SegOption::new(INSTALLED, i18n.t("installed_content_tab"))
                .with_icon("inventory_2")
                .with_count(installed_count.get()),
        ];
        options.extend(offered.get().into_iter().map(|p| SegOption::new(p.id, p.name).with_icon(p.icon)));
        options
    });
    // Where to look, in the panel's own toolbar right after its search (the tab bar keeps the
    // same width on every tab).
    let sources = ViewFn::from(move || {
        view! {
            <Show when=move || !offered.with(Vec::is_empty)>
                <Segmented options=source_options value=source />
            </Show>
        }
    });
    let header_actions = move || {
        view! {
            <IconAction
                icon="play_arrow"
                tone=ActionTone::Ok
                title=t("play")
                // Being started: Play waits, as on Home.
                loading=Signal::derive(move || {
                    build.with(|b| b.as_ref().is_some_and(|b| store.launching.with(|s| s.contains(&b.key))))
                })
                on_click=Callback::new(move |()| {
                    if let Some(b) = build.get_untracked() {
                        flow.start(b);
                    }
                })
            />
        }
    };
    // The build's tabs: none for mods and shader packs without a mod loader. Until the build is
    // known, every tab stays (a link to mods must not jump away on a slow start).
    let client = Memo::new(move |_| build.with(|b| b.as_ref().map(|b| b.client.clone())));
    Effect::new(move |_| {
        if let Some(client) = client.get() {
            let current = tab.get_untracked();
            let fitting = if module_tabs().iter().any(|t| t.id == current) {
                current
            } else {
                tab_for(current, client.as_deref())
            };
            if fitting != current {
                tab.set(fitting);
            }
        }
    });
    let chips = move || -> Vec<ChipDef> {
        let tabs = client.with(|c| c.as_ref().map_or_else(|| TABS.to_vec(), |c| tabs_for(c.as_deref())));
        with_module_tabs(tabs, &module_tabs())
            .into_iter()
            .map(|(id, icon, label)| {
                let module_tone = module_tabs().into_iter().find(|m| m.id == id).map(|m| m.tone);
                ChipDef { id, icon, label: t(label), tone: module_tone.unwrap_or_else(|| tab_tone(id)) }
            })
            .collect()
    };
    let panel_key = key.clone();
    let panel = move || {
        let key = panel_key.clone();
        let sources = sources.clone();
        match tab.get() {
            "settings" => None,
            "screenshots" => Some(view! { <ScreenshotsPanel key=key /> }.into_any()),
            "delete" => Some(view! { <DeletePanel current=build /> }.into_any()),
            id if module_tabs().iter().any(|t| t.id == id) => {
                module_tabs().into_iter().find(|t| t.id == id).map(|t| (t.view)(key))
            }
            id => {
                let kind = kind_of_tab(id).unwrap_or(ContentKind::Mods);
                let active = active.get();
                let providers = offered.get();
                Some(match providers.iter().find(|p| p.id == active).cloned() {
                    Some(provider) => {
                        view! { <ProviderPanel provider=provider key=key kind=kind current=build sources=sources /> }
                            .into_any()
                    }
                    None => view! {
                        <InstalledPanel key=key kind=kind current=build providers=providers sources=sources count=installed_count />
                    }
                    .into_any(),
                })
            }
        }
    };
    let settings_key = key;

    view! {
        <PageHeader title_key="manage_mods" back="/builds" actions=ViewFn::from(header_actions) />
        <div class="create content">
            <nav class="content__nav">
                {move || view! { <ChipTabs chips=chips() active=tab /> }}
            </nav>
            {panel}
            <Show when=move || settings_seen.get()>
                <div class="content__keep" class:is-hidden=move || tab.get() != "settings">
                    <BuildSettingsPanel key=settings_key.clone() />
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(file: &str) -> ContentItem {
        ContentItem {
            file: file.into(),
            filename: file.trim_end_matches(".disabled").into(),
            size: 1,
            enabled: !file.ends_with(".disabled"),
            folder: false,
            toggle_supported: true,
            name: None,
            version: None,
            description: None,
            mod_id: None,
            has_backup: false,
        }
    }

    #[test]
    fn a_build_s_content_action_shows_the_tab_it_opens() {
        assert_eq!(tab_icon(tab_for("mods", Some("Fabric"))), "extension");
        assert_eq!(
            tab_icon(tab_for("mods", Some("Minecraft"))),
            "palette",
            "a vanilla build opens its packs"
        );
        assert_eq!(tab_icon("nonsense"), "extension");
    }

    #[test]
    fn content_is_reached_by_build_and_tab() {
        assert_eq!(content_path("aeronautics", "mods"), "/builds/content?build=aeronautics&tab=mods");
        assert_eq!(
            content_path("моя збірка", "shaders"),
            "/builds/content?build=%D0%BC%D0%BE%D1%8F%20%D0%B7%D0%B1%D1%96%D1%80%D0%BA%D0%B0&tab=shaders"
        );
    }

    fn backups_tab() -> ModuleTab {
        ModuleTab {
            module: "backups",
            id: "backups",
            icon: "backup",
            label: "world_backups_content_tab",
            tone: TagTone::NeoForge,
            after: "shaders",
            view: |_| ().into_any(),
        }
    }

    #[test]
    fn a_module_tab_goes_after_the_tab_it_names() {
        let extra = [backups_tab()];
        let ids =
            |client| with_module_tabs(tabs_for(client), &extra).into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(
            ids(Some("Fabric")),
            vec!["mods", "resourcepacks", "shaders", "backups", "screenshots", "settings", "delete"]
        );
        assert_eq!(
            ids(Some("Minecraft")),
            vec!["resourcepacks", "backups", "screenshots", "settings", "delete"],
            "without a shaders tab it goes after the nearest tab before it"
        );
        assert_eq!(normalize_tab("backups", &extra), "backups");
        assert_eq!(normalize_tab("backups", &[]), "mods", "no module, no tab");
    }

    #[test]
    fn tabs_follow_the_original_and_map_to_kinds() {
        let ids: Vec<&str> = TABS.iter().map(|(id, ..)| *id).collect();
        assert_eq!(ids, ["mods", "resourcepacks", "shaders", "screenshots", "settings", "delete"]);
        assert_eq!(kind_of_tab("screenshots"), None);
        for kind in ContentKind::ALL {
            assert_eq!(kind_of_tab(tab_of(kind)), Some(kind));
        }
        assert_eq!((kind_of_tab("settings"), kind_of_tab("delete")), (None, None));
        assert_eq!(
            (normalize_tab("delete", &[]), normalize_tab("nope", &[]), normalize_tab("", &[])),
            ("delete", "mods", "mods")
        );
        assert_eq!(texts(ContentKind::ShaderPacks).confirm_delete, "confirm_delete_shaderpack");
        assert_eq!(texts(ContentKind::ResourcePacks).empty, "no_resourcepacks_installed");
        assert_eq!(texts(ContentKind::Mods).enable, "enable_mod");
    }

    #[test]
    fn items_are_found_by_name_file_id_or_version() {
        let mut sodium = item("sodium-fabric.jar");
        sodium.name = Some("Sodium".into());
        sodium.mod_id = Some("sodium".into());
        sodium.version = Some("0.6.0".into());
        assert_eq!(display_name(&sodium), "Sodium");
        assert_eq!(display_name(&item("a.zip")), "a.zip");
        for query in ["", "  SOD ", "fabric", "0.6", "sodium"] {
            assert!(matches(&sodium, &search_text(query)), "{query}");
        }
        assert!(!matches(&sodium, &search_text("iris")));
    }

    #[test]
    fn a_search_is_trimmed_and_lowercased_once() {
        assert_eq!(search_text("  SoD "), "sod");
        assert_eq!(search_text("   "), "");
        assert_eq!(search_text("  ЇЖАК "), "їжак");
        let mut pack = item("pack.zip");
        pack.name = Some("Їжак".into());
        assert!(matches(&pack, &search_text("ЇЖ")), "any case, any alphabet");
        assert!(matches(&pack, &search_text("")), "an empty search leaves everything");
    }

    #[test]
    fn mod_lines_are_short() {
        assert_eq!(short("  abc  ", 5), "abc");
        assert_eq!(short(&"я".repeat(120), 100), format!("{}…", "я".repeat(100)));
        let mut sodium = item("sodium.jar");
        assert_eq!(subtitle(&sodium), "");
        sodium.version = Some("0.6.0".into());
        sodium.description = Some("Fast".into());
        assert_eq!(subtitle(&sodium), "0.6.0 • Fast");
    }

    #[test]
    fn vanilla_builds_have_no_mod_or_shader_tabs() {
        let ids = |client| tabs_for(client).iter().map(|(id, ..)| *id).collect::<Vec<_>>();
        assert_eq!(ids(Some("Minecraft")), ["resourcepacks", "screenshots", "settings", "delete"]);
        assert_eq!(ids(Some("Fabric")), TABS.iter().map(|(id, ..)| *id).collect::<Vec<_>>());
        assert_eq!(tab_for("mods", Some("Minecraft")), "resourcepacks");
        assert_eq!(tab_for("shaders", Some("Minecraft")), "resourcepacks");
        assert_eq!(tab_for("settings", Some("Minecraft")), "settings");
        assert_eq!(tab_for("mods", Some("NeoForge")), "mods");
    }

    #[test]
    fn every_tab_has_its_own_colour() {
        let tones: Vec<TagTone> = TABS.iter().map(|(id, ..)| tab_tone(id)).collect();
        for (i, tone) in tones.iter().enumerate() {
            assert!(!tones[..i].contains(tone), "{:?} repeats", TABS[i].0);
        }
        assert_eq!(tab_tone("delete"), TagTone::Beta, "deleting reads as a warning");
    }
}
