//! A group as the page and a build's tab show it: name, mode, builds, paths, state and its actions.

use launcher_shared::{BuildDto, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, ConfirmDialog, Icon, IconAction, InFlight, Tag, TagTone, use_toasts};

use super::model::{build_name, mode_key, shown_path, state_key};
use super::{api, state};
use crate::dto::{GroupState, GroupView, error_text};

/// The short line «Як це працює» the empty page and the editor show.
#[component]
pub fn HowItWorks() -> impl IntoView {
    let i18n = use_i18n();
    let lines = ["sync_how_when", "sync_how_wins", "sync_how_history", "sync_how_packs"];
    view! {
        <div class="sync-how">
            <div class="sync-how__title"><Icon name="lightbulb" />{move || i18n.t("sync_how_title")}</div>
            <ul>{lines.map(|key| view! { <li>{move || i18n.t(key)}</li> }).collect_view()}</ul>
        </div>
    }
}

fn state_tone(state: GroupState) -> TagTone {
    match state {
        GroupState::Synced => TagTone::Vanilla,
        GroupState::Waiting | GroupState::Idle => TagTone::Warn,
        GroupState::Failed => TagTone::Danger,
        GroupState::New => TagTone::Neutral,
    }
}

#[component]
pub fn GroupCard(
    view: GroupView,
    #[prop(into)] builds: Signal<Vec<BuildDto>>,
    on_open: Callback<GroupView>,
    on_edit: Callback<GroupView>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let (syncing, deleting) = (InFlight::new(), InFlight::new());
    let confirm = RwSignal::new(false);
    let group = view.group.clone();
    let names = {
        let keys = group.builds.clone();
        Signal::derive(move || {
            builds.with(|all| {
                keys.iter()
                    .map(|k| {
                        build_name(all, k)
                            .unwrap_or_else(|| i18n.tp("sync_build_gone", &[("build", k.clone())]))
                    })
                    .collect::<Vec<_>>()
            })
        })
    };
    let paths = group.paths.iter().map(|p| shown_path(p)).collect::<Vec<_>>().join(", ");
    let id = StoredValue::new(group.id.clone());
    let name = StoredValue::new(group.name.clone());
    let sync = move || {
        if !syncing.start() {
            return;
        }
        spawn_local(async move {
            match api::sync(&id.get_value()).await {
                Ok(report) if report.waiting => {
                    toasts.show(Level::Warning, i18n.t("sync_waiting_toast"), None)
                }
                Ok(report) => {
                    let count = (report.written + report.removed).to_string();
                    toasts.show(
                        Level::Success,
                        i18n.tp("sync_done_toast", &[("group", name.get_value()), ("count", count)]),
                        None,
                    )
                }
                Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
            }
            state::refresh();
            syncing.done();
        });
    };
    let delete = move || {
        if !deleting.start() {
            return;
        }
        spawn_local(async move {
            if let Err(e) = api::delete(&id.get_value()).await {
                toasts.show(Level::Error, i18n.text(&error_text(&e)), None);
            }
            state::refresh();
            deleting.done();
        });
    };
    let when = match (view.state, view.last_ms) {
        (GroupState::Synced, Some(ms)) => i18n.tp("sync_state_synced_at", &[("time", state::date_time(ms))]),
        (state, _) => i18n.t(state_key(state)),
    };
    let note = view.note.clone().map(|n| i18n.text(&n));
    let (for_open, for_edit) = (view.clone(), view.clone());
    view! {
        <div class="list-row sync-card">
            <div class="sync-card__icon"><Icon name="sync" /></div>
            <div class="sync-card__text">
                <div class="sync-card__name">
                    {group.name.clone()}
                    <Tag tone=TagTone::Info>{move || i18n.t(mode_key(group.mode))}</Tag>
                </div>
                <div class="sync-card__builds">{move || names.get().join(" · ")}</div>
                <div class="sync-card__paths">{paths}</div>
                <div class="sync-card__state">
                    <Tag tone=state_tone(view.state)>{when}</Tag>
                    {note.map(|n| view! { <span class="sync-card__note">{n}</span> })}
                </div>
            </div>
            <div class="sync-card__actions">
                <IconAction icon="sync" title=t("sync_now") loading=syncing.running() on_click=Callback::new(move |()| sync()) />
                <IconAction icon="history" title=t("sync_open") on_click=Callback::new(move |()| on_open.run(for_open.clone())) />
                <IconAction icon="edit" title=t("sync_edit") on_click=Callback::new(move |()| on_edit.run(for_edit.clone())) />
                <IconAction
                    icon="delete_outline"
                    tone=ActionTone::Danger
                    title=t("delete")
                    loading=deleting.running()
                    on_click=Callback::new(move |()| confirm.set(true))
                />
            </div>
        </div>
        <ConfirmDialog
            open=confirm
            title=t("confirmation")
            message=Signal::derive(move || i18n.tp("sync_delete_confirm", &[("group", name.get_value())]))
            confirm_label=t("delete")
            cancel_label=t("cancel")
            danger=true
            on_confirm=Callback::new(move |()| delete())
        />
    }
}
