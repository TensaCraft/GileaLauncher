//! «Додати скін»: where the skin comes from (a file, a player, a link), its name and its model.

use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Dialog, DialogFooter, Field, Icon, InFlight, PathField, Select, SelectOption, TextInput, Variant,
    ipc,
};

use super::api;
use super::model::import_args;
use crate::dto::{LibrarySkin, error_text};

#[component]
pub fn AddSkinDialog(open: RwSignal<bool>, on_added: Callback<LibrarySkin>) -> impl IntoView {
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    // The source picked last stays; each source keeps its own field while the dialog is open.
    let source = RwSignal::new("file".to_string());
    let (path, nick, link) =
        (RwSignal::new(String::new()), RwSignal::new(String::new()), RwSignal::new(String::new()));
    let (name, model) = (RwSignal::new(String::new()), RwSignal::new("auto".to_string()));
    let error = RwSignal::new(None::<String>);
    let adding = InFlight::new();

    Effect::new(move |_| {
        if open.get() {
            for field in [path, nick, link, name] {
                field.set(String::new());
            }
            model.set("auto".to_string());
            error.set(None);
        }
    });
    let value = move || match source.get().as_str() {
        "player" => nick.get(),
        "link" => link.get(),
        _ => path.get(),
    };
    let args = Memo::new(move |_| import_args(&source.get(), &value(), &name.get(), &model.get()));
    // A changed field is a new try: the last refusal goes.
    Effect::new(move |_| {
        args.track();
        error.set(None);
    });

    let submit = move || {
        let Some(args) = args.get_untracked() else { return };
        if !adding.start() {
            return;
        }
        spawn_local(async move {
            match api::import(args).await {
                Ok(skin) => {
                    let _ = open.try_set(false);
                    let _ = on_added.try_run(skin);
                }
                Err(e) => {
                    let _ = error.try_set(Some(i18n.text(&error_text(&e))));
                }
            }
            adding.done();
        });
    };
    let browse = Callback::new(move |()| {
        spawn_local(async move {
            let filter = i18n.t("skins_file_filter");
            match ipc::pick_file(None, &filter, &["png"]).await {
                Ok(Some(picked)) => {
                    let _ = path.try_set(picked);
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = error.try_set(Some(i18n.text(&error_text(&e))));
                }
            }
        });
    });

    let sources = Signal::derive(move || {
        vec![
            SelectOption::new("file", i18n.t("skins_from_file")),
            SelectOption::new("player", i18n.t("skins_from_player")),
            SelectOption::new("link", i18n.t("skins_from_link")),
        ]
    });
    let models = Signal::derive(move || {
        vec![
            SelectOption::new("auto", i18n.t("skins_model_auto")),
            SelectOption::new("classic", i18n.t("skins_model_classic")),
            SelectOption::new("slim", i18n.t("skins_model_slim")),
        ]
    });
    let enter = Callback::new(move |()| submit());
    let field = move || {
        match source.get().as_str() {
        "player" => view! {
            <Field label=t("skins_nick") hint=t("skins_nick_hint")>
                <TextInput value=nick icon="person_search" placeholder="Notch" autofocus=true on_enter=enter />
            </Field>
        }
        .into_any(),
        "link" => view! {
            <Field label=t("skins_link") hint=t("skins_link_hint")>
                <TextInput value=link icon="link" placeholder="https://…/skin.png" autofocus=true on_enter=enter />
            </Field>
        }
        .into_any(),
        _ => view! {
            <Field label=t("skins_file")>
                <PathField value=path browse_label=t("browse_file") on_browse=browse />
            </Field>
        }
        .into_any(),
    }
    };

    view! {
        <Dialog open=open title=t("skins_add") icon="brand:skin">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="add"
                    loading=adding.running()
                    disabled=Signal::derive(move || args.with(Option::is_none))
                    on_click=move |_| submit()
                >
                    {move || i18n.t("skins_add")}
                </Button>
            </DialogFooter>
            <Field label=t("skins_add_source")>
                <Select options=sources value=source />
            </Field>
            {field}
            <Field label=t("skins_name") optional_note=t("field_optional") hint=t("skins_name_hint")>
                <TextInput value=name on_enter=enter />
            </Field>
            <Field label=t("skins_model_label")>
                <Select options=models value=model />
            </Field>
            {move || error.get().map(|e| view! {
                <div class="skins-add__error"><Icon name="error_outline" /><span>{e}</span></div>
            })}
        </Dialog>
    }
}
