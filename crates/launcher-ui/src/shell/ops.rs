use launcher_shared::OperationDto;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{ProgressBar, progress_percent};

use crate::store::use_store;

const RING: f64 = 69.12; // 2 * PI * 11

#[component]
pub fn OpsIndicator() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let visible = move || -> Vec<OperationDto> {
        store.ops.get().operations.into_iter().filter(|o| o.visible).collect()
    };
    let root_pct = move || visible().first().and_then(|o| progress_percent(o.progress, o.total));

    view! {
        <Show when=move || !visible().is_empty()>
            <div class="ops">
                <button type="button" class="ops__btn" data-tip=move || i18n.t("ops_panel_title") data-tip-side="top" aria-label=move || i18n.t("ops_panel_title") on:click=move |_| open.update(|o| *o = !*o)>
                    <svg class="ops__ring" class:is-spinning=move || root_pct().is_none() viewBox="0 0 26 26">
                        <circle class="ops__track" cx="13" cy="13" r="11"></circle>
                        <circle
                            class="ops__value"
                            cx="13"
                            cy="13"
                            r="11"
                            stroke-dasharray=RING
                            stroke-dashoffset=move || RING * (1.0 - root_pct().unwrap_or(25.0) / 100.0)
                        ></circle>
                    </svg>
                    <span class="ops__pct">{move || root_pct().map(|p| format!("{p:.0}%"))}</span>
                </button>
                <Show when=move || open.get()>
                    <div class="menu-catcher" on:click=move |_| open.set(false)></div>
                    <div class="ops__panel">
                        <div class="ops__head">{move || i18n.t("ops_panel_title")}</div>
                        {move || visible().into_iter().map(|op| {
                            let pct = progress_percent(op.progress, op.total);
                            view! {
                                <div class="ops__item" class:is-child=op.parent_id.is_some()>
                                    <div class="ops__title">
                                        <span>{i18n.text(&op.title)}</span>
                                        <span>{pct.map(|p| format!("{p:.0}%"))}</span>
                                    </div>
                                    {op.status.as_ref().map(|s| view! { <div class="ops__status">{i18n.text(s)}</div> })}
                                    <ProgressBar value=Signal::derive(move || pct) />
                                </div>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </div>
        </Show>
    }
}
