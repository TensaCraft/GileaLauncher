//! The Delete tab: a danger zone that removes the build — with its game
//! folder while the switch is on (the default) — and whatever modules offer to remove with it
//! (world backups), each off by default.

use launcher_shared::{BuildDto, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::module::DeleteOption;
use ui_kit::{Button, ConfirmDialog, Section, SettingRow, Switch, Variant, ipc, use_toasts};

use crate::builds::actions::use_build_actions;
use crate::modules::use_module_parts;

#[component]
pub fn DeletePanel(current: Signal<Option<BuildDto>>) -> impl IntoView {
    let build = current;
    let i18n = use_i18n();
    let toasts = use_toasts();
    let actions = use_build_actions();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let delete_files = RwSignal::new(true);
    let confirm_open = RwSignal::new(false);
    let options: Vec<(DeleteOption, RwSignal<bool>)> = use_module_parts()
        .get_untracked()
        .delete_options
        .into_iter()
        .map(|option| (option, RwSignal::new(false)))
        .collect();
    let chosen = StoredValue::new(options.clone());
    let confirm = Callback::new(move |()| {
        confirm_open.set(false);
        let Some(b) = build.get_untracked() else { return };
        let files = delete_files.get_untracked();
        let picked: Vec<DeleteOption> = chosen
            .with_value(|o| o.iter().filter(|(_, on)| on.get_untracked()).map(|(opt, _)| *opt).collect());
        spawn_local(async move {
            // A module's part goes first: when it fails, the build stays for another try.
            for option in picked {
                let args = serde_json::json!({ "key": b.key });
                if let Err(e) =
                    ipc::module_invoke::<_, serde_json::Value>(option.module, option.command, &args).await
                {
                    toasts.show(Level::Error, i18n.error(&e), None);
                    return;
                }
            }
            actions.delete(b.key, files);
        });
    });
    let message = Signal::derive(move || {
        let name = build.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default());
        i18n.tp("confirm_delete_version", &[("version", name)])
    });
    view! {
        <Section icon="warning" danger=true title=t("version_delete_warning_title") desc=t("version_delete_warning_desc")>
            <SettingRow title=t("version_delete_directory") desc=t("version_delete_directory_desc")>
                <Switch checked=delete_files label=t("version_delete_directory") />
            </SettingRow>
            {options
                .into_iter()
                .map(|(option, on)| {
                    view! {
                        <SettingRow title=t(option.label) desc=t(option.desc)>
                            <Switch checked=on label=t(option.label) />
                        </SettingRow>
                    }
                })
                .collect_view()}
            <div class="content__danger">
                <Button variant=Variant::Danger icon="delete" on_click=move |_| confirm_open.set(true)>
                    {move || i18n.t("delete_version_action")}
                </Button>
            </div>
        </Section>
        <ConfirmDialog
            open=confirm_open
            danger=true
            title=t("confirmation")
            message=message
            confirm_label=t("delete_version_action")
            cancel_label=t("cancel")
            on_confirm=confirm
        />
    }
}
