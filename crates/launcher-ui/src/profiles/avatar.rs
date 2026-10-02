//! Player heads: requested once per profile and remembered for the session.

use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::{Icon, ipc};

use crate::store::{AppStore, use_store};

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

fn request_avatar(store: AppStore, key: &str) {
    if store.avatars.with_untracked(|map| map.contains_key(key)) {
        return;
    }
    store.avatars.update(|map| {
        map.insert(key.to_string(), None);
    });
    let key = key.to_string();
    spawn_local(async move {
        let args = KeyArgs { key: key.clone() };
        if let Ok(Some(src)) = ipc::invoke::<_, Option<String>>("profile_avatar", &args).await {
            store.avatars.update(|map| {
                map.insert(key, Some(src));
            });
        }
    });
}

#[component]
pub fn ProfileAvatar(
    #[prop(into)] profile_key: Signal<Option<String>>,
    size: u32,
    #[prop(optional, into)] fallback: Option<String>,
) -> impl IntoView {
    let store = use_store();
    let failed = RwSignal::new(false);
    let fallback = fallback.unwrap_or_else(|| "person".to_string());
    Effect::new(move |_| {
        if let Some(key) = profile_key.get() {
            failed.set(false);
            request_avatar(store, &key);
        }
    });
    let src =
        move || profile_key.get().and_then(|key| store.avatars.with(|map| map.get(&key).cloned().flatten()));
    view! {
        <span class="avatar" style=format!("width:{size}px;height:{size}px")>
            {move || match src() {
                Some(url) if !failed.get() => view! { <img src=url alt="" on:error=move |_| failed.set(true) /> }.into_any(),
                _ => view! { <Icon name=fallback.clone() /> }.into_any(),
            }}
        </span>
    }
}
