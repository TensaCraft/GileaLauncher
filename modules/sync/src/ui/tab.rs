//! A build's «Синхронізація» tab: its groups, «Синхронізувати зараз» for all of them, and a new
//! group starting from this build.

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, EmptyState, InFlight, Variant, use_toasts};

use super::card::{GroupCard, HowItWorks};
use super::editor::EditorDialog;
use super::group::GroupDialog;
use super::page::load_builds;
use super::{api, state};
use crate::dto::{GroupView, SyncGroup};

#[component]
pub fn SyncTab(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    state::refresh();
    let groups = state::groups();
    let builds = load_builds();
    let key = StoredValue::new(key);
    let syncing = InFlight::new();
    let (editor_open, editing) = (RwSignal::new(false), RwSignal::new(SyncGroup::default()));
    let (group_open, opened) = (RwSignal::new(false), RwSignal::new(None::<GroupView>));
    let create = move || {
        let me = key.get_value();
        editing.set(SyncGroup { builds: vec![me.clone()], start: me, ..SyncGroup::default() });
        editor_open.set(true);
    };
    let on_edit = Callback::new(move |view: GroupView| {
        editing.set(view.group);
        group_open.set(false);
        editor_open.set(true);
    });
    let on_open = Callback::new(move |view: GroupView| {
        opened.set(Some(view));
        group_open.set(true);
    });
    let sync_all = move || {
        if !syncing.start() {
            return;
        }
        spawn_local(async move {
            match api::sync_build(&key.get_value()).await {
                Ok(results) => {
                    let failed: Vec<String> =
                        results.into_iter().filter_map(|(g, e)| e.map(|e| format!("{g}: {e}"))).collect();
                    if failed.is_empty() {
                        toasts.show(Level::Success, i18n.t("sync_build_done"), None);
                    } else {
                        toasts.show(Level::Error, failed.join("; "), None);
                    }
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
            state::refresh();
            syncing.done();
        });
    };
    let mine = move || -> Option<Vec<GroupView>> {
        groups.with(|list| {
            list.as_ref()
                .map(|l| l.iter().filter(|v| v.group.builds.contains(&key.get_value())).cloned().collect())
        })
    };
    let body = move || {
        match mine() {
        None => ().into_any(),
        Some(list) if list.is_empty() => view! {
            <EmptyState icon="sync" title=Signal::derive(move || i18n.t("sync_tab_empty")) desc=Signal::derive(move || i18n.t("sync_empty_desc"))>
                <Button variant=Variant::Primary icon="add" on_click=move |_| create()>{move || i18n.t("sync_create")}</Button>
            </EmptyState>
            <HowItWorks />
        }
        .into_any(),
        Some(list) => view! {
            <div class="sync-toolbar">
                <Button icon="add" on_click=move |_| create()>{move || i18n.t("sync_create")}</Button>
                <Button variant=Variant::Primary icon="sync" loading=syncing.running() on_click=move |_| sync_all()>
                    {move || i18n.t("sync_now")}
                </Button>
            </div>
            <div class="sync-list">
                {list.into_iter().map(|view| view! { <GroupCard view=view builds=builds on_open=on_open on_edit=on_edit /> }).collect_view()}
            </div>
        }
        .into_any(),
    }
    };
    view! {
        <div class="sync-tab">{body}</div>
        <EditorDialog open=editor_open initial=editing builds=builds />
        <GroupDialog open=group_open view=opened builds=builds on_edit=on_edit />
    }
}
