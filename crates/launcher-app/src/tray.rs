//! The launcher in the system tray: hidden there once the game it started runs, when the user chose
//! that (`GameStartAction::Tray`); back on the icon's click, its menu, a second start or (macOS) the
//! Dock.
//! The icon is there only while the window is hidden. Where no tray can be made or shown (Linux
//! without libayatana-appindicator, or without a StatusNotifier host as on stock GNOME) the window
//! is minimized instead.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};

use launcher_shared::GameStartAction;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::commands::AppState;

const TRAY_ID: &str = "main";
const MAIN_WINDOW: &str = "main";

/// The window is hidden in the tray. Kept apart from the tray icon itself, which only the main
/// thread may touch (a game's watcher asks too).
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// The user chose the tray over quitting.
pub fn wanted(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|state| state.core.settings.snapshot().on_game_start == GameStartAction::Tray)
}

/// The window is hidden in the tray now.
pub fn hidden() -> bool {
    HIDDEN.load(Ordering::SeqCst)
}

/// Hides the window into a tray icon.
pub fn hide(app: &AppHandle) {
    let handle = app.clone();
    // Asked here, not on the main thread (a D-Bus call on Linux).
    let shown = launcher_core::platform::tray::tray_shown();
    let done = app.run_on_main_thread(move || {
        let window = handle.get_webview_window(MAIN_WINDOW);
        if !shown {
            tracing::warn!("The desktop shows no tray icons; the launcher is minimized instead");
            if let Some(window) = &window {
                let _ = window.minimize();
            }
            return;
        }
        if handle.tray_by_id(TRAY_ID).is_none() {
            // A tray library that is missing panics inside it: the window is minimized instead.
            let built = std::panic::catch_unwind(AssertUnwindSafe(|| build(&handle)));
            if !matches!(built, Ok(Ok(()))) {
                tracing::warn!("No tray icon here; the launcher is minimized instead");
                if let Some(window) = &window {
                    let _ = window.minimize();
                }
                return;
            }
        }
        if let Some(window) = &window {
            let _ = window.hide();
        }
        HIDDEN.store(true, Ordering::SeqCst);
        tracing::info!("The launcher is hidden in the tray");
    });
    if let Err(e) = done {
        tracing::warn!("The launcher cannot hide in the tray: {e}");
    }
}

/// Brings the window back (shown, not minimized, focused) and takes the tray icon away, on the main
/// thread (a game's watcher or a second start may ask from another).
pub fn restore(app: &AppHandle) {
    let handle = app.clone();
    let done = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        let _ = handle.remove_tray_by_id(TRAY_ID);
        HIDDEN.store(false, Ordering::SeqCst);
    });
    if let Err(e) = done {
        tracing::warn!("The launcher cannot come back from the tray: {e}");
    }
}

/// The launcher's language now.
fn lang(app: &AppHandle) -> String {
    app.try_state::<AppState>().map(|state| state.core.settings.lang()).unwrap_or_default()
}

/// The window's title in the launcher's language (the taskbar shows it).
pub fn retitle(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.set_title(&texts(&lang(app)).title);
    }
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let AppTexts { title, open, quit } = texts(&lang(app));
    let open = MenuItem::with_id(app, "open", open, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(title)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => restore(app),
            // Held while work is under way: the window asks first.
            "quit" if !crate::hold_quit(app) => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// What the system shows of the launcher (the window's title, the tray's tooltip and menu): it says
/// what the app is, never a brand.
pub struct AppTexts {
    pub title: String,
    pub open: String,
    pub quit: String,
}

/// The launcher's texts in `lang` (English for any other language).
fn words(lang: &str) -> serde_json::Value {
    let raw = if lang == "uk_UA" {
        include_str!("../../../assets/langs/uk_UA.json")
    } else {
        include_str!("../../../assets/langs/en_US.json")
    };
    serde_json::from_str(raw).unwrap_or_default()
}

/// The title and the text of the message a start that failed shows, with why (`detail`).
pub fn startup_failure(lang: &str, detail: &str) -> (String, String) {
    let words = words(lang);
    let get = |key: &str, fallback: &str| words[key].as_str().unwrap_or(fallback).to_string();
    let text = get("startup_failed", "Why: {error}").replace("{error}", detail);
    (get("startup_failed_title", "The launcher could not start"), text)
}

/// The system's words for the launcher in its language.
pub fn texts(lang: &str) -> AppTexts {
    let words = words(lang);
    let get = |key: &str, fallback: &str| words[key].as_str().unwrap_or(fallback).to_string();
    AppTexts {
        title: get("app_title", "Minecraft launcher"),
        open: get("tray_open", "Open the launcher"),
        quit: get("tray_quit", "Quit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_start_says_why_in_the_system_s_language() {
        let (title, text) = startup_failure("uk_UA", "disk is full");
        assert_eq!(title, "Лаунчер не запустився");
        assert!(text.contains("disk is full"), "{text}");
        let (title, text) = startup_failure("de_DE", "disk is full");
        assert_eq!(title, "The launcher could not start");
        assert!(text.contains("disk is full"), "{text}");
    }

    #[test]
    fn the_tray_speaks_the_launcher_s_language_and_names_no_brand() {
        let uk = texts("uk_UA");
        assert_eq!(
            (uk.title.as_str(), uk.open.as_str(), uk.quit.as_str()),
            ("Лаунчер Minecraft", "Відкрити лаунчер", "Вийти")
        );
        let en = texts("en_US");
        assert_eq!(
            (en.title.as_str(), en.open.as_str(), en.quit.as_str()),
            ("Minecraft launcher", "Open the launcher", "Quit")
        );
    }
}
