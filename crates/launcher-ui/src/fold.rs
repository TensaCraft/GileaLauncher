//! Sections that fold under their title (Home, the Screenshots page); each remembers how the user
//! left it.

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Icon, sound};

/// Whether section `key` (Home's, the Screenshots page's) is folded, as the user left it (kept in this window's storage;
/// unfolded when it cannot be read).
pub fn fold_state(key: impl Into<String>) -> RwSignal<bool> {
    let key: String = key.into();
    let storage = || web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    let folded =
        RwSignal::new(storage().and_then(|s| s.get_item(&key).ok().flatten()).as_deref() == Some("1"));
    Effect::new(move |previous: Option<bool>| {
        let now = folded.get();
        if previous.is_some_and(|was| was != now)
            && let Some(storage) = storage()
        {
            let _ = storage.set_item(&key, if now { "1" } else { "0" });
        }
        now
    });
    folded
}

/// A section's title on a bar the section's width (Home): a click on the title folds or unfolds
/// the section; `children` are the section's own buttons, at the right.
#[component]
pub fn FoldHead(
    icon: &'static str,
    title_key: &'static str,
    folded: RwSignal<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="fold-bar">
            <button
                type="button"
                class="fold-head"
                class:is-folded=folded
                aria-expanded=move || (!folded.get()).to_string()
                on:click=move |_| {
                    sound::play_click();
                    folded.update(|f| *f = !*f);
                }
            >
                <Icon name=icon />
                <span>{move || i18n.t(title_key)}</span>
                <Icon name="expand_more" class="fold-head__chevron" />
            </button>
            {children.map(|c| view! { <div class="fold-bar__actions">{c()}</div> })}
        </div>
    }
}
