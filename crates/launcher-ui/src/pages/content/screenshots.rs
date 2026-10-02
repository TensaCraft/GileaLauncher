//! The Screenshots tab: a build's screenshots, newest first — a preview, the
//! system viewer, delete — and their folder.

use launcher_shared::{AppError, Level, ScreenshotDto};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionTone, Button, ConfirmDialog, Dialog, DialogFooter, EmptyState, IconAction, Reloadable, Skeleton,
    Variant, ipc, use_toasts,
};

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[derive(serde::Serialize)]
struct ShotArgs {
    key: String,
    name: String,
}

/// Local "dd.mm.yyyy hh:mm" of Unix milliseconds.
fn date_time(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}

#[component]
pub fn ScreenshotsPanel(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let key = StoredValue::new(key);
    // Reloads in place; the error is already translated.
    let list = Reloadable::<Vec<ScreenshotDto>>::new();
    let load = move || {
        let Some(request) = list.begin() else { return };
        let args = KeyArgs { key: key.get_value() };
        spawn_local(async move {
            let result = ipc::invoke::<_, Vec<ScreenshotDto>>("screenshots_list", &args).await;
            if let Some(told) = list.finish(request, result.map_err(|e| i18n.error(&e))) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    load();
    let fail = move |e: AppError| toasts.show(Level::Error, i18n.error(&e), None);
    let open = move |name: String| {
        let args = ShotArgs { key: key.get_value(), name };
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>("screenshot_open", &args).await {
                fail(e);
            }
        });
    };
    let open_dir = move || {
        let args = KeyArgs { key: key.get_value() };
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>("screenshots_open_dir", &args).await {
                fail(e);
            }
        });
    };

    let preview = RwSignal::new(None::<ScreenshotDto>);
    let preview_open = RwSignal::new(false);
    let delete_of = RwSignal::new(None::<ScreenshotDto>);
    let delete_open = RwSignal::new(false);
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        let Some(shot) = delete_of.get_untracked() else { return };
        let Some(request) = list.begin() else { return };
        let args = ShotArgs { key: key.get_value(), name: shot.name.clone() };
        spawn_local(async move {
            match ipc::invoke::<_, Vec<ScreenshotDto>>("screenshot_delete", &args).await {
                Ok(fresh) => {
                    list.finish(request, Ok(fresh));
                }
                Err(_) => {
                    toasts.show(
                        Level::Error,
                        i18n.tp("screenshot_delete_failed", &[("name", shot.name)]),
                        None,
                    );
                    load();
                }
            }
        });
    });
    let delete_text = Signal::derive(move || {
        delete_of
            .with(|s| s.as_ref().map(|s| i18n.tp("confirm_delete_screenshot", &[("name", s.name.clone())])))
            .unwrap_or_default()
    });

    let row = move |shot: ScreenshotDto| {
        let meta = format!(
            "{} • {}",
            launcher_shared::units::size(shot.size, &i18n.lang()),
            shot.modified_ms.map(date_time).unwrap_or_else(|| "-".into())
        );
        let (for_preview, for_open, for_delete) = (shot.clone(), shot.name.clone(), shot.clone());
        view! {
            <div class="list-row build-row shot-row">
                <button
                    type="button"
                    class="shot__thumb"
                    data-tip=move || i18n.t("open_screenshot")
                    data-tip-side="top"
                    aria-label=move || i18n.t("open_screenshot")
                    on:click=move |_| {
                        preview.set(Some(for_preview.clone()));
                        preview_open.set(true);
                    }
                >
                    <img src=shot.src.clone() alt="" loading="lazy" />
                </button>
                <div class="build-row__text">
                    <div class="build-row__name">{shot.name.clone()}</div>
                    <div class="build-row__sub component__meta">{meta}</div>
                </div>
                <div class="build-row__actions">
                    <IconAction
                        icon="open_in_new"
                        tone=ActionTone::Info
                        title=t("open_screenshot")
                        on_click=Callback::new(move |()| open(for_open.clone()))
                    />
                    <IconAction
                        icon="delete_outline"
                        tone=ActionTone::Danger
                        title=t("delete")
                        on_click=Callback::new(move |()| {
                            delete_of.set(Some(for_delete.clone()));
                            delete_open.set(true);
                        })
                    />
                </div>
            </div>
        }
    };
    let body = move || {
        match list.shown().get() {
        None => view! {
            <div class="builds__list">
                {(0..3)
                    .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=96 /><Skeleton width=260 /></div> })
                    .collect_view()}
            </div>
        }
        .into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="error_outline" title=t("unknown_error") desc=message>
                <Button icon="refresh" on_click=move |_| load()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(shots)) if shots.is_empty() => {
            view! { <EmptyState icon="photo_library" title=t("screenshots_empty") /> }.into_any()
        }
        Some(Ok(shots)) => {
            view! { <div class="builds__list">{shots.into_iter().map(row).collect_view()}</div> }.into_any()
        }
    }
    };

    view! {
        <div class="create__bar">
            <div class="create__tools">
                <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load()) />
                <Button icon="folder_open" on_click=move |_| open_dir()>{move || i18n.t("open_screenshots_folder")}</Button>
            </div>
        </div>
        {body}
        <Dialog
            open=preview_open
            wide=true
            icon="image"
            title=Signal::derive(move || preview.with(|p| p.as_ref().map(|p| p.name.clone()).unwrap_or_default()))
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| preview_open.set(false)>{move || i18n.t("close")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="open_in_new"
                    on_click=move |_| {
                        if let Some(p) = preview.get_untracked() {
                            open(p.name);
                        }
                    }
                >
                    {move || i18n.t("open_screenshot")}
                </Button>
            </DialogFooter>
            {move || preview.get().map(|p| view! { <img class="shot__preview" src=p.src alt="" /> })}
        </Dialog>
        <ConfirmDialog
            open=delete_open
            danger=true
            title=t("confirmation")
            message=delete_text
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=confirm_delete
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_arguments_use_tauri_names() {
        let args = ShotArgs { key: "aero".into(), name: "a b.png".into() };
        assert_eq!(
            serde_json::to_value(args).unwrap(),
            serde_json::json!({"key": "aero", "name": "a b.png"})
        );
    }
}
