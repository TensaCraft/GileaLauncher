//! What is drawn above everything — tooltips and dropdown menus. They are placed in window
//! coordinates next to the element they belong to, so no scroll box or neighbouring layer can
//! hide or clip them.

use std::time::Duration;

use leptos::ev;
use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// A box in window coordinates (`getBoundingClientRect`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn of(element: &Element) -> Rect {
        let r = element.get_bounding_client_rect();
        Rect { x: r.x(), y: r.y(), width: r.width(), height: r.height() }
    }
}

/// Space kept from the window's edges.
const EDGE: f64 = 8.0;
/// Between a control and its menu.
const GAP: f64 = 6.0;
/// The tallest a menu gets.
const TALL: f64 = 280.0;
/// Room below that is enough not to look above.
const ENOUGH: f64 = 200.0;

/// Where a dropdown menu goes: under its control, or above it when there is little room below and
/// more above; never past the window's edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuPlace {
    pub left: f64,
    pub width: f64,
    pub max_height: f64,
    /// Opens upwards; `edge` is then the distance from the window's bottom.
    pub up: bool,
    pub edge: f64,
}

impl MenuPlace {
    pub fn style(&self) -> String {
        let side = if self.up { "bottom" } else { "top" };
        format!(
            "left:{:.0}px;width:{:.0}px;max-height:{:.0}px;{side}:{:.0}px",
            self.left, self.width, self.max_height, self.edge
        )
    }
}

pub fn menu_place(control: Rect, viewport: (f64, f64)) -> MenuPlace {
    let below = viewport.1 - control.y - control.height - GAP - EDGE;
    let above = control.y - GAP - EDGE;
    let up = below < ENOUGH && above > below;
    let room = if up { above } else { below };
    let width = control.width.min(viewport.0 - 2.0 * EDGE).max(0.0);
    let left = control.x.min(viewport.0 - EDGE - width).max(EDGE);
    let edge = if up { viewport.1 - control.y + GAP } else { control.y + control.height + GAP };
    MenuPlace { left, width, max_height: room.clamp(0.0, TALL), up, edge }
}

/// The window's size.
pub fn viewport() -> (f64, f64) {
    let size = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
        v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
    };
    web_sys::window().map_or((0.0, 0.0), |w| (size(w.inner_width()), size(w.inner_height())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipSide {
    Right,
    Top,
}

/// Between an element and its tip, right of it and above it.
const TIP_GAP_SIDE: f64 = 10.0;
const TIP_GAP_TOP: f64 = 8.0;

/// Where a tip of `size` goes (its top-left corner): right of its element and centred on it, or
/// above it and centred; on the other side when there is no room, and never past the window's
/// edges (`EDGE`).
pub fn tip_place(host: Rect, side: TipSide, size: (f64, f64), viewport: (f64, f64)) -> (f64, f64) {
    let (width, height) = size;
    let fit = |start: f64, length: f64, room: f64| start.min(room - EDGE - length).max(EDGE);
    match side {
        TipSide::Right => {
            let right = host.x + host.width + TIP_GAP_SIDE;
            let left = if right + width > viewport.0 - EDGE { host.x - TIP_GAP_SIDE - width } else { right };
            (fit(left, width, viewport.0), fit(host.y + host.height / 2.0 - height / 2.0, height, viewport.1))
        }
        TipSide::Top => {
            let above = host.y - TIP_GAP_TOP - height;
            let top = if above < EDGE { host.y + host.height + TIP_GAP_TOP } else { above };
            (fit(host.x + host.width / 2.0 - width / 2.0, width, viewport.0), fit(top, height, viewport.1))
        }
    }
}

/// The pause before a tooltip shows.
const TIP_DELAY: Duration = Duration::from_millis(300);

/// Empties `shown`; its readers wake only when something was in it (every wheel turn and press
/// asks the tooltip to hide).
fn empty<T: Send + Sync + 'static>(shown: RwSignal<Option<T>>) {
    shown.try_maybe_update(|s| (s.take().is_some(), ()));
}

#[derive(Debug, Clone, PartialEq)]
struct Tip {
    text: String,
    side: TipSide,
    host: Rect,
    /// Where it goes, once its size is measured: drawn out of sight until then.
    at: Option<(f64, f64)>,
}

/// The app's one tooltip, drawn above everything. An element names its tip with `data-tip` (and
/// `data-tip-side="top"`; right by default); it shows after a pause on hover and hides when the
/// pointer leaves, a button is pressed or the wheel turns.
#[component]
pub fn TipLayer() -> impl IntoView {
    let tip = RwSignal::new(None::<Tip>);
    let host = StoredValue::new_local(None::<Element>);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let hide = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
        timer.try_set_value(None);
        host.try_set_value(None);
        empty(tip);
    };
    let over = window_event_listener(ev::mouseover, move |event| {
        let found = event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|e| e.closest("[data-tip]").ok().flatten());
        if host.try_with_value(|h| *h == found).unwrap_or(true) {
            return;
        }
        hide();
        let Some(element) = found else { return };
        let text = element.get_attribute("data-tip").unwrap_or_default();
        if text.trim().is_empty() {
            return;
        }
        let side = if element.get_attribute("data-tip-side").as_deref() == Some("top") {
            TipSide::Top
        } else {
            TipSide::Right
        };
        host.set_value(Some(element.clone()));
        let show = move || {
            tip.try_set(Some(Tip { text, side, host: Rect::of(&element), at: None }));
        };
        timer.set_value(set_timeout_with_handle(show, TIP_DELAY).ok());
    });
    let out = window_event_listener(ev::mouseout, move |event| {
        if event.related_target().is_none() {
            hide();
        }
    });
    let down = window_event_listener(ev::mousedown, move |_| hide());
    let wheel = window_event_listener(ev::wheel, move |_| hide());
    on_cleanup(move || {
        over.remove();
        out.remove();
        down.remove();
        wheel.remove();
    });
    // Measured where it cannot be seen, then placed inside the window (`tip_place`).
    let bubble = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let Some(el) = bubble.get() else { return };
        let Some(t) = tip.get().filter(|t| t.at.is_none()) else { return };
        let size = Rect::of(&el);
        let at = tip_place(t.host, t.side, (size.width, size.height), viewport());
        tip.try_set(Some(Tip { at: Some(at), ..t }));
    });
    view! {
        <Portal>
            {move || {
                tip.with(|t| t.as_ref().map(|t| (t.text.clone(), t.at))).map(|(text, at)| {
                    let style = match at {
                        Some((x, y)) => format!("left:{x:.0}px;top:{y:.0}px"),
                        None => "left:0;top:0;visibility:hidden".to_string(),
                    };
                    view! {
                        <div class="tip" node_ref=bubble style=style role="tooltip">
                            {text}
                        </div>
                    }
                })
            }}
        </Portal>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (f64, f64) = (1280.0, 720.0);

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn a_menu_opens_below_while_there_is_room() {
        let place = menu_place(rect(100.0, 100.0, 240.0, 36.0), VIEW);
        assert_eq!(place, MenuPlace { left: 100.0, width: 240.0, max_height: 280.0, up: false, edge: 142.0 });
        assert_eq!(place.style(), "left:100px;width:240px;max-height:280px;top:142px");
    }

    #[test]
    fn a_menu_near_the_bottom_opens_upwards() {
        let place = menu_place(rect(100.0, 640.0, 240.0, 36.0), VIEW);
        assert!(place.up);
        assert_eq!((place.edge, place.max_height), (720.0 - 640.0 + 6.0, 280.0));
        assert!(place.style().ends_with("bottom:86px"), "{}", place.style());
    }

    #[test]
    fn a_menu_takes_the_bigger_side_and_stays_in_the_window() {
        // 166 px below, 136 above: below is less than wanted but still the bigger side.
        let low = menu_place(rect(0.0, 150.0, 200.0, 20.0), (400.0, 350.0));
        assert!(!low.up);
        assert_eq!(low.max_height, 350.0 - 170.0 - 6.0 - 8.0);
        assert_eq!(low.left, 8.0, "kept off the left edge");
        let wide = menu_place(rect(900.0, 100.0, 600.0, 36.0), VIEW);
        assert_eq!((wide.left, wide.width), (1280.0 - 8.0 - 600.0, 600.0), "kept off the right edge");
    }

    #[test]
    fn tips_sit_right_of_or_above_their_element() {
        // A 100 x 30 tip: right of a sidebar item (centred), above a button (centred).
        assert_eq!(
            tip_place(rect(12.0, 100.0, 48.0, 48.0), TipSide::Right, (100.0, 30.0), VIEW),
            (70.0, 109.0)
        );
        assert_eq!(tip_place(rect(100.0, 50.0, 40.0, 20.0), TipSide::Top, (100.0, 30.0), VIEW), (70.0, 12.0));
    }

    #[test]
    fn a_tip_never_leaves_the_window() {
        // Above a button at the window's right edge: moved left, its right edge 8 px in.
        let (left, _) = tip_place(rect(1240.0, 100.0, 36.0, 36.0), TipSide::Top, (300.0, 30.0), VIEW);
        assert_eq!(left, 1280.0 - 8.0 - 300.0);
        // At the left edge: moved right.
        assert_eq!(tip_place(rect(0.0, 100.0, 20.0, 20.0), TipSide::Top, (300.0, 30.0), VIEW).0, 8.0);
        // No room above: below the element.
        assert_eq!(tip_place(rect(600.0, 10.0, 40.0, 20.0), TipSide::Top, (100.0, 30.0), VIEW).1, 38.0);
        // No room on the right: on the left of the element.
        assert_eq!(
            tip_place(rect(1200.0, 300.0, 40.0, 40.0), TipSide::Right, (200.0, 30.0), VIEW).0,
            1200.0 - 10.0 - 200.0
        );
        // Near the bottom: lifted into the window.
        assert_eq!(
            tip_place(rect(12.0, 700.0, 48.0, 20.0), TipSide::Right, (100.0, 60.0), VIEW).1,
            720.0 - 8.0 - 60.0
        );
        // Wider than the window: from the left edge (CSS wraps its text first).
        assert_eq!(tip_place(rect(600.0, 300.0, 40.0, 20.0), TipSide::Top, (2000.0, 30.0), VIEW).0, 8.0);
    }

    #[test]
    fn emptying_what_is_empty_wakes_nobody() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let owner = Owner::new();
        owner.with(|| {
            let shown = RwSignal::new(Some(1));
            // Reads the signal again each time it announces a change.
            let reads = Arc::new(AtomicUsize::new(0));
            let watcher = {
                let reads = reads.clone();
                Memo::new_with_compare(
                    move |_| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        shown.get()
                    },
                    |_, _| true,
                )
            };
            let _ = watcher.get();
            empty(shown);
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), 2, "what was shown is announced gone");
            assert_eq!(shown.get_untracked(), None);
            empty(shown);
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), 2, "nothing shown, nothing announced");
        });
        owner.cleanup();
    }
}
