//! The look's pickers: the UI font as tiles, each set in its own font, and the accent as round
//! swatches with the player's own colour last.

use launcher_shared::appearance::{ACCENT_PRESETS, UiFont, font_stacks, normalize_accent};
use leptos::prelude::*;

use super::{Icon, TextInput};
use crate::i18n::use_i18n;

/// The preset `accent` is (its id), or `None` for the player's own colour.
pub fn chosen_preset(accent: &str) -> Option<&'static str> {
    ACCENT_PRESETS.iter().find(|p| p.hex == accent).map(|p| p.id)
}

/// What the hex field gives: a whole colour, normalised; nothing while it is still being typed.
pub fn accent_input(text: &str) -> Option<String> {
    normalize_accent(text)
}

#[component]
pub fn FontTiles(#[prop(into)] value: Signal<UiFont>, on_pick: Callback<UiFont>) -> impl IntoView {
    let i18n = use_i18n();
    let tiles = UiFont::ALL
        .into_iter()
        .map(|font| {
            let style = format!("font-family: {}", font_stacks(font).0);
            view! {
                <button
                    type="button"
                    class="font-tile"
                    class:is-active=move || value.get() == font
                    style=style
                    on:click=move |_| on_pick.run(font)
                >
                    <span class="font-tile__sample">"Аа Бб"</span>
                    <span class="font-tile__name">{move || i18n.t(font.label_key())}</span>
                </button>
            }
        })
        .collect_view();
    view! { <div class="font-tiles">{tiles}</div> }
}

#[component]
pub fn AccentSwatches(#[prop(into)] value: Signal<String>, on_pick: Callback<String>) -> impl IntoView {
    let i18n = use_i18n();
    let typed = RwSignal::new(value.get_untracked());
    Effect::new(move |_| typed.set(value.get()));
    // A whole colour typed into the field applies at once; a half-typed one waits.
    Effect::new(move |_| {
        if let Some(colour) = accent_input(&typed.get())
            && colour != value.get_untracked()
        {
            on_pick.run(colour);
        }
    });
    let swatches = ACCENT_PRESETS
        .into_iter()
        .map(|preset| {
            view! {
                <button
                    type="button"
                    class="swatch"
                    class:is-active=move || value.get() == preset.hex
                    style=format!("--swatch: {}", preset.hex)
                    data-tip=move || i18n.t(preset.label_key)
                    aria-label=move || i18n.t(preset.label_key)
                    on:click=move |_| on_pick.run(preset.hex.to_string())
                ></button>
            }
        })
        .collect_view();
    view! {
        <div class="swatches">
            {swatches}
            <label
                class="swatch swatch--custom"
                class:is-active=move || chosen_preset(&value.get()).is_none()
                style=move || format!("--swatch: {}", value.get())
                data-tip=move || i18n.t("appearance_accent_custom")
            >
                <input
                    type="color"
                    prop:value=move || value.get()
                    on:change=move |ev| {
                        if let Some(colour) = accent_input(&event_target_value(&ev)) {
                            on_pick.run(colour);
                        }
                    }
                />
                <Icon name="colorize" />
            </label>
            <div class="swatches__hex">
                <TextInput
                    value=typed
                    placeholder="#rrggbb"
                    icon="tag"
                    invalid=Signal::derive(move || accent_input(&typed.get()).is_none())
                />
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_preset_is_found_by_its_colour() {
        assert_eq!(chosen_preset("#17c3a2"), Some("emerald"));
        assert_eq!(chosen_preset("#9b6cf6"), Some("amethyst"));
        assert_eq!(chosen_preset("#123456"), None, "the player's own");
    }

    #[test]
    fn accent_input_applies_only_complete_colours() {
        assert_eq!(accent_input("#7a"), None);
        assert_eq!(accent_input("#7a5cff").as_deref(), Some("#7a5cff"));
        assert_eq!(accent_input("#7A5").as_deref(), Some("#77aa55"));
    }
}
