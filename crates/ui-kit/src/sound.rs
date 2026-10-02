//! UI click sounds: enabled flag, variant, 40 ms debounce. The launcher plays them
//! (`play_click`); the web view plays no audio itself.

use std::cell::Cell;

use launcher_shared::ClickSound;

pub const CLICK_DEBOUNCE_MS: f64 = 40.0;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static VARIANT: Cell<ClickSound> = const { Cell::new(ClickSound::GateLatchClick) };
    static LAST_MS: Cell<f64> = const { Cell::new(-1.0e12) };
}

pub fn should_play(now_ms: f64, last_ms: f64) -> bool {
    now_ms - last_ms >= CLICK_DEBOUNCE_MS
}

pub fn configure(enabled: bool, variant: ClickSound) {
    ENABLED.with(|e| e.set(enabled));
    VARIANT.with(|v| v.set(variant));
}

pub fn play_click() {
    if !ENABLED.with(Cell::get) {
        return;
    }
    let now = js_sys::Date::now();
    let allowed = LAST_MS.with(|last| {
        let ok = should_play(now, last.get());
        if ok {
            last.set(now);
        }
        ok
    });
    if !allowed {
        return;
    }
    let sound = VARIANT.with(Cell::get).as_config_str();
    leptos::task::spawn_local(async move {
        let _ = crate::ipc::invoke::<_, ()>("play_click", &serde_json::json!({ "sound": sound })).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_is_debounced_by_40_ms() {
        assert!(should_play(1000.0, 0.0));
        assert!(!should_play(1039.0, 1000.0));
        assert!(should_play(1040.0, 1000.0));
    }
}
