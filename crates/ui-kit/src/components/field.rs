use leptos::prelude::*;

use super::button::{Button, Size, Variant};
use super::icon::Icon;

#[component]
pub fn Field(
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] optional_note: MaybeProp<String>,
    #[prop(optional, into)] hint: MaybeProp<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="field">
            <label class="label">
                <span>{move || label.get()}</span>
                {move || optional_note.get().map(|n| view! { <span class="label__opt">{n}</span> })}
            </label>
            {children()}
            {move || match error.get() {
                Some(e) => view! { <div class="error"><Icon name="error_outline" />{e}</div> }.into_any(),
                None => hint.get().map(|h| view! { <div class="hint">{h}</div> }).into_any(),
            }}
        </div>
    }
}

/// A key press that submits a field: Enter, and not its auto-repeat while held.
pub fn enter_submits(key: &str, repeat: bool) -> bool {
    key == "Enter" && !repeat
}

#[component]
pub fn TextInput(
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
    #[prop(optional, into)] icon: Option<String>,
    #[prop(optional, into)] invalid: MaybeProp<bool>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    #[prop(optional, into)] on_enter: Option<Callback<()>>,
    #[prop(optional)] autofocus: bool,
    /// The same heights as buttons (`Size::Md` by default).
    #[prop(optional)]
    size: Size,
) -> impl IntoView {
    view! {
        <div
            class=format!("control {}", size.control_class())
            class:is-error=move || invalid.get().unwrap_or(false)
            class:is-disabled=move || disabled.get().unwrap_or(false)
        >
            {icon.map(|name| view! { <Icon name=name outlined=true /> })}
            <input
                class="input"
                type="text"
                spellcheck="false"
                autofocus=autofocus
                placeholder=move || placeholder.get()
                disabled=move || disabled.get().unwrap_or(false)
                bind:value=value
                on:keydown=move |ev| {
                    if enter_submits(&ev.key(), ev.repeat())
                        && let Some(cb) = on_enter
                    {
                        cb.run(());
                    }
                }
            />
        </div>
    }
}

/// Several lines of text (a report's description), in the inputs' frame.
#[component]
pub fn TextArea(
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
    #[prop(optional, into)] invalid: MaybeProp<bool>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    /// Visible lines (4 unless said).
    #[prop(optional)]
    rows: Option<u32>,
) -> impl IntoView {
    view! {
        <div
            class="control control--area"
            class:is-error=move || invalid.get().unwrap_or(false)
            class:is-disabled=move || disabled.get().unwrap_or(false)
        >
            <textarea
                class="input input--area"
                spellcheck="false"
                rows=rows.unwrap_or(4)
                placeholder=move || placeholder.get()
                disabled=move || disabled.get().unwrap_or(false)
                bind:value=value
            ></textarea>
        </div>
    }
}

#[component]
pub fn PathField(
    value: RwSignal<String>,
    #[prop(into)] browse_label: Signal<String>,
    on_browse: Callback<()>,
    #[prop(optional, into)] invalid: MaybeProp<bool>,
    #[prop(optional)] size: Size,
) -> impl IntoView {
    view! {
        <div class="group">
            <TextInput value=value icon="folder" invalid=invalid size=size />
            <Button variant=Variant::Secondary size=size icon="folder_open" outlined=true on_click=on_browse>
                {move || browse_label.get()}
            </Button>
        </div>
    }
}

#[cfg(test)]
mod enter_tests {
    use super::*;

    #[test]
    fn a_repeated_enter_is_ignored() {
        assert!(enter_submits("Enter", false));
        assert!(!enter_submits("Enter", true), "a held key repeats: once is enough");
        assert!(!enter_submits("a", false));
    }
}
