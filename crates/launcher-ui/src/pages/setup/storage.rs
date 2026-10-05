//! The wizard's data-folder step: where the launcher keeps its data, the folders that follow from
//! it, and why a folder cannot be used, all checked by the backend as the folder is typed.

use std::time::Duration;

use launcher_shared::SetupPreview;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Field, Icon, PathField, SettingRow, Variant, ipc};

use super::{Wizard, use_wizard};
use crate::store::use_store;

/// How long typing in the folder field pauses before the folders it gives are asked for.
const PREVIEW_PAUSE: Duration = Duration::from_millis(300);

#[derive(serde::Serialize)]
struct StartArgs {
    start: Option<String>,
}

#[derive(serde::Serialize)]
struct DirArgs {
    dir: String,
}

/// Fills in the folder in use and keeps what it gives up to date (latest request wins; typing asks
/// after a pause, the first folder at once). Runs for the whole wizard: its last step needs the
/// folder even when this step was never opened.
pub fn watch_folder(wizard: Wizard) {
    let store = use_store();
    Effect::new(move |_| {
        if let Some(s) = store.setup.get()
            && wizard.dir.get_untracked().is_empty()
        {
            wizard.dir.set(s.app_state_dir.clone());
        }
    });
    let generation = StoredValue::new(0u64);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let cancel_timer = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    Effect::new(move |asked: Option<()>| {
        let current = wizard.dir.get();
        if asked.is_some() {
            // Another folder: what saving the last one said is no longer about it.
            wizard.error.set(None);
        }
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        cancel_timer();
        if current.trim().is_empty() {
            wizard.preview.set(None);
            wizard.preview_error.set(None);
            wizard.checked.set(None);
            return;
        }
        let ask = move || {
            spawn_local(async move {
                let result =
                    ipc::invoke::<_, SetupPreview>("setup_preview", &DirArgs { dir: current.clone() }).await;
                // The wizard may be gone by the answer.
                if generation.try_get_value() == Some(mine) {
                    let (preview, error) = match result {
                        Ok(preview) => (Some(preview), None),
                        Err(e) => (None, Some(e)),
                    };
                    let _ = wizard.preview.try_set(preview);
                    let _ = wizard.preview_error.try_set(error);
                    let _ = wizard.checked.try_set(Some(current));
                }
            })
        };
        match asked {
            None => ask(),
            Some(()) => timer.set_value(set_timeout_with_handle(ask, PREVIEW_PAUSE).ok()),
        }
    });
    on_cleanup(cancel_timer);
}

#[component]
pub fn StorageStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    // The folder's own problem first (or why it could not be checked), then what saving said.
    let problem = move || {
        if let Some(issue) = wizard.preview.with(|p| p.as_ref().and_then(|p| p.issue.clone())) {
            return Some(i18n.text(&issue));
        }
        if let Some(e) = wizard.preview_error.get().or_else(|| wizard.error.get()) {
            return Some(i18n.error(&e));
        }
        None
    };
    // What the start found stays said while the folder in use is kept: the wizard opened for it,
    // and some of it (a Minecraft folder on a missing drive) is not about this folder.
    let started_with = move || {
        let kept = store.setup.with(|s| {
            s.as_ref().is_some_and(|s| {
                wizard.preview.with(|p| p.as_ref().is_some_and(|p| p.app_state_dir == s.app_state_dir))
            })
        });
        if kept {
            store.setup.with(|s| s.as_ref().map(|s| s.issues.clone()).unwrap_or_default())
        } else {
            Vec::new()
        }
    };
    let minecraft =
        move || wizard.preview.with(|p| p.as_ref().map(|p| p.minecraft_dir.clone()).unwrap_or_default());
    let backups =
        move || wizard.preview.with(|p| p.as_ref().map(|p| p.backups_dir.clone()).unwrap_or_default());

    let browse = Callback::new(move |_| {
        let start = Some(wizard.dir.get_untracked());
        spawn_local(async move {
            if let Ok(Some(picked)) =
                ipc::invoke::<_, Option<String>>("pick_directory", &StartArgs { start }).await
            {
                let _ = wizard.dir.try_set(picked);
                let _ = wizard.error.try_set(None);
            }
        });
    });
    let use_defaults = move |_: ()| {
        if let Some(s) = store.setup.get_untracked() {
            wizard.dir.set(s.default_app_state_dir);
            wizard.error.set(None);
        }
    };

    view! {
        <div class="wizard-storage">
            {move || match problem() {
                Some(text) => view! {
                    <div class="wizard__status is-error">
                        <Icon name="warning_amber" />
                        <span>{text}</span>
                    </div>
                }.into_any(),
                None => {
                    let list = started_with();
                    if !list.is_empty() {
                        view! {
                            <div class="wizard__status is-error">
                                <Icon name="warning_amber" />
                                <div>
                                    <b>{i18n.t("setup_wizard_storage_issue")}</b>
                                    <ul>{list.iter().map(|issue| view! { <li>{i18n.text(issue)}</li> }).collect_view()}</ul>
                                </div>
                            </div>
                        }.into_any()
                    } else if wizard.folder_ready() {
                        view! {
                            <div class="wizard__status is-ok">
                                <Icon name="check_circle_outline" />
                                <span>{i18n.t("setup_wizard_storage_ready")}</span>
                            </div>
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                }
            }}
            <Field label=t("setup_wizard_launcher_data")>
                <PathField
                    value=wizard.dir
                    browse_label=t("browse_directory")
                    on_browse=browse
                    invalid=Signal::derive(move || problem().is_some())
                />
            </Field>
            <SettingRow title=t("setup_wizard_derived") stacked=true>
                <div class="wizard__paths">
                    <div><span>{move || i18n.t("setup_wizard_minecraft_dir")}</span><code>{minecraft}</code></div>
                    <div><span>{move || i18n.t("setup_wizard_backups_dir")}</span><code>{backups}</code></div>
                </div>
            </SettingRow>
            <div class="wizard-storage__more">
                <span>{move || i18n.t("setup_storage_later")}</span>
                <Button variant=Variant::Ghost icon="restart_alt" on_click=use_defaults>
                    {move || i18n.t("setup_wizard_use_defaults")}
                </Button>
            </div>
        </div>
    }
}
