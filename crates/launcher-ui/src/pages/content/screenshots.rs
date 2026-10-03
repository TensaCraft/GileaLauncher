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

    let shown = Memo::new(move |_| {
        list.shown().with(|s| match s {
            Some(Ok(shots)) => shots.iter().map(|s| (key.get_value(), s.clone())).collect(),
            _ => Vec::new(),
        })
    });
    let viewer = ViewerState::new();
    let changed = Callback::new(move |()| load());
    let show = move |name: &str, rename: bool| {
        if let Some(at) = shown.with_untracked(|l| l.iter().position(|(_, s)| s.name == name)) {
            viewer.show(at, rename);
        }
    };

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

    let tile_menu = move |name: String, (x, y): (i32, i32)| {
        menu.open_with(x, y, menu_entries(i18n), move |id| match id.as_str() {
            MENU_VIEW => show(&name, false),
            MENU_RENAME => show(&name, true),
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
                    .map(|shot| {
                        let name = shot.name.clone();
                        let for_click = shot.name.clone();
                        view! {
                            <ShotTile
                                shot=shot
                                selecting=false
                                selected=false
                                on_click=Callback::new(move |()| show(&for_click, false))
                                on_menu=Callback::new(move |pos| tile_menu(name.clone(), pos))
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
