use std::time::Duration;

use launcher_shared::{AppError, Level, SettingUpdate, SetupPlan, SetupPreview};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Field, Icon, PathField, Section, Select, SelectOption, SettingRow, Variant, ipc, use_toasts,
};

use crate::shell::PageHeader;
use crate::store::{use_settings_writer, use_store};

#[derive(serde::Serialize)]
struct PlanArgs {
    plan: SetupPlan,
}

#[derive(serde::Serialize)]
struct StartArgs {
    start: Option<String>,
}

#[derive(serde::Serialize)]
struct DirArgs {
    dir: String,
}

#[component]
pub fn SetupPage() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let toasts = use_toasts();
    let writer = use_settings_writer();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    let dir = RwSignal::new(String::new());
    let lang = RwSignal::new(store.settings.get_untracked().lang);
    let error = RwSignal::new(None::<AppError>);
    let saving = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(s) = store.setup.get()
            && dir.get_untracked().is_empty()
        {
            dir.set(s.app_state_dir.clone());
        }
    });
    Effect::new(move |_| lang.set(store.settings.get().lang));

    let issues = move || store.setup.get().map(|s| s.issues).unwrap_or_default();
    let languages = Signal::derive(move || {
        vec![
            SelectOption::new("uk_UA", i18n.t("language_uk")),
            SelectOption::new("en_US", i18n.t("language_en")),
        ]
    });
    // The backend computes the real folders (rebasing, per-OS Minecraft dir); latest request wins.
    let preview = RwSignal::new(None::<SetupPreview>);
    let generation = StoredValue::new(0u64);
    Effect::new(move |_| {
        let current = dir.get();
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        if current.trim().is_empty() {
            preview.set(None);
            return;
        }
        spawn_local(async move {
            let result = ipc::invoke::<_, SetupPreview>("setup_preview", &DirArgs { dir: current }).await;
            // The page may be gone by the answer.
            if generation.try_get_value() == Some(mine) {
                let _ = preview.try_set(result.ok());
            }
        });
    });
    let minecraft = move || preview.get().map(|p| p.minecraft_dir).unwrap_or_default();
    let backups = move || preview.get().map(|p| p.backups_dir).unwrap_or_default();

    let browse = Callback::new(move |_| {
        let start = Some(dir.get_untracked());
        spawn_local(async move {
            if let Ok(Some(picked)) =
                ipc::invoke::<_, Option<String>>("pick_directory", &StartArgs { start }).await
            {
                let _ = dir.try_set(picked);
                let _ = error.try_set(None);
            }
        });
    });
    let use_defaults = move |_: ()| {
        if let Some(s) = store.setup.get_untracked() {
            dir.set(s.default_app_state_dir);
            error.set(None);
        }
    };
    let save = move |_: ()| {
        saving.set(true);
        error.set(None);
        let plan = SetupPlan { lang: lang.get_untracked(), app_state_dir: dir.get_untracked() };
        spawn_local(async move {
            match ipc::invoke::<_, bool>("setup_apply", &PlanArgs { plan }).await {
                Ok(true) => {
                    toasts.show(
                        Level::Success,
                        i18n.t("setup_wizard_saved"),
                        Some(i18n.t("storage_dir_saved_restart")),
                    );
                    set_timeout(
                        || {
                            spawn_local(async {
                                let _ = ipc::call::<()>("app_restart").await;
                            })
                        },
                        Duration::from_millis(1200),
                    );
                }
                Ok(false) => {
                    store.setup.update(|s| {
                        if let Some(s) = s {
                            s.should_open = false;
                            s.issues.clear();
                        }
                    });
                    toasts.show(Level::Success, i18n.t("setup_wizard_saved"), None);
                    store.go("/");
                }
                Err(e) => {
                    let _ = error.try_set(Some(e));
                }
            }
            let _ = saving.try_set(false);
        });
    };

    view! {
        <PageHeader title_key="setup_wizard_title" />
        <div class="setup">
            <Section icon="rocket_launch" title=t("setup_wizard_title") desc=t("setup_wizard_desc")>
                {move || {
                    let list = issues();
                    if list.is_empty() {
                        view! {
                            <div class="setup__status is-ok">
                                <Icon name="check_circle_outline" />
                                <span>{i18n.t("setup_wizard_storage_ready")}</span>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="setup__status is-error">
                                <Icon name="warning_amber" />
                                <div>
                                    <b>{i18n.t("setup_wizard_storage_issue")}</b>
                                    <ul>{list.iter().map(|issue| view! { <li>{i18n.text(issue)}</li> }).collect_view()}</ul>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
                <SettingRow title=t("language")>
                    <div style="width:220px">
                        <Select
                            options=languages
                            value=lang
                            icon="translate"
                            on_change=Callback::new(move |l: String| writer.apply(SettingUpdate::Lang(l)))
                        />
                    </div>
                </SettingRow>
                <SettingRow title=t("setup_wizard_launcher_data") stacked=true>
                    <Field
                        label=t("select_directory")
                        error=Signal::derive(move || error.get().map(|e| i18n.error(&e)))
                    >
                        <PathField value=dir browse_label=t("browse_directory") on_browse=browse invalid=Signal::derive(move || error.get().is_some()) />
                    </Field>
                </SettingRow>
                <SettingRow title=t("setup_wizard_derived") stacked=true>
                    <div class="setup__paths">
                        <div><span>{move || i18n.t("setup_wizard_minecraft_dir")}</span><code>{minecraft}</code></div>
                        <div><span>{move || i18n.t("setup_wizard_backups_dir")}</span><code>{backups}</code></div>
                    </div>
                </SettingRow>
                <div class="setup__actions">
                    <Button variant=Variant::Ghost icon="restart_alt" on_click=use_defaults>{move || i18n.t("setup_wizard_use_defaults")}</Button>
                    <Button variant=Variant::Primary icon="check" loading=Signal::derive(move || saving.get()) on_click=save>
                        {move || i18n.t("setup_wizard_save")}
                    </Button>
                </div>
            </Section>
        </div>
    }
}
