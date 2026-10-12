//! The groups every sync view shows (the page, a build's tab, the marks on cards), loaded once
//! and refreshed after a change and after every pass (`GROUPS_EVENT`), the background ones too.

use std::cell::Cell;

use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::ipc;

use super::api;
use crate::dto::{GROUPS_EVENT, GroupView};

thread_local! {
    static GROUPS: ArcRwSignal<Option<Vec<GroupView>>> = ArcRwSignal::new(None);
    static LOADING: Cell<bool> = const { Cell::new(false) };
    /// Asked again while loading: the list may already be older than what was asked for.
    static AGAIN: Cell<bool> = const { Cell::new(false) };
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// The groups (`None` until loaded).
pub fn groups() -> ArcRwSignal<Option<Vec<GroupView>>> {
    if !LISTENING.with(|l| l.replace(true)) {
        ipc::listen::<serde_json::Value>(GROUPS_EVENT, |_| refresh());
    }
    GROUPS.with(Clone::clone)
}

/// Loads the groups again.
pub fn refresh() {
    if LOADING.with(|l| l.replace(true)) {
        AGAIN.with(|a| a.set(true));
        return;
    }
    spawn_local(async {
        loop {
            AGAIN.with(|a| a.set(false));
            if let Ok(list) = api::groups().await {
                groups().set(Some(list));
            }
            if !AGAIN.with(Cell::get) {
                break;
            }
        }
        LOADING.with(|l| l.set(false));
    });
}

/// Loads the groups unless they are.
pub fn ensure() {
    if groups().with_untracked(Option::is_none) {
        refresh();
    }
}

/// Local "dd.mm hh:mm" of `ms` since the epoch.
pub fn date_time(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}
