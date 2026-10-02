//! The memory limit, the same in the launcher's Java settings and a build's runtime settings:
//! "Auto" sits in the column of the other rows' switches; switched off, the slider opens below.

use launcher_shared::MemoryInfo;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{SettingRow, Slider, Switch};

use super::ram_slider;

#[component]
pub fn RamSetting(
    #[prop(into)] title: Signal<String>,
    /// "Automatically — N GB".
    #[prop(into)]
    auto_label: Signal<String>,
    auto: RwSignal<bool>,
    ram: RwSignal<f64>,
    #[prop(into)] memory: Signal<Option<MemoryInfo>>,
    #[prop(optional, into)] on_auto: Option<Callback<bool>>,
    #[prop(optional, into)] on_ram: Option<Callback<f64>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let hint = Signal::derive(move || {
        memory
            .get()
            .map(|m| {
                i18n.tp(
                    "ram_slider_limit_hint",
                    &[("total", m.total_gb.to_string()), ("max", m.max_heap_gb.to_string())],
                )
            })
            .unwrap_or_default()
    });
    let switched = Callback::new(move |on: bool| {
        if let Some(cb) = on_auto {
            cb.run(on);
        }
    });
    let moved = Callback::new(move |gb: f64| {
        if let Some(cb) = on_ram {
            cb.run(gb);
        }
    });
    view! {
        <SettingRow title=title desc=hint>
            <span class="row__value">{move || auto_label.get()}</span>
            <Switch checked=auto on_change=switched label=auto_label />
        </SettingRow>
        {move || {
            (!auto.get()).then(|| memory.get()).flatten().map(|info| {
                let (min, max, recommended) = ram_slider(&info);
                let value_label = move || {
                    i18n.tp(
                        "ram_slider_value",
                        &[("value", (ram.get().round() as u64).to_string()), ("max", (max as u64).to_string())],
                    )
                };
                view! {
                    <div class="row-below">
                        <Slider value=ram min=min max=max step=1.0 recommended=recommended on_change=moved label=Signal::derive(value_label) />
                        <div class="hint">{value_label}</div>
                    </div>
                }
            })
        }}
    }
}
