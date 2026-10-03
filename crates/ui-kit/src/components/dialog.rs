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
    /// In place of the title's text: a heading of the caller's own (a name edited in place).
    #[prop(optional)]
    heading: Option<ChildrenFn>,
    /// A class of the caller's own beside `dialog` (a size of its own).
    #[prop(optional)]
    class: &'static str,
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
    let heading = StoredValue::new(heading);

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
                    // Where the press began: a selection dragged out of the dialog ends on the
                    // backdrop too.
                    let pressed = StoredValue::new(false);
                    view! {
                        <div
                            class="backdrop"
                            on:mousedown=move |ev| pressed.set_value(on_backdrop(&ev))
                            on:click=move |ev| {
                                if closes_on(pressed.get_value(), on_backdrop(&ev)) && !locked {
                                    close();
                                }
                            }
                        >
                            <div class=format!("dialog {class}") class:dialog--wide=wide class:dialog--full=full role="dialog" aria-modal="true">
                                <div class="dialog__head">
                                    {icon.get_value().map(|name| view! {
                                        <div class=tone.icon_class()><Icon name=name /></div>
                                    })}
                                    <div class="dialog__heading">
                                        {heading.with_value(|h| match h {
                                            Some(own) => own().into_any(),
                                            None => view! { <h3 class="dialog__title">{move || title.get()}</h3> }.into_any(),
                                        })}
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

/// The mouse event happened on the backdrop itself, not inside the dialog.
fn on_backdrop(ev: &web_sys::MouseEvent) -> bool {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| el.class_list().contains("backdrop"))
}

/// A click closes the dialog when it both began (`pressed`) and ended (`released`) on the backdrop.
fn closes_on(pressed: bool, released: bool) -> bool {
    pressed && released
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
    fn only_a_click_begun_and_ended_on_the_backdrop_closes() {
        // A text selection dragged out of the dialog ends on the backdrop: the browser sends a
        // click there, and the dialog (with what was typed) must stay.
        assert!(super::closes_on(true, true));
        assert!(!super::closes_on(false, true));
        assert!(!super::closes_on(true, false));
    }

    #[test]
    fn a_full_dialog_fills_the_window_below_its_buttons_and_lets_its_body_flex() {
        // The window's own buttons stay above every layer and end 46px down: the dialog starts
        // below them (top 60px with the backdrop's centring).
        let css = include_str!("../../styles/components.css");
        assert!(
            css.contains(".dialog--full { width: 1100px; height: calc(100vh - 88px); margin-top: 32px; }")
        );
        assert!(css.contains(
            ".dialog--full .dialog__body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }"
        ));
    }
}
