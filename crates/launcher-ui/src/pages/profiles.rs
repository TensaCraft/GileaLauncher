use launcher_shared::{AccountKind, AppError, AuthState, ProfileDto};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionTone, Button, ConfirmDialog, Dialog, DialogFooter, EmptyState, Field, Icon, IconAction, InFlight,
    MenuEntry, TextInput, Variant, use_context_menu,
};

use crate::profiles::actions::use_profile_actions;
use crate::profiles::avatar::ProfileAvatar;
use crate::profiles::kind_key;
use crate::shell::PageHeader;
use crate::store::use_store;

#[component]
pub fn ProfilesPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_profile_actions();
    let menu = use_context_menu();
    let query = use_query_map();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let signing_in = move || store.auth.get() != AuthState::Idle;

    let offline_open = RwSignal::new(false);
    let offline_name = RwSignal::new(String::new());
    let offline_error = RwSignal::new(None::<AppError>);
    let confirm_open = RwSignal::new(false);
    let pending_delete = RwSignal::new(None::<ProfileDto>);

    let open_offline = move || {
        offline_name.set(String::new());
        offline_error.set(None);
        offline_open.set(true);
    };
    // One profile at a time: a second Enter or click while it is made does nothing.
    let creating = InFlight::new();
    let submit_offline = move || {
        if !creating.start() {
            return;
        }
        // The answer may come after the page is gone.
        let done = Callback::new(move |error: Option<AppError>| {
            creating.done();
            match error {
                None => {
                    let _ = offline_open.try_set(false);
                }
                Some(e) => {
                    let _ = offline_error.try_set(Some(e));
                }
            }
        });
        actions.create_offline(offline_name.get_untracked(), done);
    };
    let ask_delete = Callback::new(move |profile: ProfileDto| {
        pending_delete.set(Some(profile));
        confirm_open.set(true);
    });

    // `/profiles?add=offline|microsoft` (the "Profile required" dialog) runs its action once.
    Effect::new(move |handled: Option<bool>| {
        if handled == Some(true) {
            return true;
        }
        match query.with(|q| q.get("add")).as_deref() {
            Some("offline") => {
                open_offline();
                true
            }
            Some("microsoft") => {
                actions.sign_in();
                true
            }
            _ => false,
        }
    });

    let add_buttons = move || {
        view! {
            <Button icon="person_add" on_click=move |_| open_offline()>
                {move || i18n.t("offline_account")}
            </Button>
            <Button
                variant=Variant::Primary
                icon="add"
                disabled=Signal::derive(signing_in)
                on_click=move |_| actions.sign_in()
            >
                {move || i18n.t("microsoft_account")}
            </Button>
        }
    };

    let page_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        let entries = vec![
            MenuEntry::item("offline", i18n.t("offline_account")).icon("person_add"),
            MenuEntry::item("microsoft", i18n.t("microsoft_account"))
                .icon("verified_user")
                .disabled(signing_in()),
        ];
        let on_select = move |id: String| match id.as_str() {
            "offline" => open_offline(),
            "microsoft" => actions.sign_in(),
            _ => {}
        };
        menu.open_with(ev.client_x(), ev.client_y(), entries, on_select);
    };

    view! {
        <PageHeader title_key="profile_title" actions=ViewFn::from(add_buttons) />
        <div class="profiles" on:contextmenu=page_menu>
            <Show
                when=move || store.profiles.with(|list| !list.is_empty())
                fallback=move || {
                    view! {
                        <EmptyState icon="person" title=t("profiles_empty") desc=t("empty_profiles_desc")>
                            <div class="wrap">{add_buttons()}</div>
                        </EmptyState>
                    }
                }
            >
                <div class="profiles__grid">
                    <For
                        each=move || store.profiles.get()
                        key=|p| (p.key.clone(), p.is_default, p.reauth_required, p.id.clone())
                        children=move |profile| view! { <ProfileCard profile=profile on_delete=ask_delete /> }
                    />
                </div>
            </Show>
        </div>
        <Dialog open=offline_open title=t("create_offline_profile") icon="person_add">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| offline_open.set(false)>
                    {move || i18n.t("cancel")}
                </Button>
                <Button variant=Variant::Primary icon="check" loading=creating.running() on_click=move |_| submit_offline()>
                    {move || i18n.t("create")}
                </Button>
            </DialogFooter>
            <Field label=t("enter_nickname") error=Signal::derive(move || offline_error.get().map(|e| i18n.error(&e)))>
                <TextInput
                    value=offline_name
                    autofocus=true
                    invalid=Signal::derive(move || offline_error.get().is_some())
                    on_enter=Callback::new(move |_| submit_offline())
                />
            </Field>
        </Dialog>
        <ConfirmDialog
            open=confirm_open
            danger=true
            title=t("confirmation")
            message=t("are_you_sure")
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=Callback::new(move |_| {
                if let Some(profile) = pending_delete.get_untracked() {
                    actions.delete(profile.key);
                }
            })
        />
    }
}

#[component]
fn ProfileCard(profile: ProfileDto, on_delete: Callback<ProfileDto>) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_profile_actions();
    let menu = use_context_menu();
    let signing_in = move || store.auth.get() != AuthState::Idle;
    let microsoft = profile.kind == AccountKind::Microsoft;
    let kind = profile.kind;
    let is_default = profile.is_default;
    let reauth = profile.reauth_required;
    let name = profile.name.clone();
    let key = profile.key.clone();
    let for_menu = profile.clone();
    let for_use = profile.clone();
    let for_delete = profile;

    let on_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        let profile = for_menu.clone();
        let mut entries = vec![
            MenuEntry::item("default", i18n.t("set_as_default")).icon("star").disabled(profile.is_default),
        ];
        if microsoft {
            entries.push(
                MenuEntry::item("reauth", i18n.t("profile_sign_in_again"))
                    .icon("login")
                    .disabled(signing_in()),
            );
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item("delete", i18n.t("delete")).icon("delete").danger());
        let on_select = move |id: String| match id.as_str() {
            "default" => actions.set_default(profile.clone()),
            "reauth" => actions.sign_in(),
            "delete" => on_delete.run(profile.clone()),
            _ => {}
        };
        menu.open_with(ev.client_x(), ev.client_y(), entries, on_select);
    };

    view! {
        <div class="profile" class:is-active=is_default on:contextmenu=on_menu>
            <ProfileAvatar profile_key=Signal::derive(move || Some(key.clone())) size=48 />
            <div class="profile__info">
                <div class="profile__name">{name}</div>
                <div class="profile__kind">
                    <Icon name=if microsoft { "verified_user" } else { "person" } size=14 />
                    <span>{move || i18n.t(kind_key(kind))}</span>
                </div>
                {reauth.then(|| view! { <div class="profile__reauth">{move || i18n.t("profile_reauth_required")}</div> })}
            </div>
            {is_default.then(|| view! { <span class="profile__badge">{move || i18n.t("profile_active")}</span> })}
            // Every card keeps its actions in one row at the bottom, whatever it offers.
            <div class="profile__actions">
                {(!is_default).then(|| view! {
                    <Button on_click=move |_| actions.set_default(for_use.clone())>
                        {move || i18n.t("profile_use")}
                    </Button>
                })}
                {microsoft.then(|| view! {
                    <Button
                        variant=if reauth { Variant::Danger } else { Variant::Secondary }
                        icon="login"
                        disabled=Signal::derive(signing_in)
                        on_click=move |_| actions.sign_in()
                    >
                        {move || i18n.t("profile_sign_in_again")}
                    </Button>
                })}
                <IconAction
                    icon="delete_outline"
                    tone=ActionTone::Danger
                    title=Signal::derive(move || i18n.t("delete"))
                    on_click=Callback::new(move |()| on_delete.run(for_delete.clone()))
                />
            </div>
        </div>
    }
}
