//! One-time hints: a ring around a control and a bubble beside it saying what the control is for,
//! drawn above everything. A page marks the control with `data-nudge*` attributes (`NudgeMark`);
//! the app's one `NudgeLayer` finds it and draws the hint while the app says one may show.

use std::time::Duration;

use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use super::float::{Rect, viewport};
use super::icon::Icon;
use super::menu::ContextMenu;
use crate::sound;

/// A hint a page puts on a control: the app's `NudgeLayer` draws it there. The control carries it
/// as `data-nudge` (the id), `data-nudge-text`, `data-nudge-side` and, when the hint can be closed,
/// `data-nudge-close` (the close button's label).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NudgeMark {
    pub id: String,
    pub text: String,
    pub side: NudgeSide,
    /// The ✕ button's label; `None`: the hint has no ✕.
    pub close: Option<String>,
}

/// While nothing is marked, the layer looks again this often (with a mark, every frame).
const LOOK_AGAIN: Duration = Duration::from_millis(250);

/// The marked control as the layer last saw it.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    id: String,
    text: String,
    close: Option<String>,
    geometry: Geometry,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    anchor: Rect,
    radius: f64,
    /// `None` until the bubble has been measured.
    place: Option<NudgePlace>,
    asked: NudgeSide,
}

/// A corner radius as computed (`12px`, `50%`) in pixels for a box of `size`.
fn radius_px(computed: &str, size: (f64, f64)) -> f64 {
    let short = size.0.min(size.1);
    let value = computed.split_whitespace().next().unwrap_or_default();
    let px = if let Some(pct) = value.strip_suffix('%') {
        pct.parse::<f64>().map_or(0.0, |p| p / 100.0 * short)
    } else {
        value.trim_end_matches("px").parse::<f64>().unwrap_or(0.0)
    };
    px.clamp(0.0, short / 2.0)
}

/// The control is in sight: a hit test at its middle and near its corners (inside the rounding)
/// lands on it, so nothing covers it and no scroll box has cut it off.
fn in_sight(element: &Element, anchor: Rect, radius: f64) -> bool {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return false };
    let inset = (2.0 + radius * 0.3).min(anchor.width / 2.0).min(anchor.height / 2.0);
    let (left, top) = (anchor.x + inset, anchor.y + inset);
    let (right, bottom) = (anchor.x + anchor.width - inset, anchor.y + anchor.height - inset);
    let middle = (anchor.x + anchor.width / 2.0, anchor.y + anchor.height / 2.0);
    [middle, (left, top), (right, top), (left, bottom), (right, bottom)].into_iter().all(|(x, y)| {
        document
            .element_from_point(x as f32, y as f32)
            .is_some_and(|hit| element.contains(Some(hit.unchecked_ref())))
    })
}

/// What the layer should draw now: the marked control, when there is one in sight.
fn look(bubble: Option<web_sys::HtmlDivElement>, menu_open: bool) -> Option<Seen> {
    if menu_open {
        return None;
    }
    let window = web_sys::window()?;
    let element = window.document()?.query_selector("[data-nudge]").ok()??;
    let id = element.get_attribute("data-nudge")?;
    let text = element.get_attribute("data-nudge-text").filter(|t| !t.trim().is_empty())?;
    let anchor = Rect::of(&element);
    if anchor.width < 1.0 || anchor.height < 1.0 {
        return None;
    }
    let computed = window
        .get_computed_style(&element)
        .ok()
        .flatten()
        .and_then(|style| style.get_property_value("border-top-left-radius").ok())
        .unwrap_or_default();
    let radius = radius_px(&computed, (anchor.width, anchor.height));
    if !in_sight(&element, anchor, radius) {
        return None;
    }
    let asked = NudgeSide::from_attr(&element.get_attribute("data-nudge-side").unwrap_or_default());
    let place = bubble
        .map(|b| b.get_bounding_client_rect())
        .filter(|r| r.width() > 0.0)
        .map(|r| nudge_place(anchor, asked, (r.width(), r.height()), viewport()));
    Some(Seen {
        id,
        text,
        close: element.get_attribute("data-nudge-close"),
        geometry: Geometry { anchor, radius, place, asked },
    })
}

/// The ring: hugging the control's edge, with its rounding; it grows about 10 px out as it fades.
fn ring_style(g: &Geometry) -> String {
    let Rect { x, y, width, height } = g.anchor;
    let grow = |length: f64| (length + 24.0) / (length + 4.0);
    format!(
        "left:{:.1}px;top:{:.1}px;width:{:.1}px;height:{:.1}px;border-radius:{:.1}px;--nudge-sx:{:.3};--nudge-sy:{:.3}",
        x - 2.0,
        y - 2.0,
        width + 4.0,
        height + 4.0,
        g.radius + 2.0,
        grow(width),
        grow(height)
    )
}

/// The bubble: at its place, or out of sight until it has been measured.
fn bubble_style(g: &Geometry) -> String {
    match g.place {
        Some(p) => format!("left:{:.1}px;top:{:.1}px;--nudge-arrow:{:.1}px", p.x, p.y, p.arrow),
        None => "left:0;top:0;visibility:hidden".to_string(),
    }
}

/// The app's one hint layer, drawn above everything. While `active` (the app's "a hint may show
/// now"), it looks for the control a page marked (`NudgeMark`) and draws a ring around it and a
/// bubble beside it, following it as the page scrolls or changes. The hint hides while the control
/// is out of sight (scrolled away, folded, under a dialog) or the context menu is open. Neither
/// the ring nor the bubble takes clicks, except the bubble's ✕, which calls `on_close` with the id.
#[component]
pub fn NudgeLayer(#[prop(into)] active: Signal<bool>, on_close: Callback<String>) -> impl IntoView {
    let content = RwSignal::new(None::<(String, String, Option<String>)>);
    let geometry = RwSignal::new(None::<Geometry>);
    let bubble = NodeRef::<leptos::html::Div>::new();
    let running = StoredValue::new(false);
    let menu = use_context::<ContextMenu>();
    let show = move |seen: Option<Seen>| {
        let (what, shape) = match seen {
            Some(s) => (Some((s.id, s.text, s.close)), Some(s.geometry)),
            None => (None, None),
        };
        content.maybe_update(|c| {
            let changed = *c != what;
            if changed {
                *c = what;
            }
            changed
        });
        geometry.maybe_update(|g| {
            let changed = *g != shape;
            if changed {
                *g = shape;
            }
            changed
        });
    };
    // One look; then the next one: a frame later while a hint shows, a moment later otherwise.
    fn next(step: impl Fn() -> Option<bool> + Copy + 'static) {
        match step() {
            Some(true) => request_animation_frame(move || next(step)),
            Some(false) => set_timeout(move || next(step), LOOK_AGAIN),
            None => {}
        }
    }
    let step = move || -> Option<bool> {
        if !running.try_get_value().unwrap_or(false) {
            return None;
        }
        let seen = look(bubble.get_untracked(), menu.is_some_and(|m| m.is_open_untracked()));
        let found = seen.is_some();
        show(seen);
        Some(found)
    };
    Effect::new(move |_| {
        if active.get() {
            if !running.get_value() {
                running.set_value(true);
                next(step);
            }
        } else {
            running.set_value(false);
            show(None);
        }
    });
    on_cleanup(move || {
        running.try_set_value(false);
    });
    view! {
        <Portal>
            {move || {
                content.get().map(|(id, text, close)| {
                    let shape = move || geometry.get();
                    let side = move || {
                        let g = shape();
                        g.and_then(|g| g.place.map(|p| p.side)).or(g.map(|g| g.asked)).unwrap_or_default()
                    };
                    view! {
                        <div class="nudge-ring" aria-hidden="true" style=move || shape().map(|g| ring_style(&g))></div>
                        <div
                            class=move || format!("nudge nudge--{}", side().attr())
                            node_ref=bubble
                            role="status"
                            style=move || shape().map(|g| bubble_style(&g))
                        >
                            <span class="nudge__text">{text}</span>
                            {close.map(|label| {
                                let id = id.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="nudge__close"
                                        aria-label=label
                                        on:click=move |_| {
                                            sound::play_click();
                                            on_close.run(id.clone());
                                        }
                                    >
                                        <span aria-hidden="true"><Icon name="close" /></span>
                                    </button>
                                }
                            })}
                            <span class="nudge__arrow" aria-hidden="true"></span>
                        </div>
                    }
                })
            }}
        </Portal>
    }
}

/// Between the control and the bubble: room for the arrow and the ring.
const GAP: f64 = 14.0;
/// Space kept from the window's edges.
const EDGE: f64 = 8.0;
/// The arrow keeps this far from the bubble's ends (off its rounded corners).
const ARROW_MIN: f64 = 14.0;

/// The side of its control a bubble goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NudgeSide {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
}

impl NudgeSide {
    /// The `data-nudge-side` value.
    pub fn attr(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    /// The side a `data-nudge-side` value names (right for anything else).
    pub fn from_attr(value: &str) -> Self {
        match value {
            "left" => Self::Left,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => Self::Right,
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Right => Self::Left,
            Self::Left => Self::Right,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    fn across(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }
}

/// Where a bubble goes, in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NudgePlace {
    pub x: f64,
    pub y: f64,
    /// The side it went to (the other one when the asked side had no room).
    pub side: NudgeSide,
    /// Where its arrow sits along the edge facing the control: from its left (above or below the
    /// control) or its top (beside it).
    pub arrow: f64,
}

/// Where a bubble of `bubble` size goes beside `anchor`: on `side`, centred on the anchor, on the
/// other side when there is no room, and never past the window's edges.
pub fn nudge_place(anchor: Rect, side: NudgeSide, bubble: (f64, f64), viewport: (f64, f64)) -> NudgePlace {
    let (width, height) = bubble;
    let start = |side: NudgeSide| match side {
        NudgeSide::Right => anchor.x + anchor.width + GAP,
        NudgeSide::Left => anchor.x - GAP - width,
        NudgeSide::Bottom => anchor.y + anchor.height + GAP,
        NudgeSide::Top => anchor.y - GAP - height,
    };
    let fits = |side: NudgeSide| {
        let at = start(side);
        match side {
            NudgeSide::Right => at + width <= viewport.0 - EDGE,
            NudgeSide::Bottom => at + height <= viewport.1 - EDGE,
            NudgeSide::Left | NudgeSide::Top => at >= EDGE,
        }
    };
    let side = if fits(side) || !fits(side.opposite()) { side } else { side.opposite() };
    let fit = |at: f64, length: f64, room: f64| at.min(room - EDGE - length).max(EDGE);
    if side.across() {
        let middle = anchor.y + anchor.height / 2.0;
        let y = fit(middle - height / 2.0, height, viewport.1);
        let arrow = (middle - y).clamp(ARROW_MIN, (height - ARROW_MIN).max(ARROW_MIN));
        NudgePlace { x: fit(start(side), width, viewport.0), y, side, arrow }
    } else {
        let middle = anchor.x + anchor.width / 2.0;
        let x = fit(middle - width / 2.0, width, viewport.0);
        let arrow = (middle - x).clamp(ARROW_MIN, (width - ARROW_MIN).max(ARROW_MIN));
        NudgePlace { x, y: fit(start(side), height, viewport.1), side, arrow }
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
    fn a_bubble_goes_to_its_side_with_a_gap_and_centred() {
        let p = nudge_place(rect(100.0, 300.0, 40.0, 40.0), NudgeSide::Right, (200.0, 36.0), VIEW);
        assert_eq!((p.side, p.x, p.y), (NudgeSide::Right, 100.0 + 40.0 + GAP, 300.0 + 20.0 - 18.0));
        assert_eq!(p.arrow, 18.0, "the arrow points at the anchor's middle");
        let p = nudge_place(rect(600.0, 300.0, 120.0, 36.0), NudgeSide::Left, (200.0, 36.0), VIEW);
        assert_eq!((p.side, p.x), (NudgeSide::Left, 600.0 - GAP - 200.0));
        let p = nudge_place(rect(600.0, 100.0, 120.0, 36.0), NudgeSide::Bottom, (200.0, 36.0), VIEW);
        assert_eq!((p.side, p.y), (NudgeSide::Bottom, 100.0 + 36.0 + GAP));
        assert_eq!(p.x, 600.0 + 60.0 - 100.0, "centred under the anchor");
        let p = nudge_place(rect(600.0, 300.0, 120.0, 36.0), NudgeSide::Top, (200.0, 36.0), VIEW);
        assert_eq!((p.side, p.y), (NudgeSide::Top, 300.0 - GAP - 36.0));
    }

    #[test]
    fn a_bubble_without_room_flips_to_the_other_side() {
        let p = nudge_place(rect(1200.0, 300.0, 60.0, 36.0), NudgeSide::Right, (200.0, 36.0), VIEW);
        assert_eq!(p.side, NudgeSide::Left);
        let p = nudge_place(rect(20.0, 300.0, 60.0, 36.0), NudgeSide::Left, (200.0, 36.0), VIEW);
        assert_eq!(p.side, NudgeSide::Right);
        let p = nudge_place(rect(600.0, 700.0, 60.0, 16.0), NudgeSide::Bottom, (200.0, 36.0), VIEW);
        assert_eq!(p.side, NudgeSide::Top);
        let p = nudge_place(rect(600.0, 4.0, 60.0, 16.0), NudgeSide::Top, (200.0, 36.0), VIEW);
        assert_eq!(p.side, NudgeSide::Bottom);
    }

    #[test]
    fn a_bubble_stays_inside_the_window_and_its_arrow_follows_the_anchor() {
        // Under an anchor at the left edge: the bubble is pushed right, the arrow moves left with
        // the anchor.
        let p = nudge_place(rect(0.0, 100.0, 60.0, 36.0), NudgeSide::Bottom, (300.0, 36.0), VIEW);
        assert_eq!(p.x, EDGE);
        assert_eq!(p.arrow, 30.0 - EDGE, "the arrow points at the anchor's middle");
        // An anchor at the very corner: the arrow stays on the bubble's straight edge.
        let p = nudge_place(rect(0.0, 100.0, 4.0, 36.0), NudgeSide::Bottom, (300.0, 36.0), VIEW);
        assert_eq!(p.arrow, ARROW_MIN);
        // Beside an anchor low in the window: the bubble is lifted inside it.
        let p = nudge_place(rect(100.0, 680.0, 40.0, 20.0), NudgeSide::Right, (200.0, 60.0), VIEW);
        assert_eq!(p.y, 720.0 - EDGE - 60.0);
        assert_eq!(p.arrow, 690.0 - p.y);
        let p = nudge_place(rect(100.0, 700.0, 40.0, 20.0), NudgeSide::Right, (200.0, 60.0), VIEW);
        assert_eq!(p.arrow, 60.0 - ARROW_MIN, "and at the far end it stays on the edge too");
    }

    #[test]
    fn a_corner_radius_reads_in_pixels_or_percent_and_never_past_half() {
        assert_eq!(radius_px("12px", (100.0, 40.0)), 12.0);
        assert_eq!(radius_px("50%", (40.0, 40.0)), 20.0);
        assert_eq!(radius_px("10px 4px", (100.0, 40.0)), 10.0);
        assert_eq!(radius_px("999px", (100.0, 40.0)), 20.0);
        assert_eq!(radius_px("", (100.0, 40.0)), 0.0);
    }

    #[test]
    fn sides_read_from_their_attribute() {
        assert_eq!(NudgeSide::from_attr("left"), NudgeSide::Left);
        assert_eq!(NudgeSide::from_attr("bottom"), NudgeSide::Bottom);
        assert_eq!(NudgeSide::from_attr("top"), NudgeSide::Top);
        assert_eq!(NudgeSide::from_attr("nonsense"), NudgeSide::Right);
        for side in [NudgeSide::Right, NudgeSide::Left, NudgeSide::Top, NudgeSide::Bottom] {
            assert_eq!(NudgeSide::from_attr(side.attr()), side);
        }
    }
}
