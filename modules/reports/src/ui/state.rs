//! What the module's parts share: whether reports have somewhere to go (asked once).

use std::cell::Cell;

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::api;

thread_local! {
    static ENABLED: ArcRwSignal<Option<bool>> = ArcRwSignal::new(None);
    static ASKED_STATUS: Cell<bool> = const { Cell::new(false) };
}

/// Whether reports can be sent (`None` until the backend answers).
pub fn enabled() -> ArcRwSignal<Option<bool>> {
    let enabled = ENABLED.with(Clone::clone);
    if !ASKED_STATUS.with(|asked| asked.replace(true)) {
        let answer = enabled.clone();
        spawn_local(async move { answer.set(Some(api::status().await.is_ok_and(|s| s.enabled))) });
    }
    enabled
}
