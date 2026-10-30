//! The Delete tab: a danger zone that removes the build with its game folder and whatever modules
//! offer to remove with it (world backups), each while its switch is on (all on by default, as in
//! the Delete dialog: `builds::deletion`).

use launcher_shared::BuildDto;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, ConfirmDialog, Section, SettingRow, Switch, Variant, use_toasts};

use crate::builds::actions::use_build_actions;
use crate::builds::deletion::{DeleteChoices, delete_build};
use crate::modules::use_module_parts;

#[component]
pub fn DeletePanel(current: Signal<Option<BuildDto>>) -> impl IntoView {
    let build = current;
    let i18n = use_i18n();
    let toasts = use_toasts();
    let actions = use_build_actions();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let confirm_open = RwSignal::new(false);
    let choices = DeleteChoices::new();
    choices.reset(use_module_parts().get_untracked().delete_options);
    let confirm = Callback::new(move |()| {
        confirm_open.set(false);
        let Some(b) = build.get_untracked() else { return };
        delete_build(choices, b.key, actions, toasts, i18n);
    });
    let message = Signal::derive(move || {
        let name = build.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default());
        i18n.tp("confirm_delete_version", &[("version", name)])
    });
    view! {
        <Section icon="warning" danger=true title=t("version_delete_warning_title") desc=t("version_delete_warning_desc")>
            <SettingRow title=t("version_delete_directory") desc=t("version_delete_directory_desc")>
                <Switch checked=choices.files label=t("version_delete_directory") />
            </SettingRow>
            {move || {
                choices
                    .options
                    .get()
                    .into_iter()
                    .map(|(option, on)| {
                        view! {
                            <SettingRow title=t(option.label) desc=t(option.desc)>
                                <Switch checked=on label=t(option.label) />
                            </SettingRow>
                        }
                    })
                    .collect_view()
            }}
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
