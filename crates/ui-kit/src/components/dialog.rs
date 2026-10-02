use leptos::leptos_dom::helpers::request_animation_frame;
use leptos::portal::Portal;
use leptos::prelude::*;

use super::button::{Button, Size, Variant};
use super::icon::Icon;
use crate::layers;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogTone {
    #[default]
    Normal,
    Warning,
    Danger,
}

impl DialogTone {
    fn icon_class(self) -> &'static str {
        match self {
            DialogTone::Normal => "dialog__icon",
            DialogTone::Warning => "dialog__icon dialog__icon--warning",
            DialogTone::Danger => "dialog__icon dialog__icon--danger",
        }
    }
}

/// Footer slot of `Dialog` (action buttons).
#[slot]
pub struct DialogFooter {
    children: ChildrenFn,
}

/// Focuses the first field marked `autofocus` in `body` (the attribute alone works only for the
/// document's first paint, not for a dialog opened later).
fn focus_first_autofocus(body: NodeRef<leptos::html::Div>) {
    use wasm_bindgen::JsCast;
    // The dialog may be gone before its first frame.
    let target = body
        .try_get_untracked()
        .flatten()
        .and_then(|el| el.query_selector("[autofocus]").ok().flatten())
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
    if let Some(field) = target {
        let _ = field.focus();
    }
}

#[component]
pub fn Dialog(
    open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] subtitle: MaybeProp<String>,
    #[prop(optional, into)] icon: Option<String>,
    #[prop(optional)] tone: DialogTone,
    #[prop(optional)] wide: bool,
    /// As large as the window allows, for a view with its own scrolling (the launcher log).
    #[prop(optional)]
    full: bool,
    /// When true the dialog can only be closed by its own buttons.
    #[prop(optional)]
    locked: bool,
    #[prop(optional, into)] on_close: Option<Callback<()>>,
    #[prop(optional, into)] footer_hint: MaybeProp<String>,
    children: ChildrenFn,
    #[prop(optional)] dialog_footer: Option<DialogFooter>,
) -> impl IntoView {
    let close = move || {
        if open.get_untracked() {
            open.set(false);
            if let Some(cb) = on_close {
                cb.run(());
            }
        }
    };
    let children = StoredValue::new(children);
    let footer = StoredValue::new(dialog_footer.map(|f| f.children));
    let icon = StoredValue::new(icon);

    view! {
        <Show when=move || open.get()>
            <Portal>
                {move || {
                    // Escape closes the top layer only: a dropdown or a dialog over this one takes
                    // it first (and marks it handled).
                    let layer = layers::push();
                    let handle = window_event_listener(leptos::ev::keydown, move |ev| {
                        if ev.key() == "Escape" && !ev.default_prevented() && layers::is_top(layer) {
                            ev.prevent_default();
                            if !locked {
                                close();
                            }
                        }
                    });
                    on_cleanup(move || {
                        handle.remove();
                        layers::pop(layer);
                    });
                    // The field the dialog asks for gets the keyboard once it is on screen.
                    let body = NodeRef::<leptos::html::Div>::new();
                    request_animation_frame(move || focus_first_autofocus(body));
                    view! {
                        <div
                            class="backdrop"
                            on:click=move |ev| {
                                use wasm_bindgen::JsCast;
                                let on_backdrop = ev
                                    .target()
                                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                    .map(|el| el.class_list().contains("backdrop"))
                                    .unwrap_or(false);
                                if on_backdrop && !locked {
                                    close();
                                }
                            }
                        >
                            <div class="dialog" class:dialog--wide=wide class:dialog--full=full role="dialog" aria-modal="true">
                                <div class="dialog__head">
                                    {icon.get_value().map(|name| view! {
                                        <div class=tone.icon_class()><Icon name=name /></div>
                                    })}
                                    <div>
                                        <h3 class="dialog__title">{move || title.get()}</h3>
                                        {move || subtitle.get().map(|s| view! { <p class="dialog__subtitle">{s}</p> })}
                                    </div>
                                    {(!locked).then(|| view! {
                                        <div class="dialog__close">
                                            <Button variant=Variant::Ghost size=Size::Sm icon="close" on_click=move |_| close() />
                                        </div>
                                    })}
                                </div>
                                <div class="dialog__body" node_ref=body>{children.with_value(|c| c())}</div>
                                {footer.with_value(|f| f.as_ref().map(|f| {
                                    let f = f.clone();
                                    view! {
                                        <div class="dialog__foot">
                                            <span class="dialog__foot-hint">{move || footer_hint.get()}</span>
                                            {f()}
                                        </div>
                                    }
                                }))}
                            </div>
                        </div>
                    }
                }}
            </Portal>
        </Show>
    }
}

#[component]
pub fn ConfirmDialog(
    open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(into)] message: Signal<String>,
    #[prop(into)] confirm_label: Signal<String>,
    #[prop(into)] cancel_label: Signal<String>,
    #[prop(optional)] danger: bool,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let tone = if danger { DialogTone::Danger } else { DialogTone::Warning };
    let icon = if danger { "delete_outline" } else { "help_outline" };
    view! {
        <Dialog open=open title=title icon=icon tone=tone>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || cancel_label.get()}</Button>
                <Button
                    variant=if danger { Variant::Danger } else { Variant::Primary }
                    on_click=move |_| {
                        open.set(false);
                        on_confirm.run(());
                    }
                >
                    {move || confirm_label.get()}
                </Button>
            </DialogFooter>
            <p style="margin:0">{move || message.get()}</p>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_full_dialog_fills_the_window_and_lets_its_body_flex() {
        let css = include_str!("../../styles/components.css");
        assert!(css.contains(".dialog--full { width: 1100px; height: calc(100vh - 48px); }"));
        assert!(css.contains(
            ".dialog--full .dialog__body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }"
        ));
    }
}
