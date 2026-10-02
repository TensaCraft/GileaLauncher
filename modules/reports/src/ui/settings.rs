//! The module's own section of the launcher's settings, «Звіти»: all it has to set (for now, the
//! contact reports carry).

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Section, SettingRow, TextInput, Variant, use_toasts};

use super::api;

#[component]
pub fn ReportsSettings() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let contact = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(settings) = api::settings().await {
            let _ = contact.try_set(settings.contact);
        }
    });
    let save = move || {
        saving.set(true);
        let settings = api::Settings { contact: contact.get_untracked().trim().to_string() };
        spawn_local(async move {
            match api::set_settings(&settings).await {
                Ok(_) => {
                    let _ = contact.try_set(settings.contact);
                    toasts.show(Level::Success, i18n.t("settings_saved"), None);
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
            let _ = saving.try_set(false);
        });
    };
    view! {
        <Section icon="bug_report" title=t("settings_tab_reports") desc=t("reports_settings_desc")>
            <SettingRow title=t("report_contact_label") desc=t("report_contact_hint")>
                <div class="wrap" style="flex-wrap:nowrap;gap:8px">
                    <TextInput value=contact placeholder=t("report_contact_hint") on_enter=Callback::new(move |_| save()) />
                    <Button
                        variant=Variant::Primary
                        icon="save"
                        loading=Signal::derive(move || saving.get())
                        on_click=move |_| save()
                    >
                        {move || i18n.t("save")}
                    </Button>
                </div>
            </SettingRow>
        </Section>
    }
}
