//! What can be done to a screenshot: open it, show it in its folder, copy it, rename it, delete
//! it (or several). Failures are told with a toast.

use launcher_shared::{AppError, Level, ScreenshotDto, ShotRef};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Toasts, ipc, use_toasts};

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[derive(serde::Serialize)]
struct ShotArgs {
    key: String,
    name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RenameArgs {
    key: String,
    name: String,
    new_name: String,
}

#[derive(serde::Serialize)]
struct ItemsArgs {
    items: Vec<ShotRef>,
}

#[derive(Clone, Copy)]
pub struct ShotActions {
    toasts: Toasts,
    i18n: I18nCtx,
}

impl ShotActions {
    fn warn(&self, e: &AppError) {
        self.toasts.show(Level::Error, self.i18n.error(e), None);
    }

    /// `command` about one screenshot, told when it fails.
    fn about(&self, command: &'static str, key: String, name: String) {
        let this = *self;
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>(command, &ShotArgs { key, name }).await {
                this.warn(&e);
            }
        });
    }

    /// In the system's picture viewer.
    pub fn open(&self, key: String, name: String) {
        self.about("screenshot_open", key, name);
    }

    pub fn reveal(&self, key: String, name: String) {
        self.about("screenshot_reveal", key, name);
    }

    /// As a picture on the clipboard (the backend says when it is there).
    pub fn copy(&self, key: String, name: String) {
        self.about("screenshot_copy", key, name);
    }

    /// The screenshots folder of build `key`.
    pub fn open_dir(&self, key: String) {
        let this = *self;
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>("screenshots_open_dir", &KeyArgs { key }).await {
                this.warn(&e);
            }
        });
    }

    /// Renames screenshot `name` of build `key` to `wanted`; `done` gets the build key and the
    /// renamed screenshot, or why it was not (already in words). `done` is made with the page, not
    /// per click: what a click makes goes with the dialog it closes.
    pub fn rename(
        &self,
        key: String,
        name: String,
        wanted: String,
        done: Callback<(String, Result<ScreenshotDto, String>)>,
    ) {
        let i18n = self.i18n;
        spawn_local(async move {
            let args = RenameArgs { key: key.clone(), name, new_name: wanted };
            let result = ipc::invoke::<_, ScreenshotDto>("screenshot_rename", &args).await;
            let _ = done.try_run((key, result.map_err(|e| i18n.error(&e))));
        });
    }

    /// Deletes `items`; `done` (made with the page) gets how many went.
    pub fn delete(&self, items: Vec<ShotRef>, done: Callback<usize>) {
        let this = *self;
        spawn_local(async move {
            match ipc::invoke::<_, usize>("screenshots_delete", &ItemsArgs { items }).await {
                Ok(count) => {
                    let _ = done.try_run(count);
                }
                Err(e) => {
                    this.warn(&e);
                    let _ = done.try_run(0);
                }
            }
        });
    }
}

pub fn use_shot_actions() -> ShotActions {
    ShotActions { toasts: use_toasts(), i18n: use_i18n() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_use_tauri_names() {
        let rename = RenameArgs { key: "aero".into(), name: "a.png".into(), new_name: "b".into() };
        assert_eq!(
            serde_json::to_value(rename).unwrap(),
            serde_json::json!({"key": "aero", "name": "a.png", "newName": "b"})
        );
        let items = ItemsArgs { items: vec![ShotRef { key: "aero".into(), name: "a.png".into() }] };
        assert_eq!(
            serde_json::to_value(items).unwrap(),
            serde_json::json!({"items": [{"key": "aero", "name": "a.png"}]})
        );
    }
}
