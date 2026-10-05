//! Lists that reload in place: an answer replaces what is shown, and a reload that fails while a
//! list is shown keeps it (the page does not collapse and jump back to the top).

use std::time::Duration;

use leptos::prelude::*;

use crate::LatestRequest;

/// What a list shows after an answer, and the error to tell when a shown list was kept.
pub fn settle<T>(
    previous: Option<Result<T, String>>,
    arrived: Result<T, String>,
) -> (Option<Result<T, String>>, Option<String>) {
    match (previous, arrived) {
        (_, Ok(fresh)) => (Some(Ok(fresh)), None),
        (Some(Ok(shown)), Err(error)) => (Some(Ok(shown)), Some(error)),
        (_, Err(error)) => (Some(Err(error)), None),
    }
}

/// The pause after typing before a list follows its search.
pub const SEARCH_PAUSE: Duration = Duration::from_millis(150);

/// Makes `search` say `text`; its readers wake only when that is a change.
fn follow(search: RwSignal<String>, text: String) {
    search.try_maybe_update(|shown| {
        let changed = *shown != text;
        if changed {
            *shown = text;
        }
        (changed, ())
    });
}

/// What a list follows instead of each keystroke of `typed`: its text once typing pauses for
/// `pause`, and at once when it is blank.
pub fn debounced(typed: RwSignal<String>, pause: Duration) -> RwSignal<String> {
    let search = RwSignal::new(typed.get_untracked());
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let cancel = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    Effect::new(move |_| {
        let text = typed.get();
        cancel();
        if text.trim().is_empty() {
            follow(search, String::new());
        } else {
            timer.set_value(set_timeout_with_handle(move || follow(search, text), pause).ok());
        }
    });
    on_cleanup(cancel);
    search
}

/// A list the backend answers: `None` only before its first answer, and each reload keeps what
/// is shown until the newest answer comes (`settle`).
pub struct Reloadable<T: Send + Sync + 'static> {
    shown: RwSignal<Option<Result<T, String>>>,
    latest: StoredValue<LatestRequest, LocalStorage>,
}

impl<T: Send + Sync + 'static> Clone for Reloadable<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for Reloadable<T> {}

impl<T: Send + Sync + 'static> Default for Reloadable<T> {
    fn default() -> Self {
        Reloadable::new()
    }
}

impl<T: Send + Sync + 'static> Reloadable<T> {
    pub fn new() -> Reloadable<T> {
        Reloadable { shown: RwSignal::new(None), latest: StoredValue::new_local(LatestRequest::new()) }
    }

    /// A new request (`None` once the page is gone); what is shown stays.
    pub fn begin(&self) -> Option<u64> {
        self.latest.try_with_value(|l| l.begin())
    }

    /// `request` is the newest (its answer would land).
    pub fn is_current(&self, request: u64) -> bool {
        self.latest.try_with_value(|l| l.is_current(request)).unwrap_or(false)
    }

    /// Lands the answer to `request` when it is the newest; returns the error to tell when a shown
    /// list was kept.
    pub fn finish(&self, request: u64, arrived: Result<T, String>) -> Option<String> {
        if !self.is_current(request) {
            return None;
        }
        self.shown
            .try_update(|shown| {
                let (next, told) = settle(shown.take(), arrived);
                *shown = next;
                told
            })
            .flatten()
    }

    /// Forgets what is shown (another list takes its place, e.g. another world's) and any answer
    /// on its way: the view shows its skeleton until the next answer.
    pub fn clear(&self) {
        let _ = self.latest.try_with_value(|l| l.begin());
        let _ = self.shown.try_set(None);
    }

    /// Shows `fresh` at once (an action already answered with the whole list).
    pub fn replace(&self, fresh: T) {
        let _ = self.shown.try_set(Some(Ok(fresh)));
    }

    /// What is shown, for the view.
    pub fn shown(&self) -> RwSignal<Option<Result<T, String>>> {
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shown_list_survives_a_failed_reload() {
        let shown = Some(Ok(vec![1]));
        assert_eq!(settle(shown.clone(), Err("offline".to_string())), (shown, Some("offline".into())));
        assert_eq!(settle(Some(Ok(vec![1])), Ok(vec![2])), (Some(Ok(vec![2])), None));
        assert_eq!(settle::<Vec<u8>>(None, Err("x".into())), (Some(Err("x".into())), None));
    }

    #[test]
    fn a_reload_keeps_the_list_shown_until_the_answer() {
        let owner = Owner::new();
        owner.with(|| {
            let list = Reloadable::<Vec<u32>>::new();
            let first = list.begin().unwrap();
            assert_eq!(list.shown().get_untracked(), None, "nothing yet: a skeleton");
            list.finish(first, Ok(vec![1, 2]));
            let second = list.begin().unwrap();
            assert_eq!(list.shown().get_untracked(), Some(Ok(vec![1, 2])), "a reload shows the list still");
            assert_eq!(list.finish(second, Ok(vec![3])), None);
            assert_eq!(list.shown().get_untracked(), Some(Ok(vec![3])));
        });
    }

    #[test]
    fn a_failed_reload_keeps_the_list_shown() {
        let owner = Owner::new();
        owner.with(|| {
            let list = Reloadable::<Vec<u32>>::new();
            let first = list.begin().unwrap();
            list.finish(first, Ok(vec![1]));
            let second = list.begin().unwrap();
            assert_eq!(list.finish(second, Err("offline".into())), Some("offline".into()), "told once");
            assert_eq!(list.shown().get_untracked(), Some(Ok(vec![1])));
            let third = list.begin().unwrap();
            let fresh = Reloadable::<Vec<u32>>::new();
            let only = fresh.begin().unwrap();
            assert_eq!(fresh.finish(only, Err("down".into())), None, "nothing shown: the error is the page");
            assert_eq!(fresh.shown().get_untracked(), Some(Err("down".into())));
            list.finish(third, Ok(vec![]));
        });
    }

    #[test]
    fn an_older_answer_is_ignored() {
        let owner = Owner::new();
        owner.with(|| {
            let list = Reloadable::<Vec<u32>>::new();
            let old = list.begin().unwrap();
            let new = list.begin().unwrap();
            assert_eq!(list.finish(old, Err("late".into())), None);
            assert_eq!(list.shown().get_untracked(), None, "the late answer did not land");
            list.finish(new, Ok(vec![7]));
            list.finish(old, Ok(vec![9]));
            assert_eq!(list.shown().get_untracked(), Some(Ok(vec![7])));
        });
    }

    #[test]
    fn a_cleared_list_waits_for_its_next_answer() {
        let owner = Owner::new();
        owner.with(|| {
            let list = Reloadable::<Vec<u32>>::new();
            let first = list.begin().unwrap();
            list.clear();
            assert_eq!(list.finish(first, Ok(vec![1])), None);
            assert_eq!(list.shown().get_untracked(), None, "the old list's answer does not land");
        });
    }

    #[test]
    fn a_gone_page_takes_no_request() {
        let owner = Owner::new();
        let list = owner.with(Reloadable::<Vec<u32>>::new);
        drop(owner);
        assert_eq!(list.begin(), None);
    }

    #[test]
    fn a_search_that_stays_the_same_wakes_nobody() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let owner = Owner::new();
        owner.with(|| {
            let search = RwSignal::new(String::new());
            // Reads the search again each time it announces a change.
            let reads = Arc::new(AtomicUsize::new(0));
            let watcher = {
                let reads = reads.clone();
                Memo::new_with_compare(
                    move |_| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        search.get()
                    },
                    |_, _| true,
                )
            };
            let _ = watcher.get();
            follow(search, String::new());
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), 1, "still empty");
            follow(search, "sod".into());
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), 2);
            assert_eq!(search.get_untracked(), "sod");
            follow(search, "sod".into());
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), 2, "the same text again");
        });
        owner.cleanup();
    }
}
