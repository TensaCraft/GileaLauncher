//! Forwards core events to the webview.

use std::time::Duration;

use crate::commands::AppState;
use launcher_core::auth::UrlOpener;
use launcher_core::feedback::EventSink;
use launcher_shared::{
    ActivityEntry, Alert, AuthState, GameEvent, GameState, OpsSnapshot, ProfilesSnapshot, Toast,
    UpdateStatus, names,
};
use tauri::{AppHandle, Emitter, Manager};

/// How often a pending close checks whether operations have finished.
const CLOSE_POLL: Duration = Duration::from_millis(250);

/// "Close the launcher when the game starts": the game outlived its early-exit window.
pub fn should_close_launcher(event: &GameEvent) -> bool {
    matches!(event.state, GameState::Running { close_launcher: true })
}

/// How the launcher gets out of the user's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    Quit,
    /// Hidden into an icon in the system tray (`GameStartAction::Tray`).
    ToTray,
}

/// What a game event asks of the launcher: to go once the game runs, if the user wants that —
/// into the tray when they chose it (`to_tray`).
pub fn leave_on(event: &GameEvent, to_tray: bool) -> Option<Leave> {
    should_close_launcher(event).then_some(if to_tray { Leave::ToTray } else { Leave::Quit })
}

/// A game that crashed brings a hidden launcher back: its report is shown there.
pub fn comes_back_on(event: &GameEvent) -> bool {
    matches!(event.state, GameState::Crashed { .. })
}

/// Blocks until `ready()`, checking every `poll`.
pub fn wait_until(ready: impl Fn() -> bool, poll: Duration) {
    while !ready() {
        std::thread::sleep(poll);
    }
}

/// Closes the launcher once no operation runs, so an install or a copy is never cut short.
fn close_when_idle(app: AppHandle) {
    std::thread::spawn(move || {
        let busy = || app.try_state::<AppState>().is_some_and(|state| state.core.feedback.is_busy());
        wait_until(|| !busy(), CLOSE_POLL);
        app.exit(0);
    });
}

pub struct TauriSink {
    app: AppHandle,
}

impl TauriSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn send<T: serde::Serialize + Clone>(&self, event: &str, payload: &T) {
        if let Err(e) = self.app.emit(event, payload.clone()) {
            tracing::debug!("Unable to emit {event}: {e}");
        }
    }
}

impl EventSink for TauriSink {
    fn ops(&self, snapshot: &OpsSnapshot) {
        self.send(names::OPS, snapshot);
    }
    fn activity(&self, entry: &ActivityEntry) {
        self.send(names::ACTIVITY, entry);
    }
    fn toast(&self, toast: &Toast) {
        self.send(names::TOAST, toast);
    }
    fn alert(&self, alert: &Alert) {
        self.send(names::ALERT, alert);
    }
    fn update(&self, status: &UpdateStatus) {
        self.send(names::UPDATE, status);
    }
    fn profiles(&self, snapshot: &ProfilesSnapshot) {
        self.send(names::PROFILES, snapshot);
    }
    fn auth(&self, state: &AuthState) {
        self.send(names::AUTH, state);
    }
    fn module_event(&self, event: &str, payload: &serde_json::Value) {
        self.send(event, payload);
    }
    fn game(&self, event: &GameEvent) {
        self.send(names::GAME, event);
        match leave_on(event, crate::tray::wanted(&self.app)) {
            Some(Leave::Quit) => {
                tracing::info!("Closing the launcher: {} is running", event.build_name);
                close_when_idle(self.app.clone());
            }
            Some(Leave::ToTray) => crate::tray::hide(&self.app),
            None => {}
        }
        if comes_back_on(event) && crate::tray::hidden() {
            crate::tray::restore(&self.app);
        }
    }
}

/// Sign-in pages go to the user's default browser; only https addresses are opened.
pub fn may_open(url: &str) -> bool {
    url.starts_with("https://") && !url.chars().any(char::is_whitespace)
}

pub struct TauriOpener {
    app: AppHandle,
}

impl TauriOpener {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl UrlOpener for TauriOpener {
    fn open(&self, url: &str) -> bool {
        if !may_open(url) {
            tracing::warn!("Refusing to open a non-https address in the browser");
            return false;
        }
        match crate::commands::open_link(&self.app, url) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Unable to open the browser: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_pages_are_opened() {
        assert!(may_open("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x"));
        assert!(may_open("https://www.microsoft.com/link?otc=ABCD-1234"));
        assert!(!may_open("http://example.com/"));
        assert!(!may_open("file:///C:/Windows/System32/calc.exe"));
        assert!(!may_open("javascript:alert(1)"));
        assert!(!may_open(" https://spaced.example"));
    }

    #[test]
    fn the_launcher_closes_only_for_a_running_game_that_asks() {
        let event = |state| GameEvent { build_key: "aero".into(), build_name: "Aero".into(), state };
        assert!(should_close_launcher(&event(GameState::Running { close_launcher: true })));
        assert!(!should_close_launcher(&event(GameState::Running { close_launcher: false })));
        assert!(!should_close_launcher(&event(GameState::Started { pid: 7 })));
        assert!(!should_close_launcher(&event(GameState::Exited { code: Some(0) })));
    }

    #[test]
    fn the_launcher_leaves_to_the_tray_when_the_user_chose_it() {
        let event = |state| GameEvent { build_key: "aero".into(), build_name: "Aero".into(), state };
        let running = event(GameState::Running { close_launcher: true });
        assert_eq!(leave_on(&running, false), Some(Leave::Quit));
        assert_eq!(leave_on(&running, true), Some(Leave::ToTray));
        assert_eq!(
            leave_on(&event(GameState::Running { close_launcher: false }), true),
            None,
            "asked not to go"
        );
        assert_eq!(leave_on(&event(GameState::Started { pid: 7 }), true), None);
    }

    #[test]
    fn a_crash_brings_the_launcher_back_with_its_report() {
        let event = |state| GameEvent { build_key: "aero".into(), build_name: "Aero".into(), state };
        assert!(comes_back_on(&event(GameState::Crashed { code: Some(1), early: false, log: None })));
        assert!(
            !comes_back_on(&event(GameState::Exited { code: Some(0) })),
            "a game that ends well leaves it hidden"
        );
    }

    #[test]
    fn closing_waits_until_no_operation_runs() {
        let checks = std::cell::Cell::new(0);
        wait_until(
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 3
            },
            std::time::Duration::from_millis(1),
        );
        assert_eq!(checks.get(), 3);
    }
}
