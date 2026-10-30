//! The «Скіни» tab: the library's cards, its search and «Додати скін».

use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionGroup, Button, ConfirmDialog, Dialog, DialogFooter, DropZone, EmptyState, IconAction, MenuEntry,
    Rect, Skeleton, TextInput, Variant, use_context_menu, use_toasts,
};

use super::add::AddSkinDialog;
use super::api;
use super::model::{is_worn, matching};
use crate::dto::{AccountState, ImportArgs, LibrarySkin, SkinSource, SkinVariant, error_text};

fn model_name(variant: SkinVariant) -> &'static str {
    match variant {
        SkinVariant::Classic => "Classic",
        SkinVariant::Slim => "Slim",
    }
}

#[component]
pub fn LibraryTab(
    library: RwSignal<Option<Vec<LibrarySkin>>>,
    account: RwSignal<String>,
    /// The card worn, and the model it is worn on.
    #[prop(into)]
    worn: Signal<Option<(String, SkinVariant)>>,
    chosen: RwSignal<Option<String>>,
    hovered: RwSignal<Option<String>>,
    #[prop(into)] can_wear: Signal<bool>,
    on_state: Callback<AccountState>,
    reload: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let menu = use_context_menu();
    let host = ui_kit::module::use_module_host();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let query = RwSignal::new(String::new());
    let add_open = RwSignal::new(false);
    // The library as it was when «Додати скін» opened: a skin already in it is not new.
    let before = StoredValue::new(Vec::<String>::new());
    let applying = RwSignal::new(None::<String>);
    let renaming = RwSignal::new(None::<LibrarySkin>);
    let (rename_open, new_name) = (RwSignal::new(false), RwSignal::new(String::new()));
    let rename_error = RwSignal::new(None::<String>);
    let deleting = RwSignal::new(None::<LibrarySkin>);
    let delete_open = RwSignal::new(false);

    let fail = move |e: AppError| toasts.show(Level::Error, i18n.text(&error_text(&e)), None);
    let open_add = move || {
        before.set_value(library.with_untracked(|l| l.iter().flatten().map(|s| s.id.clone()).collect()));
        add_open.set(true);
    };
    // A skin added: told whether it was new, shown in the viewer (the library reloads on the
    // module's change event).
    let on_added = Callback::new(move |skin: LibrarySkin| {
        let known = before.try_with_value(|ids| ids.contains(&skin.id)).unwrap_or(false);
        let key = if known { "skins_already" } else { "skins_added" };
        toasts.show(Level::Success, i18n.tp(key, &[("name", skin.name.clone())]), None);
        let _ = chosen.try_set(Some(skin.id));
    });
    // Skins dropped from the system: each added as «Додати скін» adds a file, named after it.
    let on_dropped = Callback::new(move |paths: Vec<String>| {
        before.set_value(library.with_untracked(|l| l.iter().flatten().map(|s| s.id.clone()).collect()));
        spawn_local(async move {
            for path in paths {
                match api::import(ImportArgs::new(SkinSource::File { path })).await {
                    Ok(skin) => {
                        let _ = on_added.try_run(skin);
                    }
                    Err(e) => fail(e),
                }
            }
        });
    });
    let wear = move |skin: LibrarySkin| {
        if applying.get_untracked().is_some() {
            return;
        }
        applying.set(Some(skin.id.clone()));
        let profile = account.get_untracked();
        spawn_local(async move {
            match api::apply(profile.clone(), skin.id.clone()).await {
                Ok(answer) => {
                    let _ = on_state.try_run(answer);
                    toasts.show(Level::Success, i18n.tp("skins_applied", &[("name", skin.name)]), None);
                    if let Some(host) = host {
                        host.avatar_changed.run(profile);
                    }
                }
                Err(e) => fail(e),
            }
            let _ = applying.try_set(None);
        });
    };
    let save_name = move || {
        let Some(skin) = renaming.get_untracked() else { return };
        let name = new_name.get_untracked();
        spawn_local(async move {
            match api::rename(skin.id, name).await {
                Ok(_) => {
                    let _ = rename_open.try_set(false);
                }
                Err(e) => {
                    let _ = rename_error.try_set(Some(i18n.text(&error_text(&e))));
                }
            }
        });
    };
    let delete = move || {
        let Some(skin) = deleting.get_untracked() else { return };
        spawn_local(async move {
            match api::delete(skin.id.clone()).await {
                Ok(_) => {
                    if chosen.try_get_untracked().flatten().as_deref() == Some(skin.id.as_str()) {
                        let _ = chosen.try_set(None);
                    }
                }
                Err(e) => fail(e),
            }
        });
    };
    // The model is the switch under the figure: one place, which keeps a worn skin worn.
    let open_menu = move |skin: LibrarySkin, x: i32, y: i32| {
        let entries = vec![
            MenuEntry::item("rename", i18n.t("rename")).icon("edit"),
            MenuEntry::Separator,
            MenuEntry::item("delete", i18n.t("delete")).icon("delete_outline").danger(),
        ];
        menu.open_with(x, y, entries, move |id: String| match id.as_str() {
            "rename" => {
                new_name.set(skin.name.clone());
                rename_error.set(None);
                renaming.set(Some(skin.clone()));
                rename_open.set(true);
            }
            "delete" => {
                deleting.set(Some(skin.clone()));
                delete_open.set(true);
            }
            _ => {}
        });
    };

    let card = move |skin: LibrarySkin| {
        let id = skin.id.clone();
        let (for_click, for_hover, for_chosen, for_busy) = (id.clone(), id.clone(), id.clone(), id.clone());
        let for_worn = skin.clone();
        let (for_menu, for_more, for_wear) = (skin.clone(), skin.clone(), skin.clone());
        let more = NodeRef::<leptos::html::Span>::new();
        let is_worn = move || worn.with(|w| is_worn(w.as_ref(), &for_worn));
        let is_worn_label = is_worn.clone();
        let is_worn_button = is_worn.clone();
        view! {
            <div
                class="skins-card"
                class:is-chosen=move || chosen.get().as_deref() == Some(for_chosen.as_str())
                class:is-worn=is_worn
                on:click=move |_| chosen.set(Some(for_click.clone()))
                on:mouseenter=move |_| hovered.set(Some(for_hover.clone()))
                on:mouseleave=move |_| hovered.set(None)
                on:contextmenu=move |ev| {
                    ev.prevent_default();
                    open_menu(for_menu.clone(), ev.client_x(), ev.client_y());
                }
            >
                <div class="skins-card__picture">
                    <img class="skins-card__thumb" src=skin.thumb.clone() alt="" />
                </div>
                <div class="skins-card__title">
                    <span class="skins-card__name" data-tip=skin.name.clone() data-tip-side="top">{skin.name.clone()}</span>
                    <span class="skins-card__model">{model_name(skin.variant)}</span>
                </div>
                <div class="skins-card__actions">
                    <Button
                        variant=Variant::Primary
                        class="skins-card__wear"
                        loading=Signal::derive(move || applying.get().as_deref() == Some(for_busy.as_str()))
                        disabled=Signal::derive(move || is_worn_button() || !can_wear.get() || applying.get().is_some())
                        on_click=move |_| wear(for_wear.clone())
                    >
                        // The worn skin says so on its button: every card keeps its rows, and its buttons
                        // line up with the neighbours'.
                        {move || i18n.t(if is_worn_label() { "skins_worn" } else { "skins_wear" })}
                    </Button>
                    <span node_ref=more class="skins-card__more">
                        <IconAction
                            icon="more_vert"
                            title=t("skins_more")
                            on_click=Callback::new(move |()| {
                                if let Some(el) = more.get_untracked() {
                                    let r = Rect::of(&el);
                                    open_menu(for_more.clone(), r.x as i32, (r.y + r.height) as i32 + 4);
                                }
                            })
                        />
                    </span>
                </div>
            </div>
        }
    };

    let grid = move || {
        match library.get() {
        None => view! { <div class="skins-grid"><Skeleton /></div> }.into_any(),
        Some(list) if list.is_empty() => view! {
            <EmptyState icon="brand:skin" title=t("skins_library_empty_title") desc=t("skins_library_empty_desc")>
                <Button variant=Variant::Primary icon="add" on_click=move |_| open_add()>
                    {move || i18n.t("skins_add")}
                </Button>
            </EmptyState>
        }
        .into_any(),
        Some(list) => match matching(&list, &query.get()) {
            found if found.is_empty() => {
                view! { <EmptyState icon="search_off" title=t("skins_nothing_found") /> }.into_any()
            }
            found => view! { <div class="skins-grid">{found.into_iter().map(card).collect_view()}</div> }.into_any(),
        },
    }
    };
    view! {
        <DropZone accept=&["png"] label=t("skins_drop") on_drop=on_dropped class="skins-library">
            <div class="create__bar">
                <div class="create__tools">
                    <div class="create__search">
                        <TextInput value=query icon="search" placeholder=t("skins_search") />
                    </div>
                    <ActionGroup>
                        <IconAction icon="refresh" title=t("refresh") on_click=reload />
                    </ActionGroup>
                    <Button variant=Variant::Primary icon="add" on_click=move |_| open_add()>
                        {move || i18n.t("skins_add")}
                    </Button>
                </div>
            </div>
            {grid}
        </DropZone>
        <AddSkinDialog open=add_open on_added=on_added />
        <Dialog open=rename_open title=t("skins_rename_title") icon="edit">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| rename_open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary on_click=move |_| save_name()>{move || i18n.t("save")}</Button>
            </DialogFooter>
            <div class="skins-rename">
                <TextInput value=new_name placeholder=t("skins_name") autofocus=true on_enter=Callback::new(move |()| save_name()) />
                {move || rename_error.get().map(|e| view! { <div class="skins-rename__error">{e}</div> })}
            </div>
        </Dialog>
        <ConfirmDialog
            open=delete_open
            title=t("confirmation")
            message=Signal::derive(move || {
                let name = deleting.with(|d| d.as_ref().map(|s| s.name.clone()).unwrap_or_default());
                i18n.tp("skins_delete_confirm", &[("name", name)])
            })
            confirm_label=t("delete")
            cancel_label=t("cancel")
            danger=true
            on_confirm=Callback::new(move |()| delete())
        />
    }
}
