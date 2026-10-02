//! The crash the UI last heard of, shared by the dialog and the Diagnostics tab.

use std::cell::Cell;

use leptos::prelude::*;
use ui_kit::ipc;

use crate::dto::{DIAGNOSIS_EVENT, DiagnosisEvent};

thread_local! {
    static CRASH: ArcRwSignal<Option<DiagnosisEvent>> = ArcRwSignal::new(None);
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// A crashed game's diagnosis, while its dialog is open.
pub fn crash() -> ArcRwSignal<Option<DiagnosisEvent>> {
    CRASH.with(Clone::clone)
}

/// Hears of crashes from now on: once, however often the module's window is mounted.
pub fn listen() {
    if LISTENING.with(|l| l.replace(true)) {
        return;
    }
    let crash = crash();
    ipc::listen::<DiagnosisEvent>(DIAGNOSIS_EVENT, move |event| crash.set(Some(event)));
}
