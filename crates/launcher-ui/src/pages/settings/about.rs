use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Section, SettingRow, Tag, TagTone, Variant, ipc, use_toasts};

use super::logs::{self, LogViewer};
use crate::shell::{has_contacts, use_support};
use crate::store::use_store;

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

fn open_path(path: String) {
    spawn_local(async move {
        let _ = ipc::invoke::<_, ()>("open_path", &PathArgs { path }).await;
    });
}

#[component]
pub fn AboutSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let help = use_support();
    let contacts = has_contacts();
    let toasts = use_toasts();
    let logs_open = RwSignal::new(false);
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    view! {
        {move || store.info.get().map(|info| {
            let modules = info.modules.clone();
            let paths = [
                ("about_path_state", info.paths.app_state_dir.clone()),
                ("about_path_minecraft", info.paths.minecraft_dir.clone()),
            ];
            let log_file = info.paths.log_file.clone();
            let for_reveal = log_file.clone();
            view! {
                <div class="about">
                    <img class="about__logo" src="/img/app-icon.png" alt="" />
                    <div>
                        // The one place the launcher names itself (owner, 2026-10-02).
                        <div class="about__name">{launcher_shared::branding::APP_NAME}</div>
                        <div class="about__meta">
                            {i18n.tp("about_version", &[("version", info.version.clone())])}
                            " · "
                            {i18n.tp("about_profile", &[("profile", info.profile.clone())])}
                        </div>
                        {info.update_source.clone().map(|url| view! {
                            <div class="about__meta">{i18n.tp("update_source_test_server", &[("url", url)])}</div>
                        })}
                        {info.dev_mode.then(|| view! { <Tag tone=TagTone::Snapshot>{i18n.t("about_dev_mode")}</Tag> })}
                    </div>
                    {move || contacts.get().then(|| view! {
                        <div style="margin-left:auto">
                            <Button icon="support_agent" outlined=true on_click=move |_| help.open()>{i18n.t("support_title")}</Button>
                        </div>
                    })}
                </div>
                <Section icon="extension" title=t("about_modules")>
                    {if modules.is_empty() {
                        view! { <div class="activity__empty">{i18n.t("about_modules_none")}</div> }.into_any()
                    } else {
                        modules.into_iter().map(|m| view! {
                            <SettingRow
                                title=Signal::derive({ let id = m.id.clone(); move || i18n.t(&format!("module_{id}_name")) })
                                desc=Signal::derive({ let id = m.id.clone(); move || i18n.t(&format!("module_{id}_desc")) })
                            >
                                <Tag tone=TagTone::Neutral>{format!("v{}", m.version)}</Tag>
                            </SettingRow>
                        }).collect_view().into_any()
                    }}
                </Section>
                <Section icon="folder_open" title=t("about_paths")>
                    {paths.into_iter().map(|(key, path)| {
                        let shown = path.clone();
                        view! {
                            <SettingRow title=t(key) desc=shown>
                                <Button variant=Variant::Secondary icon="folder_open" outlined=true on_click=move |_| open_path(path.clone())>
                                    {move || i18n.t("about_open_folder")}
                                </Button>
                            </SettingRow>
                        }
                    }).collect_view()}
                    <SettingRow title=t("about_path_logs") desc=log_file>
                        <Button variant=Variant::Secondary icon="receipt_long" outlined=true on_click=move |_| logs_open.set(true)>
                            {move || i18n.t("log_view")}
                        </Button>
                        <Button
                            variant=Variant::Secondary
                            icon="folder_open"
                            outlined=true
                            on_click=move |_| logs::reveal(for_reveal.clone(), move |e| toasts.show(Level::Error, i18n.error(&e), None))
                        >
                            {move || i18n.t("log_reveal")}
                        </Button>
                    </SettingRow>
                </Section>
                <div class="activity__meta" style="text-align:center">{i18n.t("about_license")}</div>
            }
        })}
        <LogViewer open=logs_open />
    }
}
