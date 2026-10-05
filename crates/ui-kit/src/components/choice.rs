//! `ChoiceCards`: one choice among a few, each option a card with a picture of what it looks like
//! (its icon while the picture loads, or when it cannot), its name and a line about it.

use leptos::prelude::*;

use super::icon::Icon;
use crate::sound;

#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
    /// Drawn while the picture loads, and in its place when there is none or it cannot load.
    pub icon: String,
    pub desc: Option<String>,
    pub image: Option<String>,
}

impl ChoiceOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>, icon: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), icon: icon.into(), desc: None, image: None }
    }

    /// A line under the name (none when empty).
    pub fn with_desc(mut self, desc: impl Into<String>) -> Self {
        self.desc = Some(desc.into()).filter(|d| !d.trim().is_empty());
        self
    }

    /// A picture of the option; without one the card is shorter, its icon in the picture's place.
    pub fn with_image(mut self, image: Option<String>) -> Self {
        self.image = image;
        self
    }
}

#[component]
pub fn ChoiceCards(
    #[prop(into)] options: Signal<Vec<ChoiceOption>>,
    value: RwSignal<String>,
    #[prop(optional, into)] on_change: Option<Callback<String>>,
    /// What is chosen, for screen readers.
    #[prop(optional, into)]
    label: MaybeProp<String>,
    /// Cards in a row; as many as there are options by default.
    #[prop(optional)]
    columns: Option<usize>,
) -> impl IntoView {
    let cols = move || columns.unwrap_or_else(|| options.with(Vec::len)).max(1);
    view! {
        <div class="choices" role="radiogroup" aria-label=move || label.get() style=move || format!("--choice-cols: {}", cols())>
            {move || options.get().into_iter().map(|opt| choice_card(opt, value, on_change)).collect_view()}
        </div>
    }
}

fn choice_card(
    opt: ChoiceOption,
    value: RwSignal<String>,
    on_change: Option<Callback<String>>,
) -> impl IntoView {
    let chosen = {
        let v = opt.value.clone();
        Signal::derive(move || value.with(|current| *current == v))
    };
    let loaded = RwSignal::new(false);
    let plain = opt.image.is_none();
    let v = opt.value.clone();
    view! {
        <button
            type="button"
            role="radio"
            class="choice-card"
            class:is-on=chosen
            class:is-plain=plain
            aria-checked=move || chosen.get().to_string()
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
            <span class="choice-card__pic">
                <Icon name=opt.icon outlined=true />
                {opt.image.map(|src| view! {
                    <img src=src alt="" draggable="false" class:is-loaded=move || loaded.get() on:load=move |_| loaded.set(true) />
                })}
            </span>
            <span class="choice-card__text">
                <span class="choice-card__title"><span class="choice-card__dot"></span>{opt.label}</span>
                {opt.desc.map(|d| view! { <span class="choice-card__desc">{d}</span> })}
            </span>
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_keeps_only_a_picture_and_a_line_it_has() {
        let plain = ChoiceOption::new("tray", "To the tray", "south_east").with_desc("").with_image(None);
        assert_eq!((plain.desc, plain.image), (None, None));
        let pictured = ChoiceOption::new("bar", "A bar", "view_agenda")
            .with_desc("Under the name")
            .with_image(Some("https://example.net/uk_UA/cards-bar.jpg".into()));
        assert_eq!(pictured.desc.as_deref(), Some("Under the name"));
        assert_eq!(pictured.image.as_deref(), Some("https://example.net/uk_UA/cards-bar.jpg"));
        assert_eq!(pictured.icon, "view_agenda");
    }

    /// The declarations of `selector`'s rule.
    fn rule(css: &str, selector: &str) -> String {
        let start = css.find(&format!("{selector} {{")).unwrap_or_else(|| panic!("no rule {selector}"));
        let body = &css[start..];
        body[body.find('{').unwrap() + 1..body.find('}').unwrap()].to_string()
    }

    #[test]
    fn a_picture_shows_only_once_it_has_loaded_and_takes_no_clicks() {
        let css = include_str!("../../styles/components.css");
        let picture = rule(css, ".choice-card__pic img");
        assert!(picture.contains("opacity: 0") && picture.contains("pointer-events: none"), "{picture}");
        assert!(rule(css, ".choice-card__pic img.is-loaded").contains("opacity: 1"));
    }
}
