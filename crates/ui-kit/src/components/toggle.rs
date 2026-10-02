use leptos::prelude::*;

use crate::sound;

#[component]
pub fn Switch(
    checked: RwSignal<bool>,
    #[prop(optional, into)] on_change: Option<Callback<bool>>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    #[prop(optional, into)] label: MaybeProp<String>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="switch"
            class="switch"
            class:is-on=move || checked.get()
            aria-checked=move || checked.get().to_string()
            aria-label=move || label.get()
            disabled=move || disabled.get().unwrap_or(false)
            on:click=move |_| {
                let next = !checked.get_untracked();
                sound::play_click();
                checked.set(next);
                if let Some(cb) = on_change {
                    cb.run(next);
                }
            }
        ></button>
    }
}

#[component]
pub fn Checkbox(
    checked: RwSignal<bool>,
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] on_change: Option<Callback<bool>>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="checkbox"
            class="choice"
            aria-checked=move || checked.get().to_string()
            on:click=move |_| {
                let next = !checked.get_untracked();
                sound::play_click();
                checked.set(next);
                if let Some(cb) = on_change {
                    cb.run(next);
                }
            }
        >
            <span class="check" class:is-on=move || checked.get()></span>
            <span>{move || label.get()}</span>
        </button>
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegOption {
    pub value: String,
    pub label: String,
    pub icon: Option<String>,
    /// A count beside the label (how many there are).
    pub badge: Option<String>,
}

impl SegOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), icon: None, badge: None }
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Shows `count` beside the label (nothing when it is unknown or zero).
    pub fn with_count(mut self, count: Option<usize>) -> Self {
        self.badge = count.filter(|n| *n > 0).map(|n| n.to_string());
        self
    }
}

#[cfg(test)]
mod seg_tests {
    use super::SegOption;

    #[test]
    fn a_segment_shows_how_many_there_are() {
        assert_eq!(
            SegOption::new("installed", "Installed").with_count(Some(181)).badge.as_deref(),
            Some("181")
        );
        assert_eq!(
            SegOption::new("installed", "Installed").with_count(Some(0)).badge,
            None,
            "none: no badge"
        );
        assert_eq!(SegOption::new("installed", "Installed").with_count(None).badge, None, "not known yet");
    }
}

#[component]
pub fn Segmented(
    #[prop(into)] options: Signal<Vec<SegOption>>,
    value: RwSignal<String>,
    #[prop(optional, into)] on_change: Option<Callback<String>>,
) -> impl IntoView {
    view! {
        <div class="seg" role="radiogroup">
            {move || {
                options
                    .get()
                    .into_iter()
                    .map(|opt| {
                        let v = opt.value.clone();
                        let is_on = {
                            let v = v.clone();
                            move || value.get() == v
                        };
                        view! {
                            <button
                                type="button"
                                role="radio"
                                class="seg__btn"
                                class:is-on=is_on
                                on:click=move |_| {
                                    if value.get_untracked() != v {
                                        sound::play_click();
                                        value.set(v.clone());
                                        if let Some(cb) = on_change {
                                            cb.run(v.clone());
                                        }
                                    }
                                }
                            >
                                {opt.icon.map(|i| view! { <super::icon::Icon name=i outlined=true /> })}
                                {opt.label}
                                {opt.badge.map(|b| view! { <span class="seg__badge">{b}</span> })}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
