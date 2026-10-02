//! The window is an app, not a browser: no browser menu on right click (text fields keep theirs
//! for copy and paste), and in release builds no browser keys (reload, find, print, save, view
//! source, history, the inspector).

/// Whether a right click on an element keeps the system menu: text fields do, and in a dev build a
/// Shift+right click anywhere does (the inspector).
pub fn keeps_native_menu(tag: &str, input_type: Option<&str>, editable: bool, dev_shift: bool) -> bool {
    let text_input = tag.eq_ignore_ascii_case("input")
        && matches!(
            input_type.unwrap_or("text").to_ascii_lowercase().as_str(),
            "text" | "search" | "url" | "email" | "password" | "number" | "tel"
        );
    dev_shift || editable || text_input || tag.eq_ignore_ascii_case("textarea")
}

/// Installs both for the app's lifetime.
pub fn install() {
    use leptos::prelude::window_event_listener;
    use wasm_bindgen::JsCast;
    let menu = window_event_listener(leptos::ev::contextmenu, |ev| {
        let keep = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()).is_some_and(|el| {
            let dev_shift = cfg!(debug_assertions) && ev.shift_key();
            keeps_native_menu(
                &el.tag_name(),
                el.get_attribute("type").as_deref(),
                el.is_content_editable(),
                dev_shift,
            )
        });
        if !keep {
            ev.prevent_default();
        }
    });
    let mac = web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .is_some_and(|agent| agent.contains("Macintosh"));
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        let key = ev.key();
        let press = Press {
            key: &key,
            ctrl: ev.ctrl_key(),
            meta: ev.meta_key(),
            shift: ev.shift_key(),
            alt: ev.alt_key(),
        };
        if blocked_key(&press, mac, cfg!(not(debug_assertions))) {
            ev.prevent_default();
            ev.stop_propagation();
        }
    });
    // Both live as long as the app.
    std::mem::forget((menu, keys));
}

/// A key press as the browser reports it.
pub struct Press<'a> {
    pub key: &'a str,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Whether a key press is a browser's own and is dropped (`release`: a build for players; `mac`:
/// macOS, where Command makes the shortcuts).
pub fn blocked_key(press: &Press<'_>, mac: bool, release: bool) -> bool {
    // macOS text fields keep Ctrl+letter (Cocoa's editing keys) and Option+arrows (words).
    let (key, shift) = (press.key, press.shift);
    let command = if mac { press.meta } else { press.ctrl || press.meta };
    if !release {
        return false;
    }
    match key.to_ascii_lowercase().as_str() {
        "f5" | "f7" | "f12" | "browserback" | "browserforward" | "browserrefresh" => true,
        "arrowleft" | "arrowright" => press.alt && !mac,
        // Inspector, console, element picker, hard reload.
        "i" | "j" | "c" | "r" if command && shift => true,
        // Reload, view source, print, find, find next, downloads, history, new window, open, save.
        "r" | "u" | "p" | "f" | "g" | "j" | "h" | "n" | "o" | "s" => command && !shift,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_field_keeps_its_menu() {
        assert!(keeps_native_menu("TEXTAREA", None, false, false));
        assert!(keeps_native_menu("INPUT", Some("text"), false, false));
        assert!(keeps_native_menu("input", Some("search"), false, false));
        assert!(keeps_native_menu("DIV", None, true, false), "an editable element");
        assert!(!keeps_native_menu("DIV", None, false, false), "the page itself shows no browser menu");
        assert!(!keeps_native_menu("INPUT", Some("checkbox"), false, false));
        assert!(!keeps_native_menu("IMG", None, false, false));
        assert!(
            keeps_native_menu("IMG", None, false, true),
            "a dev build's Shift+right click reaches the inspector"
        );
    }

    fn press(key: &str, ctrl: bool, shift: bool, alt: bool) -> Press<'_> {
        Press { key, ctrl, meta: false, shift, alt }
    }

    #[test]
    fn macos_keeps_its_text_editing_keys() {
        let cocoa = |key| Press { key, ctrl: true, meta: false, shift: false, alt: false };
        for key in ["f", "b", "p", "n", "h", "o", "a", "e", "k"] {
            assert!(!blocked_key(&cocoa(key), true, true), "Ctrl+{key} edits text on macOS");
        }
        for key in ["ArrowLeft", "ArrowRight"] {
            let word = Press { key, ctrl: false, meta: false, shift: false, alt: true };
            assert!(!blocked_key(&word, true, true), "Option+{key} moves by words on macOS");
        }
        for key in ["r", "p", "u", "s"] {
            let command = Press { key, ctrl: false, meta: true, shift: false, alt: false };
            assert!(blocked_key(&command, true, true), "Command+{key} is the browser's on macOS");
        }
        assert!(blocked_key(&press("F5", false, false, false), true, true));
        assert!(blocked_key(&press("r", true, false, false), false, true), "Ctrl+R elsewhere");
    }

    #[test]
    fn browser_keys_are_blocked_in_releases() {
        for (key, ctrl, shift, alt) in [
            ("F5", false, false, false),
            ("F12", false, false, false),
            ("F7", false, false, false),
            ("r", true, false, false),
            ("R", true, true, false),
            ("I", true, true, false),
            ("J", true, true, false),
            ("C", true, true, false),
            ("u", true, false, false),
            ("p", true, false, false),
            ("f", true, false, false),
            ("s", true, false, false),
            ("ArrowLeft", false, false, true),
            ("ArrowRight", false, false, true),
        ] {
            let p = press(key, ctrl, shift, alt);
            assert!(blocked_key(&p, false, true), "{key} ctrl={ctrl} shift={shift} alt={alt}");
            assert!(!blocked_key(&p, false, false), "a dev build keeps {key}");
        }
        for (key, ctrl, shift) in [
            ("c", true, false),
            ("v", true, false),
            ("a", true, false),
            ("x", true, false),
            ("z", true, false),
            ("Enter", false, false),
            ("Escape", false, false),
            ("ArrowLeft", false, false),
        ] {
            assert!(!blocked_key(&press(key, ctrl, shift, false), false, true), "{key} stays for editing");
        }
    }
}
