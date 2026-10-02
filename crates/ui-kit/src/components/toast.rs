use std::time::Duration;

use launcher_shared::Level;
use leptos::prelude::*;

use super::button::{Button, Size, Variant};
use super::icon::Icon;

pub const TOAST_LIMIT: usize = 4;
const LOCAL_ID_BASE: u64 = 1 << 40;

#[derive(Clone, Debug, PartialEq)]
pub struct ToastItem {
    pub id: u64,
    pub level: Level,
    pub title: String,
    pub message: Option<String>,
    /// (label, action id)
    pub action: Option<(String, String)>,
    pub duration_ms: u32,
}

pub fn push_toast(list: &mut Vec<ToastItem>, item: ToastItem) {
    list.retain(|t| t.id != item.id);
    list.push(item);
    while list.len() > TOAST_LIMIT {
        list.remove(0);
    }
}

pub fn duration_for(level: Level) -> u32 {
    match level {
        Level::Info | Level::Success => 6000,
        Level::Warning => 9000,
        Level::Error => 12000,
    }
}

fn level_class(level: Level) -> &'static str {
    match level {
        Level::Info => "toast--info",
        Level::Success => "toast--success",
        Level::Warning => "toast--warning",
        Level::Error => "toast--error",
    }
}

fn level_icon(level: Level) -> &'static str {
    match level {
        Level::Info => "info",
        Level::Success => "check",
        Level::Warning => "priority_high",
        Level::Error => "error",
    }
}

#[derive(Clone, Copy)]
pub struct Toasts {
    items: RwSignal<Vec<ToastItem>>,
    next_local: RwSignal<u64>,
}

impl Toasts {
    pub fn push(&self, item: ToastItem) {
        self.items.update(|list| push_toast(list, item));
    }

    pub fn show(&self, level: Level, title: String, message: Option<String>) {
        let id = self.next_local.get_untracked();
        self.next_local.set(id + 1);
        self.push(ToastItem { id, level, title, message, action: None, duration_ms: duration_for(level) });
    }

    pub fn dismiss(&self, id: u64) {
        self.items.update(|list| list.retain(|t| t.id != id));
    }
}

pub fn provide_toasts() -> Toasts {
    let toasts = Toasts { items: RwSignal::new(Vec::new()), next_local: RwSignal::new(LOCAL_ID_BASE) };
    provide_context(toasts);
    toasts
}

pub fn use_toasts() -> Toasts {
    expect_context::<Toasts>()
}

#[component]
fn ToastView(
    item: ToastItem,
    toasts: Toasts,
    on_action: Option<Callback<String>>,
    close_label: Signal<String>,
) -> impl IntoView {
    let id = item.id;
    let duration = item.duration_ms;
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let schedule = move || {
        if let Ok(h) =
            set_timeout_with_handle(move || toasts.dismiss(id), Duration::from_millis(duration as u64))
        {
            timer.set_value(Some(h));
        }
    };
    let cancel = move || {
        if let Some(h) = timer.get_value() {
            h.clear();
        }
        timer.set_value(None);
    };
    schedule();
    on_cleanup(cancel);

    view! {
        <div
            class=format!("toast {}", level_class(item.level))
            role="status"
            on:mouseenter=move |_| cancel()
            on:mouseleave=move |_| schedule()
        >
            <div class="toast__icon"><Icon name=level_icon(item.level) /></div>
            <div class="toast__body">
                <b class="toast__title">{item.title.clone()}</b>
                {item.message.clone().map(|m| view! { <span class="toast__msg">{m}</span> })}
                {item.action.clone().map(|(label, action)| view! {
                    <div class="toast__actions">
                        <Button variant=Variant::Secondary size=Size::Sm on_click=move |_| {
                            toasts.dismiss(id);
                            if let Some(cb) = on_action {
                                cb.run(action.clone());
                            }
                        }>{label}</Button>
                    </div>
                })}
            </div>
            <button class="toast__close" data-tip=move || close_label.get() data-tip-side="top" aria-label=move || close_label.get() on:click=move |_| toasts.dismiss(id)>
                <Icon name="close" size=17 />
            </button>
            <span class="toast__timer" style=format!("animation-duration:{duration}ms")></span>
        </div>
    }
}

#[component]
pub fn Toaster(
    #[prop(optional, into)] on_action: Option<Callback<String>>,
    #[prop(into)] close_label: Signal<String>,
) -> impl IntoView {
    let toasts = use_toasts();
    view! {
        <div class="toaster" aria-live="polite">
            <For
                each=move || toasts.items.get()
                key=|t| t.id
                children=move |item| view! { <ToastView item=item toasts=toasts on_action=on_action close_label=close_label /> }
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64) -> ToastItem {
        ToastItem {
            id,
            level: Level::Info,
            title: format!("t{id}"),
            message: None,
            action: None,
            duration_ms: 1000,
        }
    }

    #[test]
    fn keeps_at_most_four_newest_toasts() {
        let mut list = Vec::new();
        for id in 1..=6 {
            push_toast(&mut list, item(id));
        }
        assert_eq!(list.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 4, 5, 6]);
    }

    #[test]
    fn warnings_and_errors_stay_longer() {
        assert!(duration_for(Level::Error) > duration_for(Level::Warning));
        assert!(duration_for(Level::Warning) > duration_for(Level::Info));
        assert_eq!(duration_for(Level::Info), duration_for(Level::Success));
    }
}
