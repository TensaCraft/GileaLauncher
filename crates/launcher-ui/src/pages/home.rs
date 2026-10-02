//! Home: a grid of build cards; hover shows Play, right-click the build menu.

use leptos::ev::MouseEvent;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::module::provide_home_cards;
use ui_kit::reorder::{Reorder, Reorderable};
use ui_kit::{BuildCard, Button, EmptyState, MenuEntry, Variant, use_context_menu};

use crate::builds::actions::use_build_actions;
use crate::builds::build_subtitle;
use crate::builds::dialogs::use_build_menu;
use crate::builds::launch::use_launch_flow;
use crate::modules::use_module_parts;
use crate::shell::PageHeader;
use crate::store::use_store;

/// The cards the modules add to Home, those whose backend is there (each mounted once).
#[component]
fn ModuleCards() -> impl IntoView {
    let parts = use_module_parts();
    let shown = Memo::new(move |_| parts.with(|p| p.home_cards.iter().map(|c| c.module).collect::<Vec<_>>()));
    move || {
        let modules = shown.get();
        parts.with_untracked(|p| {
            p.home_cards.iter().filter(|c| modules.contains(&c.module)).map(|c| (c.view)()).collect_view()
        })
    }
}

#[component]
pub fn HomePage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let flow = use_launch_flow();
    let build_menu = use_build_menu();
    let menu = use_context_menu();
    let page_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        let entries = vec![MenuEntry::item("add", i18n.t("add_version")).icon("add")];
        menu.open_with(ev.client_x(), ev.client_y(), entries, move |id| {
            if id == "add" {
                store.go("/builds/create");
            }
        });
    };
    // Cards are dragged into the order the user wants (modules' cards stay after them).
    let actions = use_build_actions();
    let drag = Reorder::default();
    let dropped = Callback::new(move |()| actions.reorder(drag));
    // "No builds yet" only when Home has no card at all: modules' cards (a server's builds to
    // install) count too.
    let module_cards = provide_home_cards();
    let empty = move || store.builds_loaded.get() && store.builds.with(Vec::is_empty) && !module_cards.any();
    view! {
        <PageHeader title_key="home_title" />
        <div class="home" on:contextmenu=page_menu>
            <Show when=empty>
                <EmptyState
                    icon="layers"
                    title=Signal::derive(move || i18n.t("empty_builds_title"))
                    desc=Signal::derive(move || i18n.t("empty_builds_desc"))
                >
                    <Button variant=Variant::Primary icon="add" on_click=move |_| store.go("/builds/create")>
                        {move || i18n.t("create_build")}
                    </Button>
                </EmptyState>
            </Show>
            <div class="home__grid">
                    <For
                        each=move || store.builds.get()
                        key=|b| (b.key.clone(), b.name.clone(), b.running, b.image.clone(), b.version.clone())
                        children=move |build| {
                            let key = build.key.clone();
                            let for_play = build.clone();
                            let for_menu = build.clone();
                            view! {
                                <Reorderable reorder=drag key=build.key.clone() horizontal=true on_drop=dropped>
                                <BuildCard
                                    title=build.name.clone()
                                    subtitle=build_subtitle(&build)
                                    image=build.image.clone()
                                    play_label=Signal::derive(move || i18n.t("play"))
                                    running_label=Signal::derive(move || i18n.t("version_running_badge"))
                                    running=build.running
                                    busy=Signal::derive(move || store.launching.with(|s| s.contains(&key)))
                                    on_play=Callback::new(move |()| flow.start(for_play.clone()))
                                    on_menu=Callback::new(move |(x, y)| build_menu.open(for_menu.clone(), x, y))
                                />
                                </Reorderable>
                            }
                        }
                    />
                    <ModuleCards />
            </div>
        </div>
    }
}
