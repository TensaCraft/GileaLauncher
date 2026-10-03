//! The Screenshots tab: a build's screenshots as thumbnails, newest first, with the same viewer,
//! menu and rename as the Screenshots page, and their folder.

use launcher_shared::{Level, ScreenshotDto, ShotRef};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ConfirmDialog, EmptyState, IconAction, Reloadable, Skeleton, ipc, use_context_menu, use_toasts,
};

use crate::shots::actions::use_shot_actions;
use crate::shots::tile::{
    MENU_COPY, MENU_DELETE, MENU_OPEN, MENU_RENAME, MENU_REVEAL, MENU_VIEW, ShotTile, menu_entries,
};
use crate::shots::viewer::{ScreenshotViewer, ViewerState};

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[component]
pub fn ScreenshotsPanel(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let actions = use_shot_actions();
    let menu = use_context_menu();
    let store = crate::store::use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let key = StoredValue::new(key);
    // Reloads in place; the error is already translated.
    let list = Reloadable::<Vec<ScreenshotDto>>::new();
    let load = move || {
        let Some(request) = list.begin() else { return };
        let args = KeyArgs { key: key.get_value() };
        spawn_local(async move {
            let result = ipc::invoke::<_, Vec<ScreenshotDto>>("screenshots_list", &args).await;
            if let Some(told) = list.finish(request, result.map_err(|e| i18n.error(&e))) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    load();

    let shown = Signal::derive(move || {
        list.shown().with(|s| match s {
            Some(Ok(shots)) => shots.iter().map(|s| (key.get_value(), s.clone())).collect(),
            _ => Vec::new(),
        })
    });
    let viewer = ViewerState::new();
    let stay_on = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let Some(name) = stay_on.get() else { return };
        if let Some(at) = shown.with(|list| list.iter().position(|(_, s)| s.name == name)) {
            viewer.at.set(at);
            stay_on.set(None);
        }
    });
    let changed = Callback::new(move |renamed: Option<(String, String)>| {
        stay_on.set(renamed.map(|(_, name)| name));
        load();
    });

    let delete_of = RwSignal::new(None::<String>);
    let delete_open = RwSignal::new(false);
    let delete_text = Signal::derive(move || {
        delete_of
            .with(|n| n.as_ref().map(|n| i18n.tp("confirm_delete_screenshot", &[("name", n.clone())])))
            .unwrap_or_default()
    });
    let deleted = Callback::new(move |_: usize| load());
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        let Some(name) = delete_of.get_untracked() else { return };
        actions.delete(vec![ShotRef { key: key.get_value(), name }], deleted);
    });

    let tile_menu = move |name: String, at: usize, (x, y): (i32, i32)| {
        menu.open_with(x, y, menu_entries(i18n), move |id| match id.as_str() {
            MENU_VIEW => viewer.show(at, false),
            MENU_RENAME => viewer.show(at, true),
            MENU_COPY => actions.copy(key.get_value(), name.clone()),
            MENU_OPEN => actions.open(key.get_value(), name.clone()),
            MENU_REVEAL => actions.reveal(key.get_value(), name.clone()),
            MENU_DELETE => {
                delete_of.set(Some(name.clone()));
                delete_open.set(true);
            }
            _ => {}
        });
    };

    let body = move || {
        match list.shown().get() {
        None => view! {
            <div class="shot-grid">{(0..4).map(|_| view! { <div class="shot-tile is-skeleton"><Skeleton width=120 /></div> }).collect_view()}</div>
        }
        .into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="error_outline" title=t("unknown_error") desc=message>
                <Button icon="refresh" on_click=move |_| load()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(shots)) if shots.is_empty() => {
            view! { <EmptyState icon="photo_library" title=t("screenshots_empty") desc=t("shots_none_desc") /> }.into_any()
        }
        Some(Ok(shots)) => view! {
            <div class="shot-grid">
                {shots
                    .into_iter()
                    .enumerate()
                    .map(|(at, shot)| {
                        let name = shot.name.clone();
                        view! {
                            <ShotTile
                                shot=shot
                                selecting=false
                                selected=false
                                on_click=Callback::new(move |()| viewer.show(at, false))
                                on_menu=Callback::new(move |pos| tile_menu(name.clone(), at, pos))
                            />
                        }
                    })
                    .collect_view()}
            </div>
        }
        .into_any(),
    }
    };

    view! {
        <div class="create__bar">
            <div class="create__tools">
                <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load()) />
                <Button icon="folder_open" on_click=move |_| actions.open_dir(key.get_value())>
                    {move || i18n.t("open_screenshots_folder")}
                </Button>
            </div>
        </div>
        {body}
        <ScreenshotViewer
            state=viewer
            list=shown
            build_name=Callback::new(move |key: String| {
                store.builds.with(|b| b.iter().find(|b| b.key == key).map(|b| b.name.clone())).unwrap_or(key)
            })
            on_changed=changed
        />
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
