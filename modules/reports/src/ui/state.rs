//! What the module's parts share: whether reports have somewhere to go (asked once), and the
//! build whose report window is asked for.

use std::cell::Cell;

use launcher_shared::BuildDto;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::api;

thread_local! {
    static ENABLED: ArcRwSignal<Option<bool>> = ArcRwSignal::new(None);
    static ASKED_STATUS: Cell<bool> = const { Cell::new(false) };
    static ASKED: ArcRwSignal<Option<BuildDto>> = ArcRwSignal::new(None);
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

/// Reports are known to have somewhere to go.
pub fn is_enabled() -> bool {
    ENABLED.with(|enabled| enabled.get_untracked() == Some(true))
}

/// The build whose report window is asked for.
pub fn asked() -> ArcRwSignal<Option<BuildDto>> {
    ASKED.with(Clone::clone)
}

/// Opens the report window for `build` (a build menu's entry).
pub fn open_build_report(build: BuildDto) {
    ASKED.with(|asked| asked.set(Some(build)));
}
