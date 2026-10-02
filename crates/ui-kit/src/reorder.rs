//! Putting things in order by dragging: where a dragged item lands among the others, the drag
//! in progress, and the wrapper that makes an item draggable (with a line where it would land).

use leptos::ev::DragEvent;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// `order` with `dragged` put before (or `after`) `target`; `None` when nothing moves.
pub fn moved(order: &[String], dragged: &str, target: &str, after: bool) -> Option<Vec<String>> {
    if dragged == target || !order.iter().any(|k| k == target) {
        return None;
    }
    order.iter().position(|k| k == dragged)?;
    let mut next: Vec<String> = order.iter().filter(|k| *k != dragged).cloned().collect();
    let at = next.iter().position(|k| k == target)? + usize::from(after);
    next.insert(at, dragged.to_string());
    (next.as_slice() != order).then_some(next)
}

/// A drag among keyed items: which one is dragged and where it would land.
#[derive(Clone, Copy)]
pub struct Reorder {
    dragged: RwSignal<Option<String>>,
    over: RwSignal<Option<(String, bool)>>,
}

impl Default for Reorder {
    fn default() -> Self {
        Reorder { dragged: RwSignal::new(None), over: RwSignal::new(None) }
    }
}

impl Reorder {
    fn end(&self) {
        self.dragged.set(None);
        self.over.set(None);
    }

    /// The new order once the dragged item is dropped, when it changed.
    pub fn drop_on(&self, order: &[String]) -> Option<Vec<String>> {
        let dragged = self.dragged.get_untracked();
        let over = self.over.get_untracked();
        self.end();
        let (target, after) = over?;
        moved(order, &dragged?, &target, after)
    }
}

/// Whether the pointer is past the middle of the item the drag is over (right of it in a row of
/// cards, below it in a list).
fn past_middle(ev: &DragEvent, horizontal: bool) -> bool {
    let Some(item) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
        return false;
    };
    let rect = item.get_bounding_client_rect();
    if horizontal {
        f64::from(ev.client_x()) > rect.left() + rect.width() / 2.0
    } else {
        f64::from(ev.client_y()) > rect.top() + rect.height() / 2.0
    }
}

/// An item that can be dragged among the others of its `reorder`; `on_drop` runs when one is
/// dropped on it. `horizontal`: the items stand in a row (cards) rather than a column (rows).
#[component]
pub fn Reorderable(
    reorder: Reorder,
    #[prop(into)] key: String,
    #[prop(optional)] horizontal: bool,
    on_drop: Callback<()>,
    children: Children,
) -> impl IntoView {
    let key = StoredValue::new(key);
    let mine = move |k: &Option<String>| k.as_deref() == Some(key.get_value().as_str());
    let target =
        move || reorder.over.with(|o| o.as_ref().filter(|(k, _)| *k == key.get_value()).map(|(_, a)| *a));
    view! {
        <div
            class="reorder"
            class:reorder--x=horizontal
            class:reorder--y=!horizontal
            class:is-dragging=move || reorder.dragged.with(mine)
            class:is-drop-before=move || target() == Some(false)
            class:is-drop-after=move || target() == Some(true)
            draggable="true"
            on:dragstart=move |ev: DragEvent| {
                if let Some(data) = ev.data_transfer() {
                    data.set_effect_allowed("move");
                    let _ = data.set_data("text/plain", &key.get_value());
                }
                reorder.dragged.set(Some(key.get_value()));
            }
            on:dragover=move |ev: DragEvent| {
                if reorder.dragged.with_untracked(Option::is_some) {
                    ev.prevent_default();
                    let over = Some((key.get_value(), past_middle(&ev, horizontal)));
                    if reorder.over.with_untracked(|o| *o != over) {
                        reorder.over.set(over);
                    }
                }
            }
            on:drop=move |ev: DragEvent| {
                ev.prevent_default();
                on_drop.run(());
            }
            on:dragend=move |_| reorder.end()
        >
            {children()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn moving_a_build_puts_it_before_or_after_its_target() {
        let order = keys(&["a", "b", "c", "d"]);
        assert_eq!(moved(&order, "d", "b", false), Some(keys(&["a", "d", "b", "c"])));
        assert_eq!(moved(&order, "a", "c", true), Some(keys(&["b", "c", "a", "d"])));
        assert_eq!(moved(&order, "b", "d", true), Some(keys(&["a", "c", "d", "b"])));
        assert_eq!(moved(&order, "x", "b", false), None, "not in the list");
    }

    #[test]
    fn dropping_a_build_on_itself_changes_nothing() {
        let order = keys(&["a", "b", "c"]);
        assert_eq!(moved(&order, "b", "b", true), None);
        assert_eq!(moved(&order, "a", "b", false), None, "already right before it");
        assert_eq!(moved(&order, "c", "b", true), None, "already right after it");
    }
}
