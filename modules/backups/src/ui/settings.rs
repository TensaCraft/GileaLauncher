//! The Backups section of the launcher's settings (the original's settings → Backups): backups
//! before a launch on or off, how many automatic ones a world keeps, and where they go.

use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Field, PathField, Section, SettingRow, Switch, TextInput, Variant, ipc, use_toasts};

use super::api;
use crate::dto::BackupSettings;

#[derive(serde::Serialize)]
struct StartArgs {
    start: Option<String>,
}

/// A count the user typed, when it is a whole number of at least one.
pub fn keep_of(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok().filter(|n| *n >= 1)
}

#[component]
pub fn BackupsSettings() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let saved = RwSignal::new(None::<BackupSettings>);
    let enabled = RwSignal::new(false);
    let keep = RwSignal::new(String::new());
    let dir = RwSignal::new(String::new());
    let error = RwSignal::new(None::<AppError>);
    let saving = RwSignal::new(false);
    // Unread, the section says why and its controls wait (they would save over what is unknown).
    spawn_local(async move {
        match api::settings().await {
            Ok(s) => {
                let _ = enabled.try_set(s.enabled);
                let _ = keep.try_set(s.keep.to_string());
                let _ = dir.try_set(s.dir.clone());
                let _ = saved.try_set(Some(s));
            }
            Err(e) => {
                let _ = error.try_set(Some(e));
            }
        }
    });
    let unread = Signal::derive(move || saved.with(Option::is_none));
    let keep_invalid = Signal::derive(move || keep.with(|k| !k.is_empty() && keep_of(k).is_none()));
    let store = move |wanted: BackupSettings| {
        saving.set(true);
        spawn_local(async move {
            match api::set_settings(&wanted).await {
                Ok(s) => {
                    let _ = dir.try_set(s.dir.clone());
                    let _ = saved.try_set(Some(s));
                    let _ = error.try_set(None);
                    toasts.show(Level::Success, i18n.t("settings_saved"), None);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e));
                }
            }
            let _ = saving.try_set(false);
        });
    };
    // The switch applies at once, with what was saved for the rest.
    let toggle = Callback::new(move |on: bool| {
        if let Some(s) = saved.get_untracked() {
            store(BackupSettings { enabled: on, ..s });
        }
    });
    let save = move |use_default: bool| {
        let Some(s) = saved.get_untracked() else { return };
        let Some(count) = keep_of(&keep.get_untracked()) else { return };
        let folder = if use_default { s.default_dir.clone() } else { dir.get_untracked() };
        store(BackupSettings { enabled: enabled.get_untracked(), keep: count, dir: folder, ..s });
    };
    let browse = Callback::new(move |_| {
        spawn_local(async move {
            let start = Some(dir.get_untracked());
            if let Ok(Some(picked)) =
                ipc::invoke::<_, Option<String>>("pick_directory", &StartArgs { start }).await
            {
                let _ = dir.try_set(picked);
                let _ = error.try_set(None);
            }
        });
    });

    view! {
        <Section icon="backup" title=t("world_backups") desc=t("world_backups_desc")>
            <SettingRow title=t("world_backups_enabled")>
                <Switch checked=enabled on_change=toggle disabled=unread label=t("world_backups_enabled") />
            </SettingRow>
            <SettingRow title=t("world_backups_keep_count")>
                <TextInput value=keep invalid=keep_invalid />
            </SettingRow>
            <SettingRow title=t("world_backups_dir") stacked=true>
                <Field
                    label=t("select_directory")
                    error=Signal::derive(move || {
                        if keep_invalid.get() {
                            Some(i18n.t("world_backups_keep_count_invalid"))
                        } else {
                            error.get().map(|e| i18n.error(&e))
                        }
                    })
                >
                    <PathField value=dir browse_label=t("browse_directory") on_browse=browse invalid=Signal::derive(move || error.get().is_some()) />
                </Field>
                <div class="wrap" style="justify-content:flex-end;padding-bottom:12px">
                    <Button icon="restart_alt" disabled=unread on_click=move |_| save(true)>{move || i18n.t("setup_wizard_use_defaults")}</Button>
                    <Button
                        variant=Variant::Primary
                        icon="save"
                        loading=Signal::derive(move || saving.get())
                        disabled=Signal::derive(move || keep_invalid.get() || unread.get())
                        on_click=move |_| save(false)
                    >
                        {move || i18n.t("save")}
                    </Button>
                </div>
            </SettingRow>
        </Section>
    }
}

#[cfg(test)]
mod tests {
    use super::keep_of;

    #[test]
    fn a_kept_count_is_a_whole_number_of_at_least_one() {
        assert_eq!(keep_of(" 5 "), Some(5));
        assert_eq!(keep_of("0"), None);
        assert_eq!(keep_of("-1"), None);
        assert_eq!(keep_of("два"), None);
    }
}
