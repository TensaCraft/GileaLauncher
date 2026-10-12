//! A group opened: what it syncs and how, and its history with «Відновити».

use launcher_shared::{BuildDto, Level, units};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Dialog, DialogFooter, EmptyState, IconAction, InFlight, TabDef, Tabs, Tag, TagTone, Variant,
    use_toasts,
};

use super::model::{build_name, mode_key, shown_path, state_key};
use super::{api, state};
use crate::dto::{GroupView, HistoryEntry, Reason, error_text};

fn reason_key(reason: Reason) -> &'static str {
    match reason {
        Reason::Changed => "sync_reason_changed",
        Reason::Removed => "sync_reason_removed",
        Reason::Conflict => "sync_reason_conflict",
        Reason::Base => "sync_reason_base",
        Reason::Restored => "sync_reason_restored",
    }
}

#[component]
pub fn GroupDialog(
    open: RwSignal<bool>,
    view: RwSignal<Option<GroupView>>,
    #[prop(into)] builds: Signal<Vec<BuildDto>>,
    on_edit: Callback<GroupView>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let tab = RwSignal::new("overview");
    let history = RwSignal::new(None::<Vec<HistoryEntry>>);
    let restoring = InFlight::new();
    let name_of = move |key: &str| {
        builds.with(|all| {
            build_name(all, key).unwrap_or_else(|| i18n.tp("sync_build_gone", &[("build", key.to_string())]))
        })
    };
    let load = move || {
        let Some(id) = view.with_untracked(|v| v.as_ref().map(|v| v.group.id.clone())) else { return };
        history.set(None);
        spawn_local(async move {
            let list = api::history(&id).await.unwrap_or_default();
            let _ = history.try_set(Some(list));
        });
    };
    Effect::new(move |_| {
        if open.get() {
            tab.set("overview");
            load();
        }
    });
    let restore = move |entry: String| {
        let Some(id) = view.with_untracked(|v| v.as_ref().map(|v| v.group.id.clone())) else { return };
        if !restoring.start() {
            return;
        }
        spawn_local(async move {
            match api::restore(&id, &entry).await {
                Ok(_) => toasts.show(Level::Success, i18n.t("sync_restored"), None),
                Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
            }
            restoring.done();
            state::refresh();
            load();
        });
    };
    let tabs = move || {
        vec![
            TabDef {
                id: "overview",
                icon: "sync",
                label: Signal::derive(move || i18n.t("sync_tab_overview")),
            },
            TabDef {
                id: "history",
                icon: "history",
                label: Signal::derive(move || i18n.t("sync_tab_history")),
            },
        ]
    };
    let overview = move || {
        view.get().map(|v| {
            let builds = v.group.builds.iter().map(|k| name_of(k)).collect::<Vec<_>>().join(", ");
            let paths = v.group.paths.iter().map(|p| shown_path(p)).collect::<Vec<_>>().join(", ");
            let start = name_of(&v.group.start);
            let when = v.last_ms.map(state::date_time).unwrap_or_else(|| "—".into());
            let for_edit = v.clone();
            view! {
                <dl class="sync-facts">
                    <dt>{move || i18n.t("sync_mode")}</dt><dd>{i18n.t(mode_key(v.group.mode))}</dd>
                    <dt>{move || i18n.t("sync_builds")}</dt><dd>{builds}</dd>
                    <dt>{move || i18n.t("sync_start")}</dt><dd>{start}</dd>
                    <dt>{move || i18n.t("sync_paths")}</dt><dd class="sync-facts__mono">{paths}</dd>
                    <dt>{move || i18n.t("sync_state")}</dt><dd>{i18n.t(state_key(v.state))}" · "{when}</dd>
                </dl>
                <div class="wrap">
                    <Button icon="edit" on_click=move |_| on_edit.run(for_edit.clone())>{move || i18n.t("sync_edit")}</Button>
                </div>
            }
        })
    };
    let entries = move || {
        match history.get() {
        None => view! { <div class="hint">{move || i18n.t("loading")}</div> }.into_any(),
        Some(list) if list.is_empty() => view! {
            <EmptyState icon="history" title=Signal::derive(move || i18n.t("sync_history_empty")) />
        }
        .into_any(),
        Some(list) => list
            .into_iter()
            .map(|entry| {
                let id = entry.id.clone();
                let from = entry.from.as_deref().map(|f| i18n.tp("sync_history_from", &[("build", name_of(f))]));
                let tone = match entry.reason {
                    Reason::Conflict => TagTone::Warn,
                    Reason::Removed => TagTone::Danger,
                    _ => TagTone::Neutral,
                };
                view! {
                    <div class="list-row sync-history">
                        <div class="sync-history__text">
                            <div class="sync-history__path">{entry.path.clone()}</div>
                            <div class="sync-history__sub">
                                {name_of(&entry.build)}" · "{state::date_time(entry.at_ms)}" · "{units::size(entry.size, &i18n.lang())}
                                {from.map(|f| view! { " · "{f} })}
                            </div>
                        </div>
                        <Tag tone=tone>{i18n.t(reason_key(entry.reason))}</Tag>
                        <IconAction
                            icon="restore"
                            title=Signal::derive(move || i18n.t("sync_restore"))
                            loading=restoring.running()
                            on_click=Callback::new(move |()| restore(id.clone()))
                        />
                    </div>
                }
            })
            .collect_view()
            .into_any(),
    }
    };
    let title =
        Signal::derive(move || view.with(|v| v.as_ref().map(|v| v.group.name.clone()).unwrap_or_default()));
    view! {
        <Dialog open=open title=title icon="sync" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <Tabs tabs=tabs() active=tab />
            <div class="sync-group">
                {move || if tab.get() == "overview" { overview().into_any() } else { entries().into_any() }}
            </div>
        </Dialog>
    }
}
