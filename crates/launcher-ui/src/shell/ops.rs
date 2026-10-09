use leptos::prelude::*;
use ui_kit::ProgressBar;
use ui_kit::i18n::use_i18n;

use crate::pages::settings::activity::{listed_ops, live_op, overall_percent};
use crate::store::use_store;

const RING: f64 = 69.12; // 2 * PI * 11

#[component]
pub fn OpsIndicator() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let visible = listed_ops(store);
    // All the work under way: the ring turns only when it moves.
    let root_pct = Memo::new(move |_| visible.with(|v| overall_percent(v)));

    view! {
        <Show when=move || !visible.with(Vec::is_empty)>
            <div class="ops">
                <button type="button" class="ops__btn" data-tip=move || i18n.t("ops_panel_title") data-tip-side="top" aria-label=move || i18n.t("ops_panel_title") on:click=move |_| open.update(|o| *o = !*o)>
                    <svg class="ops__ring" class:is-spinning=move || root_pct.get().is_none() viewBox="0 0 26 26">
                        <circle class="ops__track" cx="13" cy="13" r="11"></circle>
                        <circle
                            class="ops__value"
                            cx="13"
                            cy="13"
                            r="11"
                            stroke-dasharray=RING
                            stroke-dashoffset=move || RING * (1.0 - root_pct.get().unwrap_or(25.0) / 100.0)
                        ></circle>
                    </svg>
                    <span class="ops__pct">{move || root_pct.get().map(|p| format!("{p:.0}%"))}</span>
                </button>
                <Show when=move || open.get()>
                    <div class="menu-catcher" on:click=move |_| open.set(false)></div>
                    <div class="ops__panel">
                        <div class="ops__head">{move || i18n.t("ops_panel_title")}</div>
                        // A row stays while its operation moves, so its bar glides on.
                        <For
                            each=move || visible.get()
                            key=|op| op.id
                            children=move |op| {
                                let live = live_op(i18n, visible, op.id);
                                view! {
                                    <div class="ops__item" class:is-child=op.parent_id.is_some()>
                                        <div class="ops__title">
                                            <span>{move || live.title.get()}</span>
                                            <span>{move || live.percent.get().map(|p| format!("{p:.0}%"))}</span>
                                        </div>
                                        <Show when=move || live.status.with(Option::is_some)>
                                            <div class="ops__status">{move || live.status.get()}</div>
                                        </Show>
                                        <ProgressBar value=live.percent />
                                    </div>
                                }
                            }
                        />
                    </div>
                </Show>
            </div>
        </Show>
    }
}
