use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Variant};

use super::ops::OpsIndicator;
use crate::store::use_store;

#[derive(Clone, Copy)]
pub struct HeaderState {
    /// Translation key of the page title.
    pub title: RwSignal<String>,
    pub subtitle: RwSignal<Option<String>>,
    pub back: RwSignal<Option<&'static str>>,
    pub actions: RwSignal<Option<ViewFn>>,
}

pub fn provide_header() -> HeaderState {
    let state = HeaderState {
        title: RwSignal::new(String::new()),
        subtitle: RwSignal::new(None),
        back: RwSignal::new(None),
        actions: RwSignal::new(None),
    };
    provide_context(state);
    state
}

pub fn use_header() -> HeaderState {
    expect_context::<HeaderState>()
}

/// Declares the current page's header. Renders nothing itself.
#[component]
pub fn PageHeader(
    title_key: &'static str,
    #[prop(optional)] back: Option<&'static str>,
    #[prop(optional)] actions: Option<ViewFn>,
) -> impl IntoView {
    let h = use_header();
    h.title.set(title_key.to_string());
    h.back.set(back);
    h.subtitle.set(None);
    h.actions.set(actions);
}

#[component]
pub fn Header() -> impl IntoView {
    let h = use_header();
    let i18n = use_i18n();
    let store = use_store();
    view! {
        <header class="header" data-window-drag="">
            {move || h.back.get().map(|path| view! {
                <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| store.go(path) />
            })}
            <div>
                <h1 class="header__title">{move || i18n.t(&h.title.get())}</h1>
                {move || h.subtitle.get().map(|s| view! { <div class="header__subtitle">{s}</div> })}
            </div>
            <div class="header__actions">
                {move || h.actions.get().map(|a| a.run())}
                <OpsIndicator />
            </div>
        </header>
    }
}
