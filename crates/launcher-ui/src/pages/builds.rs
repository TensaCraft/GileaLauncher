//! Builds: one row per build with Play/Stop, Copy and Open folder, the build
//! menu on right-click, and an empty state (a port addition).

use launcher_shared::BuildDto;
use launcher_shared::provider::{ModpackBuild, ProviderInfo};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::reorder::{Reorder, Reorderable};
use ui_kit::{ActionTone, Button, EmptyState, Icon, IconAction, Tag, TagTone, Variant};

use crate::builds::actions::use_build_actions;
use crate::builds::dialogs::{use_build_dialogs, use_build_menu};
use crate::builds::launch::use_launch_flow;
use crate::builds::{build_subtitle, loader_icon};
use crate::providers::pack_updates::{self, PackUpdateDialog, updates};
use crate::providers::{modpack_update_providers, providers};
use crate::shell::PageHeader;
use crate::store::use_store;

/// Icon and translation key of a row's first action.
pub fn primary_action(running: bool) -> (&'static str, &'static str) {
    if running { ("stop", "version_stop") } else { ("play_arrow", "play") }
}

#[component]
pub fn BuildsPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let add = move || {
        view! {
            <Button icon="construction" on_click=move |_| store.go("/builds/components")>
                {move || i18n.t("minecraft_components_nav")}
            </Button>
            <Button variant=Variant::Primary icon="add" on_click=move |_| store.go("/builds/create")>
                {move || i18n.t("add_version")}
            </Button>
        }
    };
    let empty = move || store.builds_loaded.get() && store.builds.with(Vec::is_empty);
    // Modpack updates: asked again when a build comes or goes, not when one starts or stops.
    let keys = Memo::new(move |_| store.builds.with(|b| b.iter().map(|b| b.key.clone()).collect::<Vec<_>>()));
    let offered = Memo::new(move |_| modpack_update_providers(&store.info.with(|i| providers(i.as_ref()))));
    Effect::new(move |_| {
        keys.track();
        pack_updates::load(&offered.get());
    });
    let (pack_open, pack_target) =
        (RwSignal::new(false), RwSignal::new(None::<(ProviderInfo, ModpackBuild)>));
    let update_pack = Callback::new(move |target| {
        pack_target.set(Some(target));
        pack_open.set(true);
    });
    // Rows are dragged into the order the user wants.
    let drag = Reorder::default();
    let reorder_actions = use_build_actions();
    let dropped = Callback::new(move |()| reorder_actions.reorder(drag));
    view! {
        <PageHeader title_key="builds_title" actions=ViewFn::from(add) />
        <div class="builds">
            <Show
                when=move || !empty()
                fallback=move || view! {
                    <EmptyState
                        icon="layers"
                        title=Signal::derive(move || i18n.t("empty_builds_title"))
                        desc=Signal::derive(move || i18n.t("empty_builds_desc"))
                    >
                        <Button variant=Variant::Primary icon="add" on_click=move |_| store.go("/builds/create")>
                            {move || i18n.t("create_build")}
                        </Button>
                    </EmptyState>
                }
            >
                <div class="builds__list">
                    <For
                        each=move || store.builds.get()
                        key=|b| (b.key.clone(), b.name.clone(), b.running, b.image.clone(), b.version.clone())
                        children=move |item| view! {
                            <Reorderable reorder=drag key=item.key.clone() on_drop=dropped>
                                <BuildRow item=item update_pack=update_pack />
                            </Reorderable>
                        }
                    />
                </div>
            </Show>
        </div>
        <PackUpdateDialog open=pack_open target=pack_target />
    }
}

#[component]
fn BuildRow(item: BuildDto, update_pack: Callback<(ProviderInfo, ModpackBuild)>) -> impl IntoView {
    let build = item;
    let i18n = use_i18n();
    let flow = use_launch_flow();
    let actions = use_build_actions();
    let dialogs = use_build_dialogs();
    let build_menu = use_build_menu();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let running = build.running;
    let (icon, label) = primary_action(running);
    let (for_main, for_copy, for_open, for_menu) =
        (build.clone(), build.clone(), build.key.clone(), build.clone());
    let for_settings = build.key.clone();
    let for_content = build.key.clone();
    let for_pack = build.key.clone();
    // Offered only while the modpack has a newer version: an occasional action, before the permanent ones.
    let pack_update = move || {
        let (provider, pack) = updates().update_of(&for_pack)?;
        let version = pack.newest.as_ref().map(|v| v.version_number.clone()).unwrap_or_default();
        let key = pack.key.clone();
        Some(view! {
            <IconAction
                icon="upgrade"
                tone=ActionTone::Ok
                title=Signal::derive(move || i18n.tp("modpack_update_to", &[("version", version.clone())]))
                loading=Signal::derive(move || updates().updating(&key))
                on_click=Callback::new(move |()| update_pack.run((provider.clone(), pack.clone())))
            />
        })
    };
    let store = use_store();
    let for_row = build.key.clone();
    // The build's first content tab: mods, or resource packs without a mod loader.
    let first_tab = crate::pages::content::tab_for("mods", build.client.as_deref());
    let on_open = move |_| store.go(&crate::pages::content::content_path(&for_row, first_tab));
    // Being started: Play waits, as on Home.
    let starting = {
        let key = build.key.clone();
        Signal::derive(move || store.launching.with(|s| s.contains(&key)))
    };
    let main = Callback::new(move |()| {
        if running {
            actions.stop(for_main.key.clone());
        } else {
            flow.start(for_main.clone());
        }
    });
    let on_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        build_menu.open(for_menu.clone(), ev.client_x(), ev.client_y());
    };
    let picture = match build.image.clone().filter(|s| !s.trim().is_empty()) {
        Some(src) => view! { <img src=src alt="" /> }.into_any(),
        None => view! { <Icon name=loader_icon(build.client.as_deref().or(build.loader.as_deref())) /> }
            .into_any(),
    };
    view! {
        <div class="list-row build-row is-link" class:is-running=running on:contextmenu=on_menu on:click=on_open>
            <div class="build-row__icon">{picture}</div>
            <div class="build-row__text">
                <div class="build-row__name">{build.name.clone()}</div>
                <div class="build-row__sub">{build_subtitle(&build)}</div>
            </div>
            {running.then(|| view! { <Tag tone=TagTone::Vanilla>{move || i18n.t("version_running_badge")}</Tag> })}
            <div class="build-row__actions">
                {pack_update}
                <IconAction
                    icon=icon
                    tone=if running { ActionTone::Danger } else { ActionTone::Ok }
                    title=t(label)
                    loading=starting
                    on_click=main
                />
                <IconAction icon="content_copy" title=t("copy") on_click=Callback::new(move |()| dialogs.copy(for_copy.clone())) />
                <IconAction
                    icon=crate::pages::content::tab_icon(first_tab)
                    title=t("manage_mods")
                    on_click=Callback::new(move |()| store.go(&crate::pages::content::content_path(&for_content, first_tab)))
                />
                <IconAction
                    icon="tune"
                    title=t("version_settings_content_tab")
                    on_click=Callback::new(move |()| store.go(&crate::pages::content::content_path(&for_settings, "settings")))
                />
                <IconAction
                    icon="folder_open"
                    tone=ActionTone::Info
                    title=t("open_directory")
                    on_click=Callback::new(move |()| actions.open_dir(for_open.clone()))
                />
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rows_show_play_or_stop() {
        assert_eq!(primary_action(false), ("play_arrow", "play"));
        assert_eq!(primary_action(true), ("stop", "version_stop"));
    }
}
