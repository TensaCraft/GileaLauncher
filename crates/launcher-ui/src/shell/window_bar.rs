//! The launcher's own title bar (the window has no system one): the window's buttons at the
//! header's right end, and the surfaces that move the window (the header, the sidebar's free
//! space).

use launcher_shared::WindowAction;
use leptos::ev::MouseEvent;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Variant, ipc};
use wasm_bindgen::JsCast;

#[derive(Serialize)]
struct ActionArgs {
    action: WindowAction,
}

/// Asks the window for `action`; whether it fills the screen then.
async fn control(action: WindowAction) -> Option<bool> {
    ipc::invoke::<_, bool>("window_control", &ActionArgs { action }).await.ok()
}

/// The surfaces that move the window carry this attribute (the header, the sidebar).
const SURFACE: &str = "data-window-drag";

/// What a press on a surface may land on without moving the window: its controls and popups.
const CONTROLS: &str = "button, a, input, select, textarea, label, [role=button], .menu, .ops__panel";

/// A left press on a surface, not on one of its controls.
fn on_surface(ev: &MouseEvent) -> bool {
    let found = |el: &web_sys::Element, selector: &str| el.closest(selector).ok().flatten().is_some();
    ev.button() == 0
        && ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| found(&el, &format!("[{SURFACE}]")) && !found(&el, CONTROLS))
}

/// Moves the window from a surface (the shell listens for the whole app): a press arms it and the
/// first move with the button held hands the window to the system (a press alone stays a click,
/// so a double click still comes); a double click maximizes or restores.
#[derive(Clone, Copy)]
pub struct WindowDrag {
    armed: StoredValue<bool>,
    filled: RwSignal<bool>,
}

impl WindowDrag {
    pub fn press(&self, ev: MouseEvent) {
        self.armed.set_value(on_surface(&ev));
    }

    pub fn moved(&self, ev: MouseEvent) {
        if self.armed.get_value() && ev.buttons() & 1 == 1 {
            self.armed.set_value(false);
            spawn_local(async {
                control(WindowAction::Drag).await;
            });
        }
    }

    pub fn release(&self) {
        self.armed.set_value(false);
    }

    pub fn double(&self, ev: MouseEvent) {
        if on_surface(&ev) {
            self.act(WindowAction::Maximize);
        }
    }

    fn act(&self, action: WindowAction) {
        let filled = self.filled;
        spawn_local(async move {
            if let Some(now) = control(action).await {
                filled.set(now);
            }
        });
    }
}

/// The shell's one `WindowDrag`, which also knows whether the window fills the screen: looked up
/// at start and after every resize (the window manager's own maximize too).
pub fn provide_window_drag() -> WindowDrag {
    let drag = WindowDrag { armed: StoredValue::new(false), filled: RwSignal::new(false) };
    drag.act(WindowAction::Look);
    let resize = window_event_listener(leptos::ev::resize, move |_| drag.act(WindowAction::Look));
    on_cleanup(move || resize.remove());
    provide_context(drag);
    drag
}

pub fn use_window_drag() -> WindowDrag {
    expect_context::<WindowDrag>()
}

/// To the tray, minimize, maximize or restore, close: above every layer, a dialog's too, as the
/// system's title bar would be (in the page's body, where dialogs go, not in the app's own layer).
#[component]
pub fn WindowControls() -> impl IntoView {
    let i18n = use_i18n();
    let drag = use_window_drag();
    let filled = drag.filled;
    let button = move |icon: &'static str, tip: &'static str, action: WindowAction, class: &'static str| {
        view! {
            <Button
                variant=Variant::Ghost
                icon=icon
                class=class
                title=Signal::derive(move || i18n.t(tip))
                on_click=move |_| drag.act(action)
            />
        }
    };
    view! {
        <Portal>
        <div class="window-controls">
            {button("move_to_inbox", "window_tray", WindowAction::Tray, "")}
            {button("remove", "window_minimize", WindowAction::Minimize, "")}
            {move || if filled.get() {
                button("filter_none", "window_restore", WindowAction::Maximize, "")
            } else {
                button("crop_square", "window_maximize", WindowAction::Maximize, "")
            }}
            {button("close", "window_close", WindowAction::Close, "is-close")}
        </div>
        </Portal>
    }
}
