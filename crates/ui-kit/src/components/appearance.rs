//! The look's pickers: the UI font as tiles, each set in its own font, and the accent as round
//! swatches with the player's own colour last.

use launcher_shared::appearance::{
    ACCENT_PRESETS, DEFAULT_ACCENT, UiFont, font_stacks, light_ink_reads, normalize_accent,
};
use leptos::prelude::*;

use super::{Icon, IconAction, TextInput};
use crate::i18n::use_i18n;

/// The preset `accent` is (its id), or `None` for the player's own colour.
pub fn chosen_preset(accent: &str) -> Option<&'static str> {
    ACCENT_PRESETS.iter().find(|p| p.hex == accent).map(|p| p.id)
}

/// What the hex field gives on Enter: a whole colour (`#rgb` too), normalised.
pub fn accent_input(text: &str) -> Option<String> {
    normalize_accent(text)
}

/// What the hex field gives while typed into: only a whole `#rrggbb` (a `#rgb` on the way to six
/// digits would apply and rewrite the field under the fingers).
pub fn live_accent(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() == 7 { normalize_accent(text) } else { None }
}

/// The field shows the setting again: not while it is being typed into, and not when it already
/// says the same colour.
pub fn field_follows(typed: &str, value: &str, focused: bool) -> bool {
    !focused && accent_input(typed).as_deref() != Some(value)
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
    // Only a new colour wakes the field, not every other change of the settings.
    let value = Memo::new(move |_| value.get());
    let typed = RwSignal::new(value.get_untracked());
    let focused = RwSignal::new(false);
    Effect::new(move |_| {
        let (shown, editing) = (value.get(), focused.get());
        if field_follows(&typed.get_untracked(), &shown, editing) {
            typed.set(shown);
        }
    });
    // A whole six-digit colour typed into the field applies at once; a shorter one on Enter.
    Effect::new(move |_| {
        if let Some(colour) = live_accent(&typed.get())
            && colour != value.get_untracked()
        {
            on_pick.run(colour);
        }
    });
    let enter = Callback::new(move |()| {
        if let Some(colour) = accent_input(&typed.get_untracked())
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
                // Its mark in the ink that reads on the colour picked.
                style=move || {
                    let colour = value.get();
                    let ink = if light_ink_reads(&colour) { "--light" } else { "--on-accent" };
                    format!("--swatch: {colour}; --swatch-ink: var({ink})")
                }
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
            <div class="swatches__hex" on:focusin=move |_| focused.set(true) on:focusout=move |_| focused.set(false)>
                <TextInput
                    value=typed
                    placeholder="#rrggbb"
                    icon="tag"
                    on_enter=enter
                    invalid=Signal::derive(move || !focused.get() && accent_input(&typed.get()).is_none())
                />
            </div>
            // The way back to the launcher's own colour, while another one is on.
            <Show when=move || value.get() != DEFAULT_ACCENT>
                <IconAction
                    icon="restart_alt"
                    title=Signal::derive(move || i18n.t("appearance_accent_reset"))
                    on_click=Callback::new(move |()| on_pick.run(DEFAULT_ACCENT.to_string()))
                />
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_preset_is_found_by_its_colour() {
        assert_eq!(chosen_preset("#17c3a2"), Some("default"));
        assert_eq!(chosen_preset("#9b6cf6"), Some("amethyst"));
        assert_eq!(chosen_preset("#123456"), None, "the player's own");
    }

    #[test]
    fn typing_applies_only_a_whole_six_digit_colour() {
        for half in ["#", "#7", "#7a", "#7a5", "#7a5c", "#7a5cf"] {
            assert_eq!(live_accent(half), None, "{half}");
        }
        assert_eq!(live_accent("#7A5CFF").as_deref(), Some("#7a5cff"));
        assert_eq!(accent_input("#7a5").as_deref(), Some("#77aa55"), "Enter takes the short form");
    }

    #[test]
    fn the_field_follows_the_setting_only_when_left_alone() {
        assert!(!field_follows("#7a5c", "#17c3a2", true), "being typed into");
        assert!(field_follows("#7a5c", "#17c3a2", false), "left half-typed: shows the setting again");
        assert!(!field_follows("#17C3A2", "#17c3a2", false), "already shows it");
        assert!(field_follows("#17c3a2", "#9b6cf6", false), "a swatch was picked");
    }

    #[test]
    fn accent_input_applies_only_complete_colours() {
        assert_eq!(accent_input("#7a"), None);
        assert_eq!(accent_input("#7a5cff").as_deref(), Some("#7a5cff"));
        assert_eq!(accent_input("#7A5").as_deref(), Some("#77aa55"));
    }
}
