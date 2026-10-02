//! Components: what `versions/` holds — each with its loader, Minecraft
//! version, size, date and the builds that use it — with Verify (repairing what it finds),
//! Reinstall, Open folder and Delete; and an Install mode that puts a component in place without
//! a build.

use launcher_shared::{ComponentDto, ComponentsSnapshot, Level, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionTone, Button, ConfirmDialog, Dialog, DialogFooter, EmptyState, Field, Icon, IconAction, Reloadable,
    SegOption, Segmented, Select, Skeleton, TextInput, Variant, ipc, use_toasts,
};

use crate::builds::catalog::{Catalog, CreateRow, build_choices, row_title};
use crate::builds::loader_icon;
use crate::shell::PageHeader;

type TextParams = Vec<(&'static str, String)>;

/// The translation key and parameters of a component's usage line.
pub fn usage(c: &ComponentDto) -> (&'static str, TextParams) {
    if !c.used_by.is_empty() {
        ("minecraft_components_used_by", vec![("versions", c.used_by.join(", "))])
    } else if !c.base_for.is_empty() {
        ("minecraft_components_base_for", vec![("count", c.base_for.len().to_string())])
    } else {
        ("minecraft_components_unused", Vec::new())
    }
}

/// The delete confirmation: the builds that use it first, then the components built on it.
pub fn delete_message(c: &ComponentDto) -> (&'static str, TextParams) {
    if !c.used_by.is_empty() {
        ("minecraft_components_delete_used_message", vec![("versions", c.used_by.join(", "))])
    } else if !c.base_for.is_empty() {
        ("minecraft_components_delete_base_message", vec![("count", c.base_for.len().to_string())])
    } else {
        ("minecraft_components_delete_message", Vec::new())
    }
}

/// "Minecraft 1.21.1", "Fabric • Minecraft 1.21.1"; an unknown loader shows as "?".
pub fn summary(c: &ComponentDto) -> String {
    let kind = c.loader.map(LoaderKind::display_name).unwrap_or("?");
    match (c.loader, &c.minecraft) {
        (Some(LoaderKind::Minecraft), Some(mc)) => format!("Minecraft {mc}"),
        (_, Some(mc)) => format!("{kind} • Minecraft {mc}"),
        (_, None) => kind.to_string(),
    }
}

/// Local "dd.mm.yyyy" of Unix milliseconds.
fn date_of(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!("{:02}.{:02}.{}", d.get_date(), d.get_month() + 1, d.get_full_year())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallArgs {
    loader: LoaderKind,
    mc: String,
    loader_version: Option<String>,
}

#[component]
pub fn ComponentsPage() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let mode = RwSignal::new("installed".to_string());
    // Reloads in place, only the newest answer lands; the error is already translated.
    let components = Reloadable::<Vec<ComponentDto>>::new();
    let list = components.shown();
    let query = RwSignal::new(String::new());
    let refresh = move || {
        let Some(request) = components.begin() else { return };
        spawn_local(async move {
            let result = ipc::call::<ComponentsSnapshot>("components_list").await;
            let arrived = result.map(|s| s.components).map_err(|e| i18n.error(&e));
            if let Some(told) = components.finish(request, arrived) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    refresh();
    let installed_ids = Signal::derive(move || {
        list.with(|l| match l {
            Some(Ok(items)) => items.iter().map(|c| c.id.clone()).collect(),
            _ => Vec::new(),
        })
    });
    // The components at work: their rows' actions wait until it ends (one at a time each).
    let working = RwSignal::new(Vec::<String>::new());
    let at_work = move |id: &str| working.with(|w| w.iter().any(|v| v == id));
    // Successes are announced by the backend; a failure gets the action's own message
    // (`failed`, with `{version}` and `{error}`) or, without one, the translated error. `occupy`
    // holds the component `version` names until the answer.
    let run = move |command: &'static str,
                    args: Value,
                    failed: Option<&'static str>,
                    version: String,
                    occupy: bool| {
        if occupy {
            if working.with_untracked(|w| w.contains(&version)) {
                return;
            }
            working.update(|w| w.push(version.clone()));
        }
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>(command, &args).await {
                let text = match failed {
                    Some(key) => i18n.tp(key, &[("version", version.clone()), ("error", i18n.error(&e))]),
                    None => i18n.error(&e),
                };
                toasts.show(Level::Error, text, None);
            }
            if occupy {
                let _ = working.try_update(|w| w.retain(|v| *v != version));
            }
            refresh();
        });
    };

    let reinstall_of = RwSignal::new(None::<ComponentDto>);
    let reinstall_open = RwSignal::new(false);
    let delete_of = RwSignal::new(None::<ComponentDto>);
    let delete_open = RwSignal::new(false);
    let install_of = RwSignal::new(None::<CreateRow>);
    let install_open = RwSignal::new(false);
    let install_build = RwSignal::new(String::new());
    let confirm_reinstall = Callback::new(move |()| {
        reinstall_open.set(false);
        if let Some(c) = reinstall_of.get_untracked() {
            run(
                "component_reinstall",
                json!({"id": c.id}),
                Some("minecraft_components_reinstall_failed"),
                c.id.clone(),
                true,
            );
        }
    });
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        if let Some(c) = delete_of.get_untracked() {
            run(
                "component_delete",
                json!({"id": c.id}),
                Some("minecraft_components_delete_failed"),
                c.id.clone(),
                true,
            );
        }
    });
    let pick = Callback::new(move |row: CreateRow| {
        install_build.set(row.default_version.clone().unwrap_or_default());
        install_of.set(Some(row));
        install_open.set(true);
    });
    let submit_install = move || {
        let Some(row) = install_of.get_untracked() else { return };
        install_open.set(false);
        let label = row_title(&row);
        let loader_version = (row.kind != LoaderKind::Minecraft).then(|| install_build.get_untracked());
        let args = InstallArgs { loader: row.kind, mc: row.mc, loader_version };
        let args = serde_json::to_value(args).unwrap_or_default();
        run("component_install", args, Some("minecraft_components_install_failed"), label, true);
    };

    let row = move |c: ComponentDto| {
        let kind_name = c.loader.map(LoaderKind::display_name).unwrap_or("?");
        let sub = {
            let (text, lv) = (summary(&c), c.loader_version.clone());
            move || match &lv {
                Some(v) => {
                    format!("{text} • {}", i18n.tp("version_create_loader_build", &[("version", v.clone())]))
                }
                None => text.clone(),
            }
        };
        let meta = {
            let (size, modified, (key, params)) =
                (launcher_shared::units::size(c.size, &i18n.lang()), c.modified_ms.map(date_of), usage(&c));
            move || {
                let mut parts = vec![size.clone()];
                if let Some(date) = &modified {
                    parts.push(i18n.tp("minecraft_components_modified", &[("date", date.clone())]));
                }
                parts.push(i18n.tp(key, &params));
                parts.join(" • ")
            }
        };
        let (for_verify, for_reinstall, for_open, for_delete) = (c.clone(), c.clone(), c.clone(), c.clone());
        let busy = {
            let id = c.id.clone();
            Signal::derive(move || at_work(&id))
        };
        view! {
            <div class="list-row build-row">
                <div class="build-row__icon"><Icon name=loader_icon(Some(kind_name)) /></div>
                <div class="build-row__text">
                    <div class="build-row__name">{c.id.clone()}</div>
                    <div class="build-row__sub">{sub}</div>
                    <div class="build-row__sub component__meta">{meta}</div>
                </div>
                <div class="build-row__actions">
                    <IconAction
                        icon="fact_check"
                        tone=ActionTone::Ok
                        title=t("minecraft_components_verify")
                        loading=busy
                        on_click=Callback::new(move |()| {
                            let c = for_verify.clone();
                            run("component_verify", json!({"id": c.id}), Some("minecraft_components_repair_failed"), c.id.clone(), true);
                        })
                    />
                    <IconAction
                        icon="restart_alt"
                        title=t("minecraft_components_reinstall")
                        loading=busy
                        on_click=Callback::new(move |()| {
                            reinstall_of.set(Some(for_reinstall.clone()));
                            reinstall_open.set(true);
                        })
                    />
                    <IconAction
                        icon="folder_open"
                        tone=ActionTone::Info
                        title=t("open_directory")
                        on_click=Callback::new(move |()| {
                            let c = for_open.clone();
                            run("component_open_dir", json!({"id": c.id}), None, c.id.clone(), false);
                        })
                    />
                    <IconAction
                        icon="delete_outline"
                        tone=ActionTone::Danger
                        title=t("delete")
                        loading=busy
                        on_click=Callback::new(move |()| {
                            delete_of.set(Some(for_delete.clone()));
                            delete_open.set(true);
                        })
                    />
                </div>
            </div>
        }
    };
    let matching = move || {
        let q = query.get().trim().to_lowercase();
        list.with(|l| match l {
            Some(Ok(items)) => Some(
                items
                    .iter()
                    .filter(|c| q.is_empty() || c.id.to_lowercase().contains(&q))
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
    };
    let installed_body = move || {
        match (list.with(|l| l.as_ref().map(|r| r.as_ref().err().cloned())), matching()) {
        (None, _) => view! {
            <div class="builds__list">
                {(0..4)
                    .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=48 /><Skeleton width=260 /></div> })
                    .collect_view()}
            </div>
        }
        .into_any(),
        (Some(Some(message)), _) => view! {
            <EmptyState icon="error_outline" title=t("unknown_error") desc=message>
                <Button icon="refresh" on_click=move |_| refresh()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        (_, Some(items)) if !items.is_empty() => {
            view! { <div class="builds__list">{items.into_iter().map(row).collect_view()}</div> }.into_any()
        }
        _ => view! { <EmptyState icon="inventory_2" title=t("minecraft_components_empty") /> }.into_any(),
    }
    };
    // Installed or install: in the list's own bar, right after its search, in both modes.
    let modes = ViewFn::from(move || {
        view! {
            <Segmented
                options=Signal::derive(move || {
                    vec![
                        SegOption::new("installed", i18n.t("minecraft_components_installed_tab"))
                            .with_icon("inventory_2")
                            .with_count(list.with(|l| match l {
                                Some(Ok(items)) => Some(items.len()),
                                _ => None,
                            })),
                        SegOption::new("install", i18n.t("minecraft_components_install_tab")).with_icon("download"),
                    ]
                })
                value=mode
            />
        }
    });
    let catalog_modes = modes.clone();
    let delete_text = Signal::derive(move || {
        delete_of.with(|c| {
            c.as_ref()
                .map(|c| {
                    let (key, params) = delete_message(c);
                    i18n.tp(key, &params)
                })
                .unwrap_or_default()
        })
    });
    let dialog_version = move |of: RwSignal<Option<ComponentDto>>| {
        of.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default())
    };

    view! {
        <PageHeader title_key="minecraft_components_title" back="/builds" />
        <div class="create">
            <Show
                when=move || mode.get() == "installed"
                fallback=move || view! { <Catalog on_pick=pick installed=installed_ids sources=catalog_modes.clone() /> }
            >
                <div class="create__bar">
                    <div class="create__tools">
                        <div class="create__search">
                            <TextInput value=query icon="search" placeholder=t("minecraft_components_search") />
                        </div>
                        {modes.run()}
                        <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| refresh()) />
                    </div>
                </div>
                {installed_body}
            </Show>
        </div>
        <Dialog
            open=install_open
            title=Signal::derive(move || {
                let label = install_of.with(|r| r.as_ref().map(row_title).unwrap_or_default());
                i18n.tp("minecraft_components_install_confirm_title", &[("version", label)])
            })
            subtitle=t("minecraft_components_install_confirm_message")
            icon="download"
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| install_open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="download" on_click=move |_| submit_install()>
                    {move || i18n.t("minecraft_components_install_action")}
                </Button>
            </DialogFooter>
            <Show when=move || install_of.with(|r| r.as_ref().is_some_and(|r| r.kind != LoaderKind::Minecraft))>
                <Field label=t("minecraft_components_loader_build_label")>
                    <Select options=Signal::derive(move || install_of.with(|r| build_choices(r.as_ref(), &i18n.t("version_create_unstable_loader_badge")))) value=install_build />
                </Field>
            </Show>
        </Dialog>
        <ConfirmDialog
            open=reinstall_open
            title=Signal::derive(move || i18n.tp("minecraft_components_reinstall_confirm_title", &[("version", dialog_version(reinstall_of))]))
            message=t("minecraft_components_reinstall_confirm_message")
            confirm_label=t("minecraft_components_reinstall")
            cancel_label=t("cancel")
            on_confirm=confirm_reinstall
        />
        <ConfirmDialog
            open=delete_open
            danger=true
            title=Signal::derive(move || i18n.tp("minecraft_components_delete_confirm_title", &[("version", dialog_version(delete_of))]))
            message=delete_text
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=confirm_delete
        />
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn component(used_by: &[&str], base_for: &[&str]) -> ComponentDto {
        ComponentDto {
            id: "1.21.1".into(),
            loader: Some(LoaderKind::Minecraft),
            minecraft: Some("1.21.1".into()),
            loader_version: None,
            size: 1,
            modified_ms: None,
            used_by: used_by.iter().map(|s| s.to_string()).collect(),
            base_for: base_for.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn usage_and_delete_texts_name_builds_first() {
        let used = component(&["Aero", "Vanilla"], &["fabric-loader-0.16.9-1.21.1"]);
        assert_eq!(
            usage(&used),
            ("minecraft_components_used_by", vec![("versions", "Aero, Vanilla".to_string())])
        );
        assert_eq!(
            delete_message(&used),
            ("minecraft_components_delete_used_message", vec![("versions", "Aero, Vanilla".to_string())])
        );
        let base = component(&[], &["a", "b"]);
        assert_eq!(usage(&base), ("minecraft_components_base_for", vec![("count", "2".to_string())]));
        assert_eq!(
            delete_message(&base),
            ("minecraft_components_delete_base_message", vec![("count", "2".to_string())])
        );
        let free = component(&[], &[]);
        assert_eq!(usage(&free), ("minecraft_components_unused", vec![]));
        assert_eq!(delete_message(&free), ("minecraft_components_delete_message", vec![]));
    }

    #[test]
    fn summaries_name_the_loader_and_minecraft() {
        let mut c = component(&[], &[]);
        assert_eq!(summary(&c), "Minecraft 1.21.1");
        c.loader = Some(LoaderKind::Fabric);
        assert_eq!(summary(&c), "Fabric • Minecraft 1.21.1");
        c.loader = None;
        assert_eq!(summary(&c), "? • Minecraft 1.21.1");
        c.minecraft = None;
        assert_eq!(summary(&c), "?");
    }

    #[test]
    fn install_arguments_use_tauri_names() {
        let args = InstallArgs {
            loader: LoaderKind::Fabric,
            mc: "1.21.1".into(),
            loader_version: Some("0.16.9".into()),
        };
        assert_eq!(
            serde_json::to_value(args).unwrap(),
            json!({"loader": "fabric", "mc": "1.21.1", "loaderVersion": "0.16.9"})
        );
    }
}
