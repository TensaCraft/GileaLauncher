//! Create build: the install catalog (`builds::catalog`) and a name dialog
//! before the install — for loaders also the loader build.

use launcher_shared::LoaderKind;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Field, Select, TextInput, Variant};

use crate::builds::actions::use_build_actions;
use crate::builds::catalog::{Catalog, CreateRow, build_choices, row_title};
use crate::builds::{check_new_name, default_build_name};
use crate::shell::PageHeader;
use crate::store::use_store;

#[component]
pub fn CreateBuildPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_build_actions();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let install_of = RwSignal::new(None::<CreateRow>);
    let install_open = RwSignal::new(false);
    let install_name = RwSignal::new(String::new());
    let install_build = RwSignal::new(String::new());
    let install_error = RwSignal::new(None::<String>);
    let installed = Callback::new(move |ok: bool| {
        if ok {
            store.go("/builds");
        }
    });
    let open_install = Callback::new(move |row: CreateRow| {
        install_name.set(store.builds.with_untracked(|b| default_build_name(&row_title(&row), b)));
        install_build.set(row.default_version.clone().unwrap_or_default());
        install_error.set(None);
        install_of.set(Some(row));
        install_open.set(true);
    });
    let submit = move || {
        let Some(row) = install_of.get_untracked() else { return };
        let raw = install_name.get_untracked();
        match store.builds.with_untracked(|b| check_new_name(&raw, b)) {
            Err(problem) => {
                install_error.set(Some(i18n.tp(problem.key(), &[("name", raw.trim().to_string())])))
            }
            Ok(name) => {
                install_open.set(false);
                match row.kind {
                    LoaderKind::Minecraft => actions.create_vanilla(name, row.mc, installed),
                    kind => {
                        actions.create_loader(name, kind, row.mc, install_build.get_untracked(), installed)
                    }
                }
            }
        }
    };
    let install_label = move || install_of.with(|r| r.as_ref().map(row_title).unwrap_or_default());

    view! {
        <PageHeader title_key="create_version_title" back="/builds" />
        <div class="create">
            <Catalog on_pick=open_install />
        </div>
        <Dialog
            open=install_open
            title=Signal::derive(move || i18n.tp("version_create_install_confirm_title", &[("version", install_label())]))
            subtitle=t("version_create_install_confirm_message")
            icon="download"
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| install_open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="download" on_click=move |_| submit()>
                    {move || i18n.t("minecraft_components_install_action")}
                </Button>
            </DialogFooter>
            <Field label=t("version_name_label") error=Signal::derive(move || install_error.get())>
                <TextInput
                    value=install_name
                    autofocus=true
                    invalid=Signal::derive(move || install_error.get().is_some())
                    on_enter=Callback::new(move |()| submit())
                />
            </Field>
            <Show when=move || install_of.with(|r| r.as_ref().is_some_and(|r| r.kind != LoaderKind::Minecraft))>
                <Field label=t("minecraft_components_loader_build_label")>
                    <Select options=Signal::derive(move || install_of.with(|r| build_choices(r.as_ref(), &i18n.t("version_create_unstable_loader_badge")))) value=install_build />
                </Field>
            </Show>
        </Dialog>
    }
}
