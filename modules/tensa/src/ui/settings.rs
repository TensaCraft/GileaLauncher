//! The module's own section of the launcher's settings, «Tensa»: all it has to set (for now,
//! whether Home offers the server builds not installed yet).

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ui_kit::i18n::use_i18n;
use ui_kit::{Section, SettingRow, Switch, ipc, use_toasts};

/// The module's `settings` (and `set_settings`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Settings {
    show: bool,
}

#[component]
pub fn ServerBuildsSettings() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let show = RwSignal::new(true);
    spawn_local(async move {
        if let Ok(s) = ipc::module_invoke::<_, Settings>(crate::ID, "settings", &Value::Null).await {
            let _ = show.try_set(s.show);
        }
    });
    let toggle = Callback::new(move |on: bool| {
        spawn_local(async move {
            match ipc::module_invoke::<_, Value>(crate::ID, "set_settings", &Settings { show: on }).await {
                Ok(_) => toasts.show(Level::Success, i18n.t("settings_saved"), None),
                Err(e) => {
                    let _ = show.try_set(!on);
                    toasts.show(Level::Error, i18n.error(&e), None);
                }
            }
        });
    });
    view! {
        <Section icon="rocket_launch" title=t("settings_tab_tensa") desc=t("tensa_settings_desc")>
            <SettingRow title=t("show_tensacraft_versions")>
                <Switch checked=show on_change=toggle label=t("show_tensacraft_versions") />
            </SettingRow>
        </Section>
    }
}
