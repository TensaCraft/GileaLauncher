//! Profile commands. Create during component setup (`use_profile_actions`), call from handlers.

use launcher_shared::{AppError, ErrorCode, Level, ProfileDto, ProfilesSnapshot};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Toasts, ipc, use_toasts};

use crate::store::{AppStore, use_store};

#[derive(serde::Serialize)]
struct NameArgs {
    name: String,
}

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[derive(Clone, Copy)]
pub struct ProfileActions {
    store: AppStore,
    toasts: Toasts,
    i18n: I18nCtx,
}

impl ProfileActions {
    /// `done` gets `None` on success, or the error to show under the nickname field.
    pub fn create_offline(&self, name: String, done: Callback<Option<AppError>>) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            match ipc::invoke::<_, ProfilesSnapshot>("profile_create_offline", &NameArgs { name }).await {
                Ok(snapshot) => {
                    store.profiles.set(snapshot.profiles);
                    toasts.show(Level::Success, i18n.t("profile_created"), None);
                    done.try_run(None);
                }
                Err(e) => {
                    done.try_run(Some(e));
                }
            }
        });
    }

    pub fn delete(&self, key: String) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            match ipc::invoke::<_, ProfilesSnapshot>("profile_delete", &KeyArgs { key }).await {
                Ok(snapshot) => {
                    store.profiles.set(snapshot.profiles);
                    toasts.show(Level::Success, i18n.t("profile_deleted"), None);
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    }

    pub fn set_default(&self, profile: ProfileDto) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            let args = KeyArgs { key: profile.key.clone() };
            match ipc::invoke::<_, ProfilesSnapshot>("profile_set_default", &args).await {
                Ok(snapshot) => {
                    store.profiles.set(snapshot.profiles);
                    let text = i18n.tp("profile_set_as_default", &[("profile", profile.name)]);
                    toasts.show(Level::Info, text, None);
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    }

    /// Microsoft sign-in in the browser; a device code dialog appears when that is impossible.
    pub fn sign_in(&self) {
        let Self { toasts, i18n, .. } = *self;
        toasts.show(Level::Info, i18n.t("microsoft_auth_starting"), None);
        spawn_local(async move {
            match ipc::call::<ProfileDto>("auth_sign_in").await {
                Ok(_) => toasts.show(Level::Success, i18n.t("profile_created"), None),
                Err(e) if e.code == ErrorCode::Cancelled => {
                    toasts.show(Level::Info, i18n.t("microsoft_auth_cancelled"), None)
                }
                Err(e) if e.code == ErrorCode::Busy => {}
                Err(e) => crate::shell::failure::failure_toast(toasts, i18n, i18n.error(&e), &e),
            }
        });
    }

    pub fn cancel_sign_in(&self) {
        spawn_local(async move {
            let _ = ipc::call::<()>("auth_cancel").await;
        });
    }
}

pub fn use_profile_actions() -> ProfileActions {
    ProfileActions { store: use_store(), toasts: use_toasts(), i18n: use_i18n() }
}
