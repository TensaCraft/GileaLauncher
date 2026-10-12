//! The «Синхронізація» page: every sync group, a button to make one, and what sync does when
//! there is none.

use launcher_shared::BuildDto;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, EmptyState, Skeleton, Variant};

use super::card::{GroupCard, HowItWorks};
use super::editor::EditorDialog;
use super::group::GroupDialog;
use super::{api, state};
use crate::dto::{GroupView, SyncGroup};

/// The build list, loaded once for a view.
pub fn load_builds() -> RwSignal<Vec<BuildDto>> {
    let builds = RwSignal::new(Vec::new());
    spawn_local(async move {
        if let Ok(list) = api::builds().await {
            let _ = builds.try_set(list);
        }
    });
    builds
}

#[component]
pub fn SyncPage() -> impl IntoView {
    let i18n = use_i18n();
    state::refresh();
    let groups = state::groups();
    let builds = load_builds();
    let (editor_open, editing) = (RwSignal::new(false), RwSignal::new(SyncGroup::default()));
    let (group_open, opened) = (RwSignal::new(false), RwSignal::new(None::<GroupView>));
    let create = move || {
        editing.set(SyncGroup::default());
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
    let list = move || {
        match groups.get() {
        None => view! { <div class="sync-list"><div class="list-row"><Skeleton /></div></div> }.into_any(),
        Some(list) if list.is_empty() => view! {
            <EmptyState icon="sync" title=Signal::derive(move || i18n.t("sync_empty_title")) desc=Signal::derive(move || i18n.t("sync_empty_desc"))>
                <Button variant=Variant::Primary icon="add" on_click=move |_| create()>{move || i18n.t("sync_create")}</Button>
            </EmptyState>
            <HowItWorks />
        }
        .into_any(),
        Some(list) => view! {
            <div class="sync-toolbar">
                <Button variant=Variant::Primary icon="add" on_click=move |_| create()>{move || i18n.t("sync_create")}</Button>
            </div>
            <div class="sync-list">
                {list.into_iter().map(|view| view! { <GroupCard view=view builds=builds on_open=on_open on_edit=on_edit /> }).collect_view()}
            </div>
        }
        .into_any(),
    }
    };
    view! {
        <div class="page sync-page">{list}</div>
        <EditorDialog open=editor_open initial=editing builds=builds />
        <GroupDialog open=group_open view=opened builds=builds on_edit=on_edit />
    }
}
