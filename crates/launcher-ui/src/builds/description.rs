//! A build's description: its field, shared by the build settings page and the wizard, with the
//! limit the backend keeps to.

use launcher_shared::DESCRIPTION_MAX;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Field, TextArea};

/// A description over the limit (a translation key).
pub const DESCRIPTION_TOO_LONG: &str = "build_description_too_long";

/// What is wrong with `text` as a description: it is over the limit.
pub fn description_problem(text: &str) -> Option<&'static str> {
    (text.chars().count() > DESCRIPTION_MAX).then_some(DESCRIPTION_TOO_LONG)
}

/// «Опис» with how much of the limit it takes, or what is wrong with it.
#[component]
pub fn DescriptionField(value: RwSignal<String>) -> impl IntoView {
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let error = Signal::derive(move || value.with(|text| description_problem(text)).map(|key| i18n.t(key)));
    let hint = Signal::derive(move || {
        let count = value.with(|text| text.chars().count()).to_string();
        i18n.tp("build_description_count", &[("count", count), ("max", DESCRIPTION_MAX.to_string())])
    });

    view! {
        <Field label=t("build_description_label") hint=hint error=error>
            <TextArea
                value=value
                rows=2
                placeholder=t("build_description_hint")
                invalid=Signal::derive(move || error.get().is_some())
            />
        </Field>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_is_measured_in_characters_not_bytes() {
        assert_eq!(description_problem(&"а".repeat(300)), None, "300 Cyrillic letters are 600 bytes");
        assert_eq!(description_problem(&"а".repeat(301)), Some("build_description_too_long"));
        assert_eq!(description_problem(""), None);
    }
}
