//! A group opened: what it syncs and how.

use launcher_shared::BuildDto;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Variant};

use super::model::{build_name, mode_key, shown_path, state_key};
use super::state;
use crate::dto::GroupView;

#[component]
pub fn GroupDialog(
    open: RwSignal<bool>,
    view: RwSignal<Option<GroupView>>,
    #[prop(into)] builds: Signal<Vec<BuildDto>>,
    on_edit: Callback<GroupView>,
) -> impl IntoView {
    let i18n = use_i18n();
    let name_of = move |key: &str| {
        builds.with(|all| {
            build_name(all, key).unwrap_or_else(|| i18n.tp("sync_build_gone", &[("build", key.to_string())]))
        })
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
    let title =
        Signal::derive(move || view.with(|v| v.as_ref().map(|v| v.group.name.clone()).unwrap_or_default()));
    view! {
        <Dialog open=open title=title icon="sync" class="dialog--sync">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <div class="sync-group">{overview}</div>
        </Dialog>
    }
}
