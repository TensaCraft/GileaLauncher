//! The chosen font and accent on the window: every variable on `<html>`, and a copy in the
//! window's storage that `appearance-boot.js` puts back before the first frame of the next start.

use launcher_shared::appearance::{UiFont, appearance_vars};
use leptos::prelude::*;

use crate::store::AppStore;

/// Where `appearance-boot.js` finds the variables.
const STORAGE_KEY: &str = "appearance";

/// The variables to set: none before the backend's settings arrive, so the window's placeholder
/// settings never replace the look `appearance-boot.js` put back.
pub fn wanted(loaded: bool, font: UiFont, accent: &str) -> Option<Vec<(&'static str, String)>> {
    loaded.then(|| appearance_vars(font, accent))
}

/// Keeps `<html>` in the look the settings choose, from now on.
pub fn follow_settings(store: AppStore) {
    // Only a new font or accent wakes it, not every other change of the settings.
    let look = Memo::new(move |_| {
        let loaded = store.settings_loaded.get();
        store.settings.with(|s| wanted(loaded, s.ui_font, &s.accent))
    });
    Effect::new(move |_| {
        let Some(vars) = look.get() else { return };
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

    use launcher_shared::appearance::DEFAULT_ACCENT;

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

    /// Before the backend's settings arrive, the look `appearance-boot.js` put back stays: the
    /// window's placeholder settings would show the default for a moment and store it.
    #[test]
    fn nothing_is_applied_before_the_settings_arrive() {
        assert_eq!(wanted(false, UiFont::Minecraft, "#9b6cf6"), None);
        assert_eq!(
            wanted(true, UiFont::Minecraft, "#9b6cf6"),
            Some(appearance_vars(UiFont::Minecraft, "#9b6cf6"))
        );
    }

    /// The boot file is asked for from the root, whatever page the window reloads on.
    #[test]
    fn the_boot_file_is_loaded_from_the_root() {
        let html = include_str!("../index.html");
        assert!(html.contains(r#"<script src="/appearance-boot.js"></script>"#), "{html}");
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
