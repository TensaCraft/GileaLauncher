//! The chosen font and accent on the window: every variable on `<html>`, and a copy in the
//! window's storage that `appearance-boot.js` puts back before the first frame of the next start.

use launcher_shared::appearance::appearance_vars;
use leptos::prelude::*;

use crate::store::AppStore;

/// Where `appearance-boot.js` finds the variables.
const STORAGE_KEY: &str = "appearance";

/// Keeps `<html>` in the look the settings choose, from now on.
pub fn follow_settings(store: AppStore) {
    Effect::new(move |_| {
        let (font, accent) = store.settings.with(|s| (s.ui_font, s.accent.clone()));
        let vars = appearance_vars(font, &accent);
        let Some(window) = web_sys::window() else { return };
        if let Some(root) = window.document().and_then(|d| d.document_element())
            && let Ok(root) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(root)
        {
            let style = root.style();
            for (name, value) in &vars {
                let _ = style.set_property(name, value);
            }
        }
        // A storage that is missing or refuses only costs the next start one frame of the default.
        if let (Ok(Some(storage)), Ok(json)) = (window.local_storage(), serde_json::to_string(&vars)) {
            let _ = storage.set_item(STORAGE_KEY, &json);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use launcher_shared::appearance::{DEFAULT_ACCENT, UiFont};

    use super::*;

    /// The variables `tokens.css` defines in the families the look owns.
    fn owned_in_tokens() -> BTreeSet<String> {
        let tokens = include_str!("../../ui-kit/styles/tokens.css");
        let owned =
            ["--primary", "--accent-", "--tint-", "--s-", "--text", "--line", "--ring", "--bg-", "--f-"];
        tokens
            .lines()
            .filter_map(|line| line.trim().split_once(':').map(|(name, _)| name.trim().to_string()))
            .filter(|name| owned.iter().any(|prefix| name.starts_with(prefix)))
            .collect()
    }

    /// Every variable the look sets has a default in `tokens.css`, and every one of its families
    /// there is set: none keeps the default teal under another accent.
    #[test]
    fn appearance_vars_cover_every_token_the_css_reads() {
        let set: BTreeSet<String> = appearance_vars(UiFont::Standard, DEFAULT_ACCENT)
            .into_iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(set, owned_in_tokens());
    }
}
