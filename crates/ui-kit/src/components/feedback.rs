use leptos::prelude::*;

use super::icon::Icon;

pub fn progress_percent(done: Option<f64>, total: Option<f64>) -> Option<f64> {
    match (done, total) {
        (Some(d), Some(t)) if t > 0.0 => Some((d / t * 100.0).clamp(0.0, 100.0)),
        _ => None,
    }
}

#[component]
pub fn EmptyState(
    #[prop(into)] icon: String,
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] desc: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="empty">
            <div class="empty__icon"><Icon name=icon outlined=true /></div>
            <div class="empty__title">{move || title.get()}</div>
            {move || desc.get().map(|d| view! { <div class="empty__desc">{d}</div> })}
            {children.map(|c| view! { <div style="margin-top:8px">{c()}</div> })}
        </div>
    }
}

#[component]
pub fn Skeleton(#[prop(optional)] width: Option<u32>) -> impl IntoView {
    let width = width.unwrap_or(100).min(100);
    view! { <div class="skeleton" style=format!("width:{width}%")></div> }
}

#[component]
pub fn ProgressBar(#[prop(into)] value: Signal<Option<f64>>) -> impl IntoView {
    view! {
        <div class="progress" class:is-indeterminate=move || value.get().is_none()>
            <span class="progress__bar" style=move || value.get().map(|v| format!("width:{v:.1}%")).unwrap_or_default()></span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_percent_handles_missing_and_bad_totals() {
        assert_eq!(progress_percent(Some(50.0), Some(200.0)), Some(25.0));
        assert_eq!(progress_percent(Some(500.0), Some(200.0)), Some(100.0));
        assert_eq!(progress_percent(None, Some(10.0)), None);
        assert_eq!(progress_percent(Some(5.0), None), None);
        assert_eq!(progress_percent(Some(5.0), Some(0.0)), None);
    }
}
