//! IPC actions on builds. Create during component setup (`use_build_actions`), call from handlers.

use launcher_shared::{BuildDto, BuildsSnapshot, ErrorCode, Level, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::reorder::Reorder;
use ui_kit::{Toasts, ipc, use_toasts};

use crate::store::{AppStore, use_store};

#[derive(serde::Serialize)]
struct ReorderArgs {
    keys: Vec<String>,
}

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[derive(serde::Serialize)]
struct CreateArgs {
    name: String,
    version: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateLoaderArgs {
    name: String,
    loader: LoaderKind,
    mc: String,
    loader_version: String,
}

#[derive(serde::Serialize)]
struct CopyArgs {
    key: String,
    name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteArgs {
    key: String,
    delete_files: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchArgs {
    key: String,
    profile_key: Option<String>,
    allow_duplicate: bool,
}

/// Reports the outcome of a finished call; nothing happens when the page that asked is gone.
fn finish(done: Callback<bool>, ok: bool) {
    let _ = done.try_run(ok);
}

/// Success toasts come from the backend; failures are shown here (`op.fail` only records them).
#[derive(Clone, Copy)]
pub struct BuildActions {
    store: AppStore,
    toasts: Toasts,
    i18n: I18nCtx,
}

impl BuildActions {
    fn warn(&self, text: String) {
        self.toasts.show(Level::Error, text, None);
    }

    /// Reloads the list (game events change `running` without sending it).
    pub fn refresh(&self) {
        let store = self.store;
        spawn_local(async move {
            if let Ok(snapshot) = ipc::call::<BuildsSnapshot>("builds_list").await {
                store.builds.set(snapshot.builds);
                store.builds_loaded.set(true);
            }
        });
    }

    /// Drops the dragged build where it landed: the list takes the new order at once and the
    /// backend keeps it; a failure says so and lists the builds again.
    pub fn reorder(&self, drag: Reorder) {
        let order: Vec<String> =
            self.store.builds.with_untracked(|b| b.iter().map(|b| b.key.clone()).collect());
        let Some(keys) = drag.drop_on(&order) else { return };
        self.store.builds.update(|builds| {
            builds.sort_by_key(|b| keys.iter().position(|k| *k == b.key).unwrap_or(usize::MAX))
        });
        let this = *self;
        spawn_local(async move {
            match ipc::invoke::<_, BuildsSnapshot>("builds_reorder", &ReorderArgs { keys }).await {
                Ok(snapshot) => this.store.builds.set(snapshot.builds),
                Err(e) => {
                    this.warn(this.i18n.error(&e));
                    this.refresh();
                }
            }
        });
    }

    /// Installs Minecraft `version` as build `name`; `done(true)` once it is registered.
    pub fn create_vanilla(&self, name: String, version: String, done: Callback<bool>) {
        let this = *self;
        spawn_local(async move {
            let args = CreateArgs { name: name.clone(), version };
            match ipc::invoke::<_, BuildDto>("build_create_vanilla", &args).await {
                Ok(_) => finish(done, true),
                Err(e) => {
                    let error = this.i18n.error(&e);
                    this.warn(this.i18n.tp(
                        "version_install_error",
                        &[("client", "Minecraft".into()), ("version", name), ("error", error)],
                    ));
                    finish(done, false);
                }
            }
        });
    }

    /// Installs a loader build `name`; `done(true)` once it is registered.
    pub fn create_loader(
        &self,
        name: String,
        kind: LoaderKind,
        mc: String,
        loader_version: String,
        done: Callback<bool>,
    ) {
        let this = *self;
        spawn_local(async move {
            let args = CreateLoaderArgs { name: name.clone(), loader: kind, mc, loader_version };
            match ipc::invoke::<_, BuildDto>("build_create_loader", &args).await {
                Ok(_) => finish(done, true),
                Err(e) => {
                    let error = this.i18n.error(&e);
                    this.warn(this.i18n.tp(
                        "version_install_error",
                        &[("client", kind.display_name().into()), ("version", name), ("error", error)],
                    ));
                    finish(done, false);
                }
            }
        });
    }

    pub fn copy(&self, key: String, name: String, done: Callback<bool>) {
        let this = *self;
        spawn_local(async move {
            match ipc::invoke::<_, BuildDto>("build_copy", &CopyArgs { key, name: name.clone() }).await {
                Ok(_) => finish(done, true),
                Err(e) => {
                    let error = this.i18n.error(&e);
                    this.warn(this.i18n.tp("version_copy_error", &[("version", name), ("error", error)]));
                    finish(done, false);
                }
            }
        });
    }

    pub fn delete(&self, key: String, delete_files: bool) {
        let this = *self;
        spawn_local(async move {
            match ipc::invoke::<_, BuildsSnapshot>("build_delete", &DeleteArgs { key, delete_files }).await {
                Ok(snapshot) => this.store.builds.set(snapshot.builds),
                Err(e) => this.warn(this.i18n.error(&e)),
            }
        });
    }

    pub fn stop(&self, key: String) {
        spawn_local(async move {
            let _ = ipc::invoke::<_, usize>("build_stop", &KeyArgs { key }).await;
        });
    }

    pub fn open_dir(&self, key: String) {
        let this = *self;
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, ()>("build_open_dir", &KeyArgs { key }).await {
                this.warn(this.i18n.error(&e));
            }
        });
    }

    pub fn shortcut(&self, key: String) {
        let this = *self;
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, String>("build_shortcut", &KeyArgs { key }).await {
                this.warn(this.i18n.error(&e));
            }
        });
    }

    /// Starts `build` (the backend shows "starting"). A missing account opens "Profile required"
    /// (`need_profile`).
    pub fn launch(
        &self,
        build: BuildDto,
        profile_key: Option<String>,
        allow_duplicate: bool,
        need_profile: RwSignal<bool>,
    ) {
        let this = *self;
        let key = build.key.clone();
        this.store.launching.update(|set| {
            set.insert(key.clone());
        });
        spawn_local(async move {
            let args = LaunchArgs { key: key.clone(), profile_key, allow_duplicate };
            let result = ipc::invoke::<_, u32>("build_launch", &args).await;
            this.store.launching.update(|set| {
                set.remove(&key);
            });
            match result {
                Ok(_) => {}
                Err(e) if e.code == ErrorCode::NoProfile => need_profile.set(true),
                Err(e) => this.toasts.show(Level::Warning, this.i18n.error(&e), None),
            }
        });
    }
}

pub fn use_build_actions() -> BuildActions {
    BuildActions { store: use_store(), toasts: use_toasts(), i18n: use_i18n() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_finished_call_never_touches_a_closed_page() {
        let owner = Owner::new();
        let done = owner.with(|| Callback::new(|_: bool| {}));
        owner.cleanup();
        finish(done, true);
    }

    #[test]
    fn ipc_arguments_use_tauri_names() {
        let launch =
            LaunchArgs { key: "aero".into(), profile_key: Some("Steve".into()), allow_duplicate: true };
        assert_eq!(
            serde_json::to_value(launch).unwrap(),
            json!({"key": "aero", "profileKey": "Steve", "allowDuplicate": true})
        );
        let delete = DeleteArgs { key: "aero".into(), delete_files: false };
        assert_eq!(serde_json::to_value(delete).unwrap(), json!({"key": "aero", "deleteFiles": false}));
        let create = CreateArgs { name: "Aero".into(), version: "1.21.1".into() };
        assert_eq!(serde_json::to_value(create).unwrap(), json!({"name": "Aero", "version": "1.21.1"}));
        let loader = CreateLoaderArgs {
            name: "F".into(),
            loader: LoaderKind::Fabric,
            mc: "1.21.1".into(),
            loader_version: "0.16.9".into(),
        };
        assert_eq!(
            serde_json::to_value(loader).unwrap(),
            json!({"name": "F", "loader": "fabric", "mc": "1.21.1", "loaderVersion": "0.16.9"})
        );
    }
}
