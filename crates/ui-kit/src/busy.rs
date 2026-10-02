//! One action at a time: a second click, a repeated Enter or a click on a row that is already at
//! work does nothing until the first ends.

use leptos::prelude::*;

/// An action that runs once at a time.
#[derive(Clone, Copy)]
pub struct InFlight(RwSignal<bool>);

impl Default for InFlight {
    fn default() -> Self {
        InFlight::new()
    }
}

impl InFlight {
    pub fn new() -> InFlight {
        InFlight(RwSignal::new(false))
    }

    /// Starts the action: `false` when it already runs (or the page is gone), and then nothing
    /// must happen.
    pub fn start(&self) -> bool {
        match self.0.try_get_untracked() {
            Some(false) => self.0.try_set(true).is_none(),
            _ => false,
        }
    }

    /// The action ended (the page may be gone by then).
    pub fn done(&self) {
        let _ = self.0.try_set(false);
    }

    /// Whether it runs, for `loading=` and `disabled=`.
    pub fn running(&self) -> Signal<bool> {
        let running = self.0;
        Signal::derive(move || running.try_get().unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_submitted_twice_runs_once() {
        let owner = Owner::new();
        owner.with(|| {
            let copying = InFlight::new();
            assert!(copying.start(), "the first Enter copies");
            assert!(!copying.start(), "the second finds it at work");
            assert!(copying.running().get_untracked());
        });
    }

    #[test]
    fn an_action_runs_again_once_it_is_done() {
        let owner = Owner::new();
        owner.with(|| {
            let adding = InFlight::new();
            assert!(adding.start());
            adding.done();
            assert!(!adding.running().get_untracked());
            assert!(adding.start(), "a new click after the answer");
        });
    }

    #[test]
    fn a_gone_page_starts_nothing() {
        let owner = Owner::new();
        let action = owner.with(InFlight::new);
        drop(owner);
        assert!(!action.start());
        action.done();
    }
}
