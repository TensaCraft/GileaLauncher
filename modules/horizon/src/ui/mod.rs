//! The Horizon module's UI: its card in the build wizard and the dialog that makes a Horizon build.

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use ui_kit::i18n::use_i18n;
use ui_kit::module::{ModuleBuildKind, UiModule, use_module_host};
use ui_kit::{Button, Dialog, DialogFooter, Field, TextInput, Variant, ipc, use_toasts};

use crate::dto::CreateArgs;

/// Horizon's own pages, opened from the dialog.
const LINKS: [(&str, &str); 2] = [
    ("horizon_site", "https://playhorizon.dev"),
    ("horizon_source", "https://github.com/horizon-rust/horizon"),
];

pub struct HorizonUi;

impl UiModule for HorizonUi {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../../locales/en_US.json")),
            _ => None,
        }
    }

    fn build_kinds(&self) -> Vec<ModuleBuildKind> {
        vec![ModuleBuildKind {
            module: crate::ID,
            id: crate::ENGINE,
            title: "horizon_kind_title",
            desc: "horizon_kind_desc",
            icon: "rocket_launch",
            dialog: |open| view! { <CreateDialog open=open /> }.into_any(),
        }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(HorizonUi)
}

/// What a Horizon build is, the name it gets, and Create: the build's first start brings the game.
#[component]
fn CreateDialog(open: RwSignal<bool>) -> impl IntoView {
    let (i18n, toasts) = (use_i18n(), use_toasts());
    let host = use_module_host();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let name = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let creating = RwSignal::new(false);
    Effect::new(move |_| {
        if open.get() {
            name.set(i18n.t("horizon_default_name"));
            error.set(None);
        }
    });
    let create = move || {
        if creating.get_untracked() {
            return;
        }
        creating.set(true);
        let args = CreateArgs { name: name.get_untracked(), ..CreateArgs::default() };
        spawn_local(async move {
            match ipc::module_invoke::<_, Value>(crate::ID, "create", &args).await {
                Ok(_) => {
                    let _ = open.try_set(false);
                    toasts.show(Level::Success, i18n.t("horizon_created"), None);
                    if let Some(host) = host {
                        host.go.run("/builds".to_string());
                    }
                }
                Err(e) => {
                    let _ = error.try_set(Some(i18n.error(&e)));
                }
            }
            let _ = creating.try_set(false);
        });
    };
    let link = move |url: &'static str| {
        spawn_local(async move {
            let _ = ipc::invoke::<_, ()>("open_url", &serde_json::json!({ "url": url })).await;
        });
    };

    view! {
        <Dialog open=open title=t("horizon_kind_title") subtitle=t("horizon_dialog_subtitle") icon="rocket_launch">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="add" loading=Signal::derive(move || creating.get()) on_click=move |_| create()>
                    {move || i18n.t("horizon_create")}
                </Button>
            </DialogFooter>
            <p class="hint">{move || i18n.t("horizon_dialog_about")}</p>
            <p class="hint">{move || i18n.t("horizon_dialog_account")}</p>
            <div class="wrap">
                {LINKS
                    .iter()
                    .map(|&(label, url)| view! {
                        <Button variant=Variant::Ghost icon="open_in_new" on_click=move |_| link(url)>
                            {move || i18n.t(label)}
                        </Button>
                    })
                    .collect_view()}
            </div>
            <Field label=t("version_name_label") error=Signal::derive(move || error.get())>
                <TextInput
                    value=name
                    autofocus=true
                    invalid=Signal::derive(move || error.get().is_some())
                    on_enter=Callback::new(move |()| create())
                />
            </Field>
        </Dialog>
    }
}
