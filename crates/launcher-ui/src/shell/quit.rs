//! A close held while work is under way (the window's button, Alt+F4, Quit in the tray): the window
//! asks whether to quit anyway, naming what would be cut short.

use launcher_shared::{Text, names};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, DialogTone, Variant, ipc};

#[component]
pub fn QuitConfirm() -> impl IntoView {
    let i18n = use_i18n();
    let work = RwSignal::new(Vec::<Text>::new());
    let open = RwSignal::new(false);
    ipc::listen::<Vec<Text>>(names::QUIT_HELD, move |held| {
        work.set(held);
        open.set(true);
    });
    let named = move || work.with(|w| w.iter().map(|t| i18n.text(t)).collect::<Vec<_>>().join(", "));
    view! {
        <Dialog
            open=open
            title=Signal::derive(move || i18n.t("quit_held_title"))
            icon="warning_amber"
            tone=DialogTone::Warning
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Danger
                    on_click=move |_| {
                        open.set(false);
                        spawn_local(async {
                            let _ = ipc::call::<()>("app_quit").await;
                        });
                    }
                >
                    {move || i18n.t("quit_anyway")}
                </Button>
            </DialogFooter>
            <p style="margin:0">{move || i18n.tp("quit_held_text", &[("work", named())])}</p>
        </Dialog>
    }
}
