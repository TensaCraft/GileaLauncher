use leptos::html::Div;
use leptos::prelude::*;

pub fn value_to_percent(value: f64, min: f64, max: f64) -> f64 {
    if max <= min { 0.0 } else { ((value - min) / (max - min) * 100.0).clamp(0.0, 100.0) }
}

pub fn percent_to_value(percent: f64, min: f64, max: f64, step: f64) -> f64 {
    if max <= min {
        return min;
    }
    let raw = min + percent.clamp(0.0, 100.0) / 100.0 * (max - min);
    let step = if step > 0.0 { step } else { 1.0 };
    (((raw - min) / step).round() * step + min).clamp(min, max)
}

/// Range input with a diamond thumb, ticks and a recommended marker. `on_change` fires on commit.
#[component]
pub fn Slider(
    value: RwSignal<f64>,
    min: f64,
    max: f64,
    #[prop(optional)] step: Option<f64>,
    #[prop(optional)] recommended: Option<f64>,
    #[prop(optional)] ticks: Vec<String>,
    #[prop(optional, into)] on_change: Option<Callback<f64>>,
    #[prop(optional, into)] label: MaybeProp<String>,
) -> impl IntoView {
    let step = step.unwrap_or(1.0);
    let track = NodeRef::<Div>::new();
    let dragging = RwSignal::new(false);

    let set_from_x = move |client_x: i32| {
        if let Some(el) = track.get_untracked() {
            let rect = el.get_bounding_client_rect();
            if rect.width() > 0.0 {
                let pct = (client_x as f64 - rect.left()) / rect.width() * 100.0;
                value.set(percent_to_value(pct, min, max, step));
            }
        }
    };
    let commit = move || {
        if let Some(cb) = on_change {
            cb.run(value.get_untracked());
        }
    };
    let pct = move || value_to_percent(value.get(), min, max);

    view! {
        <div
            class="slider"
            tabindex="0"
            role="slider"
            aria-label=move || label.get()
            aria-valuemin=min
            aria-valuemax=max
            aria-valuenow=move || value.get()
            on:pointerdown=move |ev| {
                dragging.set(true);
                if let Some(target) = ev.current_target() {
                    use wasm_bindgen::JsCast;
                    if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                        let _ = el.set_pointer_capture(ev.pointer_id());
                    }
                }
                set_from_x(ev.client_x());
            }
            on:pointermove=move |ev| {
                if dragging.get_untracked() {
                    set_from_x(ev.client_x());
                }
            }
            on:pointerup=move |_| {
                if dragging.get_untracked() {
                    dragging.set(false);
                    commit();
                }
            }
            on:keydown=move |ev| {
                let current = value.get_untracked();
                let next = match ev.key().as_str() {
                    "ArrowRight" | "ArrowUp" => current + step,
                    "ArrowLeft" | "ArrowDown" => current - step,
                    "Home" => min,
                    "End" => max,
                    _ => return,
                };
                ev.prevent_default();
                value.set(next.clamp(min, max));
                commit();
            }
        >
            <div class="slider__track" node_ref=track>
                <div class="slider__fill" style=move || format!("width:{}%", pct())></div>
                {recommended.map(|r| view! {
                    <div class="slider__rec" style=format!("left:{}%", value_to_percent(r, min, max))></div>
                })}
                <div class="slider__thumb" style=move || format!("left:{}%", pct())></div>
            </div>
            <div class="slider__ticks">
                {ticks.into_iter().map(|t| view! { <span>{t}</span> }).collect_view()}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_conversions_clamp_and_snap_to_step() {
        assert_eq!(value_to_percent(8.0, 1.0, 29.0), 25.0);
        assert_eq!(value_to_percent(50.0, 1.0, 29.0), 100.0);
        assert_eq!(value_to_percent(5.0, 5.0, 5.0), 0.0);
        assert_eq!(percent_to_value(25.0, 1.0, 29.0, 1.0), 8.0);
        assert_eq!(percent_to_value(26.9, 1.0, 29.0, 1.0), 9.0);
        assert_eq!(percent_to_value(-10.0, 1.0, 29.0, 1.0), 1.0);
        assert_eq!(percent_to_value(150.0, 1.0, 29.0, 1.0), 29.0);
    }
}
