use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, DialogTone, Variant};

use super::{has_contacts, use_support};
use crate::modules::use_module_parts;
use crate::store::use_store;

/// Modal for backend warnings/errors (`app://alert`).
#[component]
pub fn AlertHost() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let help = use_support();
    let parts = use_module_parts();
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(store.alert.get().is_some()));
    let contacts = has_contacts();
    view! {
        <Dialog
            open=open
            title=Signal::derive(move || store.alert.get().map(|a| i18n.text(&a.title)).unwrap_or_default())
            icon="warning_amber"
            tone=DialogTone::Warning
            on_close=Callback::new(move |_| store.alert.set(None))
        >
            <DialogFooter slot>
                // Modules' buttons (a report) for an alert the user can report.
                {move || {
                    store.alert.get().filter(|a| a.allow_report).map(|alert| {
                        parts.with(|p| p.alert_actions.iter().map(|a| (a.view)(alert.clone())).collect_view())
                    })
                }}
                {move || contacts.get().then(|| view! {
                    <Button variant=Variant::Secondary icon="support_agent" outlined=true on_click=move |_| help.open()>
                        {move || i18n.t("support_title")}
                    </Button>
                })}
                <Button variant=Variant::Ghost on_click=move |_| store.alert.set(None)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <p style="margin:0">{move || store.alert.get().map(|a| i18n.text(&a.message)).unwrap_or_default()}</p>
        </Dialog>
    }
}
