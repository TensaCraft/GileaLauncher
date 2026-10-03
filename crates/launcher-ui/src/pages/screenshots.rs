//! Screenshots: every build's screenshots in sections that fold, with a search, a build filter, a
//! sort, a viewer and a select mode for deleting several.

use std::collections::BTreeSet;

use launcher_shared::{BuildDto, BuildShots, ShotRef};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionGroup, Button, ConfirmDialog, EmptyState, Icon, IconAction, Select, SelectOption, Skeleton,
    TextInput, Variant, ipc, use_context_menu,
};

use crate::builds::loader_icon;
use crate::fold::fold_state;
use crate::shell::PageHeader;
use crate::shots::actions::use_shot_actions;
use crate::shots::tile::{
    MENU_COPY, MENU_DELETE, MENU_OPEN, MENU_RENAME, MENU_REVEAL, MENU_VIEW, ShotTile, menu_entries,
};
use crate::shots::viewer::{ScreenshotViewer, ViewerState};
use crate::shots::{ShotSort, flat, shown_groups, totals};
use crate::store::use_store;

/// The picture of a build in a section's title: its own, else its loader's icon.
fn cover(build: Option<&BuildDto>) -> AnyView {
    match build.and_then(|b| b.image.clone()).filter(|s| !s.trim().is_empty()) {
        Some(src) => view! { <img src=src alt="" draggable="false" /> }.into_any(),
        None => {
            let loader = build.and_then(|b| b.client.as_deref().or(b.loader.as_deref()));
            view! { <Icon name=loader_icon(loader) /> }.into_any()
        }
    }
}

#[component]
pub fn ScreenshotsPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_shot_actions();
    let menu = use_context_menu();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    // `None` while the first answer is on its way; an answer to an older request is dropped.
    let all = RwSignal::new(None::<Result<Vec<BuildShots>, String>>);
    let asked = StoredValue::new(0u64);
    let load = move || {
        let turn = asked.get_value() + 1;
        asked.set_value(turn);
        spawn_local(async move {
            let answer = ipc::call::<Vec<BuildShots>>("screenshots_all").await;
            if asked.get_value() == turn {
                all.set(Some(answer.map_err(|e| i18n.error(&e))));
            }
        });
    };
    // Again when a game stops: it may have left new screenshots.
    let running = Memo::new(move |_| {
        store.builds.with(|b| b.iter().filter(|b| b.running).map(|b| b.key.clone()).collect::<Vec<_>>())
    });
    Effect::new(move |_| {
        running.track();
        load();
    });

    let query = RwSignal::new(String::new());
    let only = RwSignal::new(String::new());
    let sort = RwSignal::new(ShotSort::Newest.id().to_string());
    let build_name = Callback::new(move |key: String| {
        store.builds.with(|b| b.iter().find(|b| b.key == key).map(|b| b.name.clone())).unwrap_or(key)
    });
    let groups = Memo::new(move |_| {
        let order: Vec<String> = store.builds.with(|b| b.iter().map(|b| b.key.clone()).collect());
        all.with(|a| match a {
            Some(Ok(all)) => {
                let only = only.get();
                shown_groups(
                    all,
                    &order,
                    &query.get(),
                    (!only.is_empty()).then_some(only.as_str()),
                    ShotSort::from_id(&sort.get()),
                )
            }
            _ => Vec::new(),
        })
    });
    let shown = Memo::new(move |_| groups.with(|g| flat(g)));
    let build_options = Signal::derive(move || {
        let mut options = vec![SelectOption::new("", i18n.t("shots_all_builds"))];
        all.with(|a| {
            if let Some(Ok(all)) = a {
                options.extend(all.iter().map(|g| {
                    SelectOption::new(
                        g.key.clone(),
                        format!("{} · {}", build_name.run(g.key.clone()), g.shots.len()),
                    )
                }));
            }
        });
        options
    });
    let sort_options = Signal::derive(move || {
        ShotSort::ALL.iter().map(|s| SelectOption::new(s.id(), i18n.t(s.label_key()))).collect::<Vec<_>>()
    });

    let viewer = ViewerState::new();
    let selecting = RwSignal::new(false);
    let selected = RwSignal::new(BTreeSet::<(String, String)>::new());
    Effect::new(move |_| {
        if !selecting.get() {
            selected.set(BTreeSet::new());
        }
    });
    let changed = Callback::new(move |()| load());
    // Another search or build starts the selection over: what is selected is always shown.
    Effect::new(move |_| {
        query.track();
        only.track();
        selected.set(BTreeSet::new());
    });
    // The viewer opens on a screenshot by what it is, so a list loaded meanwhile cannot shift it.
    let show = move |key: &str, name: &str, rename: bool| {
        if let Some(at) = shown.with_untracked(|l| l.iter().position(|(k, s)| k == key && s.name == name)) {
            viewer.show(at, rename);
        }
    };

    // Deleting from a thumbnail's menu, or the selected ones.
    let delete_items = RwSignal::new(Vec::<ShotRef>::new());
    let delete_open = RwSignal::new(false);
    let delete_text = Signal::derive(move || {
        delete_items.with(|items| match items.as_slice() {
            [one] => i18n.tp("confirm_delete_screenshot", &[("name", one.name.clone())]),
            many => i18n.tp("shots_confirm_delete_many", &[("count", many.len().to_string())]),
        })
    });
    let deleted = Callback::new(move |_: usize| {
        selecting.set(false);
        load();
    });
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        actions.delete(delete_items.get_untracked(), deleted);
    });
    let ask_delete = move |items: Vec<ShotRef>| {
        if !items.is_empty() {
            delete_items.set(items);
            delete_open.set(true);
        }
    };

    let tile_menu = move |key: String, name: String, (x, y): (i32, i32)| {
        menu.open_with(x, y, menu_entries(i18n), move |id| match id.as_str() {
            MENU_VIEW => show(&key, &name, false),
            MENU_RENAME => show(&key, &name, true),
            MENU_COPY => actions.copy(key.clone(), name.clone()),
            MENU_OPEN => actions.open(key.clone(), name.clone()),
            MENU_REVEAL => actions.reveal(key.clone(), name.clone()),
            MENU_DELETE => ask_delete(vec![ShotRef { key: key.clone(), name: name.clone() }]),
            _ => {}
        });
    };

    let group_view = move |group: BuildShots| {
        let key = group.key.clone();
        let folded = fold_state(format!("shots.fold.{key}"));
        let build = store.builds.with_untracked(|b| b.iter().find(|b| b.key == key).cloned());
        let name = build.as_ref().map_or_else(|| key.clone(), |b| b.name.clone());
        let (count, bytes) = totals(&group.shots);
        let meta = format!("{count} · {}", launcher_shared::units::size(bytes, &i18n.lang()));
        let for_dir = key.clone();
        let tiles = group
            .shots
            .into_iter()
            .map(|shot| {
                let id = (key.clone(), shot.name.clone());
                let (for_click, for_check, for_menu) = (id.clone(), id.clone(), id);
                view! {
                    <ShotTile
                        shot=shot
                        selecting=selecting
                        selected=Signal::derive(move || selected.with(|s| s.contains(&for_check)))
                        on_click=Callback::new(move |()| {
                            if selecting.get_untracked() {
                                let id = for_click.clone();
                                selected.update(|s| {
                                    if !s.remove(&id) {
                                        s.insert(id);
                                    }
                                });
                            } else {
                                show(&for_click.0, &for_click.1, false);
                            }
                        })
                        on_menu=Callback::new(move |pos| tile_menu(for_menu.0.clone(), for_menu.1.clone(), pos))
                    />
                }
            })
            .collect_view();
        view! {
            <section class="shot-group">
                <div class="shot-group__head">
                    <button
                        type="button"
                        class="fold-head shot-group__fold"
                        class:is-folded=folded
                        aria-expanded=move || (!folded.get()).to_string()
                        on:click=move |_| {
                            ui_kit::sound::play_click();
                            folded.update(|f| *f = !*f);
                        }
                    >
                        <span class="shot-group__cover">{cover(build.as_ref())}</span>
                        <span class="shot-group__name">{name}</span>
                        <span class="shot-group__meta">{meta}</span>
                        <Icon name="expand_more" class="fold-head__chevron" />
                    </button>
                    <IconAction
                        icon="folder_open"
                        title=t("open_screenshots_folder")
                        on_click=Callback::new(move |()| actions.open_dir(for_dir.clone()))
                    />
                </div>
                <div class="shot-grid" class:is-hidden=folded>{tiles}</div>
            </section>
        }
    };

    let body = move || {
        match all.get() {
        None => view! {
            <div class="shot-grid">{(0..8).map(|_| view! { <div class="shot-tile is-skeleton"><Skeleton width=120 /></div> }).collect_view()}</div>
        }
        .into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="error_outline" title=t("unknown_error") desc=message>
                <Button icon="refresh" on_click=move |_| load()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(all)) if all.is_empty() => {
            view! { <EmptyState icon="photo_library" title=t("shots_none_title") desc=t("shots_none_desc") /> }.into_any()
        }
        Some(Ok(_)) if groups.with(Vec::is_empty) => {
            view! { <EmptyState icon="search_off" title=t("shots_nothing_found") /> }.into_any()
        }
        Some(Ok(_)) => groups.get().into_iter().map(group_view).collect_view().into_any(),
    }
    };

    let select_all = move || {
        let every: BTreeSet<(String, String)> =
            shown.with(|list| list.iter().map(|(k, s)| (k.clone(), s.name.clone())).collect());
        selected.set(every);
    };
    // Only what is shown goes, whatever was selected before.
    let delete_selected = move || {
        let items = shown.with_untracked(|list| {
            selected.with_untracked(|s| {
                list.iter()
                    .filter(|(key, shot)| s.contains(&(key.clone(), shot.name.clone())))
                    .map(|(key, shot)| ShotRef { key: key.clone(), name: shot.name.clone() })
                    .collect()
            })
        });
        ask_delete(items);
    };

    view! {
        <PageHeader title_key="screenshots_title" />
        <div class="shots">
            <div class="create__bar">
                <div class="create__tools">
                    <div class="create__search">
                        <TextInput value=query icon="search" placeholder=t("shots_search") />
                    </div>
                    <div class="shots__select">
                        <Select options=build_options value=only icon="layers" on_change=Callback::new(move |v: String| only.set(v)) />
                    </div>
                    <div class="shots__select">
                        <Select options=sort_options value=sort icon="sort" on_change=Callback::new(move |v: String| sort.set(v)) />
                    </div>
                    {move || {
                        let on = selecting.get();
                        view! {
                            <Button
                                icon="checklist"
                                variant=if on { Variant::Primary } else { Variant::Secondary }
                                on_click=move |_| selecting.update(|s| *s = !*s)
                            >
                                {i18n.t("shots_select")}
                            </Button>
                        }
                    }}
                    <ActionGroup>
                        <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load()) />
                    </ActionGroup>
                </div>
            </div>
            <div class="shots__body">{body}</div>
            <Show when=move || selecting.get()>
                <div class="shots__selection">
                    <span>{move || i18n.tp("shots_selected", &[("count", selected.with(BTreeSet::len).to_string())])}</span>
                    <Button variant=Variant::Ghost icon="select_all" on_click=move |_| select_all()>{move || i18n.t("shots_select_all")}</Button>
                    <Button
                        variant=Variant::Danger
                        icon="delete_outline"
                        disabled=Signal::derive(move || selected.with(BTreeSet::is_empty))
                        on_click=move |_| delete_selected()
                    >
                        {move || i18n.t("delete")}
                    </Button>
                    <Button variant=Variant::Ghost on_click=move |_| selecting.set(false)>{move || i18n.t("cancel")}</Button>
                </div>
            </Show>
        </div>
        <ScreenshotViewer state=viewer list=shown build_name=build_name on_changed=changed />
        <ConfirmDialog
            open=delete_open
            danger=true
            title=t("confirmation")
            message=delete_text
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=confirm_delete
        />
    }
}
