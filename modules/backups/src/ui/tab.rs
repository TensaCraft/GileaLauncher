//! The Backups tab: the build's worlds; a world's backups with a new manual
//! one, restoring and deleting — each with a confirmation.

use launcher_shared::{AppError, ErrorCode, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{
    ActionTone, Button, ConfirmDialog, EmptyState, Icon, IconAction, InFlight, Reloadable, Skeleton, Variant,
    ipc, use_toasts,
};

use launcher_shared::units;

use super::{api, date_time, kind_key};
use crate::dto::{BackupDto, Kind, WorldDto};

/// An error as the tab shows it: a running game asks to close it first.
fn said(i18n: I18nCtx, e: &AppError) -> String {
    if e.code == ErrorCode::GameRunning { i18n.t("world_backup_close_game_first") } else { i18n.error(e) }
}

/// The build and world the tab shows, while the tab is there.
fn opened(key: StoredValue<String>, world: RwSignal<Option<String>>) -> Option<(String, String)> {
    Some((key.try_get_value()?, world.try_get_untracked().flatten()?))
}

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

#[component]
pub fn BackupsTab(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let key = StoredValue::new(key);
    // Each list reloads in place, on its own (a world's backups do not cancel the worlds).
    let (world_list, backup_list) = (Reloadable::<Vec<WorldDto>>::new(), Reloadable::<Vec<BackupDto>>::new());
    let (worlds, backups) = (world_list.shown(), backup_list.shown());
    let world = RwSignal::new(None::<String>);
    // One backup action at a time: a restore, delete or new backup, each waits for the last.
    let (creating, acting) = (InFlight::new(), InFlight::new());
    let pending = RwSignal::new(None::<BackupDto>);
    let (restore_open, delete_open) = (RwSignal::new(false), RwSignal::new(false));

    let load_worlds = move || {
        let Some(request) = world_list.begin() else { return };
        let Some(key) = key.try_get_value() else { return };
        spawn_local(async move {
            let answer = api::worlds(&key).await.map_err(|e| i18n.error(&e));
            if let Some(told) = world_list.finish(request, answer) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    let load_backups = move || {
        // Also called when an action ends: the tab may be gone by then.
        let Some((key, name)) = opened(key, world) else { return };
        let Some(request) = backup_list.begin() else { return };
        spawn_local(async move {
            let answer = api::backups(&key, &name).await.map_err(|e| i18n.error(&e));
            if let Some(told) = backup_list.finish(request, answer) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    load_worlds();
    let open_world = move |name: String| {
        world.set(Some(name));
        // Another world's backups: nothing of the last one's stays shown.
        backup_list.clear();
        load_backups();
    };
    let back = move || {
        world.set(None);
        load_worlds();
    };
    let create = move || {
        let Some(name) = world.get_untracked() else { return };
        let key = key.get_value();
        if !creating.start() {
            return;
        }
        spawn_local(async move {
            if let Err(e) = api::create(&key, &name).await {
                toasts.show(Level::Error, said(i18n, &e), None);
            }
            creating.done();
            load_backups();
        });
    };
    let act = move |restore: bool| {
        let (Some(name), Some(backup)) = (world.get_untracked(), pending.get_untracked()) else { return };
        let key = key.get_value();
        restore_open.set(false);
        delete_open.set(false);
        if !acting.start() {
            return;
        }
        spawn_local(async move {
            let done = if restore {
                api::restore(&key, &name, &backup.zip_name).await
            } else {
                api::delete(&key, &name, &backup.zip_name).await
            };
            if let Err(e) = done {
                toasts.show(Level::Error, said(i18n, &e), None);
            }
            acting.done();
            load_backups();
        });
    };
    let open_dir = move |path: String| {
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, ()>("open_path", &PathArgs { path }).await {
                toasts.show(Level::Error, i18n.error(&e), None);
            }
        });
    };

    let world_row = move |w: WorldDto| {
        let details = i18n.tp(
            "world_backups_world_details",
            &[
                ("size", units::size(w.size, &i18n.lang())),
                ("backups", w.backups.to_string()),
                ("modified", date_time(w.modified)),
            ],
        );
        let (for_row, dir) = (w.folder.clone(), w.backups_dir.clone());
        let folder = (w.backups > 0).then(|| {
            view! {
                <IconAction
                    icon="folder_open"
                    tone=ActionTone::Info
                    title=t("world_backups_open")
                    on_click=Callback::new(move |()| open_dir(dir.clone()))
                />
            }
        });
        view! {
            <div class="list-row build-row is-link" on:click=move |_| open_world(for_row.clone())>
                <div class="build-row__icon"><Icon name="public" /></div>
                <div class="build-row__text">
                    <div class="build-row__name">{w.folder.clone()}</div>
                    <div class="build-row__sub component__meta">{details}</div>
                </div>
                <div class="build-row__actions">{folder}</div>
            </div>
        }
    };
    let backup_row = move |b: BackupDto| {
        let line = i18n.tp(
            "world_backup_row",
            &[
                ("kind", i18n.t(kind_key(b.kind))),
                ("date", date_time(b.created)),
                ("size", units::size(b.size, &i18n.lang())),
            ],
        );
        let icon = if b.kind == Kind::Auto { "schedule" } else { "save" };
        let (for_restore, for_delete) = (b.clone(), b.clone());
        view! {
            <div class="list-row build-row">
                <div class="build-row__icon"><Icon name=icon /></div>
                <div class="build-row__text">
                    <div class="build-row__name">{line}</div>
                    <div class="build-row__sub component__meta">{b.zip_name.clone()}</div>
                </div>
                <div class="build-row__actions">
                    <IconAction
                        icon="settings_backup_restore"
                        tone=ActionTone::Info
                        title=t("restore")
                        loading=acting.running()
                        on_click=Callback::new(move |()| {
                            pending.set(Some(for_restore.clone()));
                            restore_open.set(true);
                        })
                    />
                    <IconAction
                        icon="delete_outline"
                        tone=ActionTone::Danger
                        title=t("delete")
                        loading=acting.running()
                        on_click=Callback::new(move |()| {
                            pending.set(Some(for_delete.clone()));
                            delete_open.set(true);
                        })
                    />
                </div>
            </div>
        }
    };
    let skeleton = || {
        view! {
            <div class="builds__list">
                {(0..3)
                    .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=48 /><Skeleton width=260 /></div> })
                    .collect_view()}
            </div>
        }
    };
    let worlds_body = move || match worlds.get() {
        None => skeleton().into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="error_outline" title=t("world_backups_unavailable") desc=message>
                <Button icon="refresh" on_click=move |_| load_worlds()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(list)) if list.is_empty() => {
            view! { <EmptyState icon="public" title=t("world_backups_no_worlds") /> }.into_any()
        }
        Some(Ok(list)) => {
            view! { <div class="builds__list">{list.into_iter().map(world_row).collect_view()}</div> }
                .into_any()
        }
    };
    let backups_body = move || match backups.get() {
        None => skeleton().into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="error_outline" title=t("world_backups_unavailable") desc=message>
                <Button icon="refresh" on_click=move |_| load_backups()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(list)) if list.is_empty() => {
            view! { <EmptyState icon="backup" title=t("world_backups_empty") /> }.into_any()
        }
        Some(Ok(list)) => {
            view! { <div class="builds__list">{list.into_iter().map(backup_row).collect_view()}</div> }
                .into_any()
        }
    };
    let world_name = move || world.get().unwrap_or_default();
    let confirm_text = move |k: &'static str| Signal::derive(move || i18n.tp(k, &[("world", world_name())]));

    view! {
        <Show
            when=move || world.with(Option::is_some)
            fallback=move || view! {
                <div class="create__bar">
                    <div class="create__tools">
                        <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load_worlds()) />
                    </div>
                </div>
                {worlds_body}
            }
        >
            <div class="create__bar">
                <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| back()>
                    {move || i18n.t("world_backups_back_to_worlds")}
                </Button>
                <span class="create__count">{move || i18n.tp("world_backups_for_world", &[("world", world_name())])}</span>
                <div class="create__tools">
                    <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load_backups()) />
                    <Button
                        variant=Variant::Primary
                        icon="backup"
                        loading=creating.running()
                        on_click=move |_| create()
                    >
                        {move || i18n.t("world_backups_create_now")}
                    </Button>
                </div>
            </div>
            {backups_body}
        </Show>
        <ConfirmDialog
            open=restore_open
            title=t("confirmation")
            message=confirm_text("world_backup_restore_confirm")
            confirm_label=t("restore")
            cancel_label=t("cancel")
            on_confirm=Callback::new(move |()| act(true))
        />
        <ConfirmDialog
            open=delete_open
            danger=true
            title=t("confirmation")
            message=confirm_text("world_backup_delete_confirm")
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=Callback::new(move |()| act(false))
        />
    }
}

#[cfg(test)]
mod tests {
    use leptos::prelude::*;

    use super::opened;

    #[test]
    fn a_tab_that_is_gone_opens_nothing() {
        let owner = Owner::new();
        let (key, world) =
            owner.with(|| (StoredValue::new("aero".to_string()), RwSignal::new(Some("W".to_string()))));
        assert_eq!(opened(key, world), Some(("aero".to_string(), "W".to_string())));
        drop(owner);
        assert_eq!(opened(key, world), None, "after the tab is disposed nothing is read");
    }
}
