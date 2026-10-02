use leptos::ev;
use leptos::portal::Portal;
use leptos::prelude::*;

use super::button::Size;
use super::float::{MenuPlace, Rect, menu_place, viewport};
use super::icon::Icon;
use crate::sound;

#[derive(Clone, Debug, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub group: Option<String>,
    pub meta: Option<String>,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), group: None, meta: None }
    }

    pub fn in_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn with_meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

/// Keyboard navigation with wrap-around.
pub fn next_index(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let len_i = len as i32;
    let base = match current {
        Some(i) => i as i32 + delta,
        None if delta >= 0 => 0,
        None => len_i - 1,
    };
    Some(base.rem_euclid(len_i) as usize)
}

#[component]
pub fn Select(
    #[prop(into)] options: Signal<Vec<SelectOption>>,
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
    #[prop(optional, into)] icon: Option<String>,
    #[prop(optional, into)] on_change: Option<Callback<String>>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    /// The same heights as buttons (`Size::Md` by default).
    #[prop(optional)]
    size: Size,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let active = RwSignal::new(None::<usize>);
    // The menu is drawn in a portal above everything, placed by the control's box when it opens.
    let control = NodeRef::<leptos::html::Div>::new();
    let place = RwSignal::new(None::<MenuPlace>);
    let resize = window_event_listener(ev::resize, move |_| {
        open.try_set(false);
    });
    on_cleanup(move || resize.remove());
    let off = move || disabled.get().unwrap_or(false);

    let choose = move |v: String| {
        sound::play_click();
        open.set(false);
        if value.get_untracked() != v {
            value.set(v.clone());
            if let Some(cb) = on_change {
                cb.run(v);
            }
        }
    };
    let toggle = move || {
        if off() {
            return;
        }
        let now_open = !open.get_untracked();
        if now_open {
            let current = value.get_untracked();
            active.set(options.with_untracked(|o| o.iter().position(|x| x.value == current)));
            place.set(control.get_untracked().map(|el| menu_place(Rect::of(&el), viewport())));
        }
        open.set(now_open);
    };
    let selected_label = move || {
        let v = value.get();
        options.with(|o| o.iter().find(|x| x.value == v).map(|x| x.label.clone()))
    };

    view! {
        <div class="select" class:is-open=move || open.get()>
            <div
                node_ref=control
                class=format!("control {}", size.control_class())
                class:is-focus=move || open.get()
                class:is-disabled=off
                tabindex="0"
                role="combobox"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| toggle()
                on:keydown=move |ev| match ev.key().as_str() {
                    "Enter" | " " => {
                        ev.prevent_default();
                        if open.get_untracked() {
                            let pick = active
                                .get_untracked()
                                .and_then(|i| options.with_untracked(|o| o.get(i).map(|x| x.value.clone())));
                            if let Some(v) = pick {
                                choose(v);
                            }
                        } else {
                            toggle();
                        }
                    }
                    "ArrowDown" | "ArrowUp" => {
                        ev.prevent_default();
                        if !open.get_untracked() {
                            toggle();
                        }
                        let delta = if ev.key() == "ArrowDown" { 1 } else { -1 };
                        let len = options.with_untracked(Vec::len);
                        active.set(next_index(len, active.get_untracked(), delta));
                    }
                    // An open list takes Escape for itself: the dialog around it stays.
                    "Escape" if open.get_untracked() => {
                        ev.prevent_default();
                        ev.stop_propagation();
                        open.set(false);
                    }
                    _ => {}
                }
            >
                {icon.map(|name| view! { <Icon name=name outlined=true /> })}
                <span class="select__value">
                    {move || match selected_label() {
                        Some(l) => view! { <span>{l}</span> }.into_any(),
                        None => view! { <span class="select__placeholder">{placeholder.get()}</span> }.into_any(),
                    }}
                </span>
                <Icon name="expand_more" class="select__caret" />
            </div>
            <Show when=move || open.get()>
                <Portal>
                <div class="menu-catcher is-floating" on:click=move |_| open.set(false)></div>
                <div
                    class="menu is-floating"
                    role="listbox"
                    style=move || place.get().map(|p| p.style()).unwrap_or_default()
                >
                    {move || {
                        let current = value.get();
                        let mut last_group: Option<String> = None;
                        options
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, opt)| {
                                let header = (opt.group.is_some() && opt.group != last_group)
                                    .then(|| view! { <div class="menu__group">{opt.group.clone()}</div> });
                                last_group = opt.group.clone();
                                let is_selected = opt.value == current;
                                let v = opt.value.clone();
                                view! {
                                    {header}
                                    <div
                                        class="menu__item"
                                        class:is-selected=is_selected
                                        class:is-active=move || active.get() == Some(i)
                                        role="option"
                                        on:mouseenter=move |_| active.set(Some(i))
                                        on:click=move |ev| {
                                            ev.stop_propagation();
                                            choose(v.clone());
                                        }
                                    >
                                        <span>{opt.label.clone()}</span>
                                        {opt.meta.clone().map(|m| view! { <span class="menu__meta">{m}</span> })}
                                        <Icon name="check" class="menu__check" />
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                </div>
                </Portal>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_navigation_wraps() {
        assert_eq!(next_index(0, None, 1), None);
        assert_eq!(next_index(3, None, 1), Some(0));
        assert_eq!(next_index(3, None, -1), Some(2));
        assert_eq!(next_index(3, Some(2), 1), Some(0));
        assert_eq!(next_index(3, Some(0), -1), Some(2));
    }
}
