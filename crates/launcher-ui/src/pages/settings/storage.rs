use std::time::Duration;

use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ConfirmDialog, Field, InFlight, PathField, Section, SettingRow, Variant, ipc, use_toasts,
};

use crate::store::use_store;

#[derive(serde::Serialize)]
struct StartArgs {
    start: Option<String>,
}

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

/// A folder path as compared here: trimmed, `/`-separated, without a trailing `/`.
fn spelled(path: &str) -> String {
    path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

/// Saving `chosen` (empty: the default) moves the launcher from `current` to another folder, so
/// it restarts.
pub fn restart_needed(current: &str, chosen: &str, default: &str) -> bool {
    let chosen = if chosen.trim().is_empty() { default } else { chosen };
    spelled(current) != spelled(chosen)
}

#[component]
pub fn StorageSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let dir = RwSignal::new(store.settings.get_untracked().minecraft_dir);
    let error = RwSignal::new(None::<AppError>);
    let saving = InFlight::new();
    let confirm_open = RwSignal::new(false);

    let browse = Callback::new(move |_| {
        let start = Some(dir.get_untracked());
        spawn_local(async move {
            if let Ok(Some(picked)) =
                ipc::invoke::<_, Option<String>>("pick_directory", &StartArgs { start }).await
            {
                // The section may be gone (another one chosen) by now.
                let _ = dir.try_set(picked);
                let _ = error.try_set(None);
            }
        });
    });
    // "Default paths" only fills the field, as the setup wizard does; saving is the player's.
    let use_defaults = move || {
        dir.set(store.settings.with_untracked(|s| s.default_minecraft_dir.clone()));
        error.set(None);
    };
    let save = move || {
        if !saving.start() {
            return;
        }
        let path = dir.get_untracked();
        spawn_local(async move {
            match ipc::invoke::<_, bool>("settings_save_minecraft_dir", &PathArgs { path }).await {
                Ok(true) => {
                    toasts.show(Level::Success, i18n.t("storage_dir_saved_restart"), None);
                    set_timeout(
                        || {
                            spawn_local(async {
                                let _ = ipc::call::<()>("app_restart").await;
                            })
                        },
                        Duration::from_millis(1200),
                    );
                }
                Ok(false) => toasts.show(Level::Success, i18n.t("settings_saved"), None),
                Err(e) => {
                    let _ = error.try_set(Some(e));
                }
            }
            saving.done();
        });
    };
    // Another folder restarts the launcher: the player says so first.
    let ask_save = move || {
        let needed = store.settings.with_untracked(|s| {
            restart_needed(&s.minecraft_dir, &dir.get_untracked(), &s.default_minecraft_dir)
        });
        if needed { confirm_open.set(true) } else { save() }
    };

    view! {
        <Section icon="folder" title=t("minecraft_storage") desc=t("minecraft_storage_desc")>
            <SettingRow title=t("minecraft_game_dir_label") stacked=true>
                <Field
                    label=t("select_directory")
                    hint=t("minecraft_storage_hint")
                    error=Signal::derive(move || error.get().map(|e| i18n.error(&e)))
                >
                    <PathField value=dir browse_label=t("browse_directory") on_browse=browse invalid=Signal::derive(move || error.get().is_some()) />
                </Field>
                <div class="wrap" style="justify-content:flex-end;padding-bottom:12px">
                    <Button icon="restart_alt" on_click=move |_| use_defaults()>{move || i18n.t("setup_wizard_use_defaults")}</Button>
                    <Button variant=Variant::Primary icon="save" loading=saving.running() on_click=move |_| ask_save()>{move || i18n.t("save")}</Button>
                </div>
            </SettingRow>
        </Section>
        <ConfirmDialog
            open=confirm_open
            title=t("confirmation")
            message=t("storage_dir_restart_confirm")
            confirm_label=t("restart_now")
            cancel_label=t("cancel")
            on_confirm=Callback::new(move |()| save())
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_minecraft_folder_needs_a_restart() {
        assert!(restart_needed("C:/Launcher/minecraft", "D:/Games/MC", "C:/Launcher/minecraft"));
        assert!(restart_needed("D:/Games/MC", "", "C:/Launcher/minecraft"), "back to the default");
        assert!(restart_needed("D:/Games/MC", "C:/Launcher/minecraft", "C:/Launcher/minecraft"));
    }

    #[test]
    fn the_same_folder_needs_none() {
        assert!(!restart_needed(r"C:\Launcher\minecraft", " C:/Launcher/minecraft/ ", "C:/other"));
        assert!(!restart_needed("C:/Launcher/minecraft", "", "C:/Launcher/minecraft"), "the default kept");
    }
}
