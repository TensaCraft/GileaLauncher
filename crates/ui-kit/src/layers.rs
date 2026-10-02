//! The layers open over the page — dialogs, the context menu: Escape closes only the top one.
//! A layer that handles Escape marks the key press handled (`prevent_default`), so a layer below
//! it, whose listener runs for the same press, leaves it.

use std::cell::{Cell, RefCell};

thread_local! {
    static STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// A layer opened now, on top of the others.
pub fn push() -> u64 {
    let layer = NEXT.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    STACK.with(|stack| stack.borrow_mut().push(layer));
    layer
}

/// The layer closed (wherever it is in the stack).
pub fn pop(layer: u64) {
    STACK.with(|stack| stack.borrow_mut().retain(|l| *l != layer));
}

/// `layer` is the top one: Escape is its to handle.
pub fn is_top(layer: u64) -> bool {
    STACK.with(|stack| stack.borrow().last() == Some(&layer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_closes_only_the_top_layer() {
        let dialog = push();
        let confirm = push();
        assert!(is_top(confirm) && !is_top(dialog), "the confirmation over the dialog goes first");
        pop(confirm);
        assert!(is_top(dialog), "then the dialog");
        pop(dialog);
    }

    #[test]
    fn a_closed_layer_leaves_the_stack() {
        let (below, middle, top) = (push(), push(), push());
        pop(middle);
        assert!(is_top(top) && !is_top(middle) && !is_top(below));
        pop(top);
        assert!(is_top(below));
        pop(below);
        assert!(!is_top(below), "nothing is open");
    }
}
