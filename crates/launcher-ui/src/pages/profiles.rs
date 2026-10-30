//! «Профілі»: a card per account with the player's whole figure (its switch at the figure's foot
//! makes it the active one), the account's kind and whether it needs a sign-in, its few buttons at
//! the card's right. The last card adds an account: a dialog asks which kind first.

use launcher_shared::{AccountKind, AppError, AuthState, ErrorCode, Level, ProfileDto};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ChoiceCards, ChoiceOption, ConfirmDialog, Dialog, DialogFooter, EmptyState, Field, Icon,
    InFlight, MenuEntry, Size, TextInput, Variant, use_context_menu, use_toasts,
};

use crate::modules::use_module_parts;
use crate::profiles::actions::{ServerSignIn, use_profile_actions};
use crate::profiles::avatar::ProfileBody;
use crate::profiles::kind_text;
use crate::shell::sidebar::{ModulePages, backend_modules};
use crate::shell::{PageHeader, copy_text};
use crate::store::use_store;

/// The kinds of account the add dialog offers: value, label key, icon, description key; the
/// modules' follow (`UiModule::account_kinds`).
pub const ACCOUNT_KINDS: [(&str, &str, &str, &str); 3] = [
    ("microsoft", "microsoft_account", "verified_user", "account_microsoft_desc"),
    ("offline", "offline_account", "person", "account_offline_desc"),
    ("elyby", "account_elyby", "public", "account_elyby_desc"),
];

/// The address of the skins page opened on `key`'s account (the key percent-encoded).
pub fn skins_path(key: &str) -> String {
    let encoded: String = key
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("/skins?account={encoded}")
}

#[component]
pub fn ProfilesPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_profile_actions();
    let menu = use_context_menu();
    let query = use_query_map();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let signing_in = move || store.auth.get() != AuthState::Idle;

    // The add dialog: which kind (`None`), then that kind's form.
    let add_open = RwSignal::new(false);
    let add_step = RwSignal::new(None::<&'static str>);
    let kind_choice = RwSignal::new(String::new());
    let offline_name = RwSignal::new(String::new());
    let offline_error = RwSignal::new(None::<AppError>);
    // A sign-in on another auth server: the login and password, and the second factor once the
    // server asks for it.
    let login = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let code_asked = RwSignal::new(false);
    let server_error = RwSignal::new(None::<AppError>);
    let confirm_open = RwSignal::new(false);
    let pending_delete = RwSignal::new(None::<ProfileDto>);

    // The skins page is there when its module is built in and its backend runs.
    let pages = StoredValue::new(use_context::<ModulePages>().unwrap_or_default().0);
    let skins = Signal::derive(move || {
        let backend = store.info.with(|i| backend_modules(i.as_ref()));
        pages.with_value(|p| {
            p.iter().any(|page| page.path == "/skins" && backend.iter().any(|m| m == page.module))
        })
    });

    // Every kind on offer: the core's, then the modules' whose backend runs.
    let parts = use_module_parts();
    let all_kinds = Signal::derive(move || {
        let mut kinds: Vec<(&'static str, &'static str, &'static str, &'static str)> = ACCOUNT_KINDS.to_vec();
        parts.with(|p| kinds.extend(p.account_kinds.iter().map(|k| (k.id, k.label, k.icon, k.desc))));
        kinds
    });
    let open_add = move || {
        kind_choice.set(String::new());
        add_step.set(None);
        add_open.set(true);
    };
    let to_offline = move || {
        offline_name.set(String::new());
        offline_error.set(None);
        add_step.set(Some("offline"));
        add_open.set(true);
    };
    let to_server = move |kind: &'static str| {
        for field in [login, password, code] {
            field.set(String::new());
        }
        code_asked.set(false);
        server_error.set(None);
        add_step.set(Some(kind));
    };
    let chosen = Callback::new(move |kind: String| match kind.as_str() {
        "microsoft" => {
            add_open.set(false);
            actions.sign_in();
        }
        "offline" => to_offline(),
        other => {
            if let Some(&(kind, ..)) =
                all_kinds.with_untracked(|k| k.iter().find(|k| k.0 == other).copied()).as_ref()
            {
                to_server(kind);
            }
        }
    });
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
                    let _ = add_open.try_set(false);
                }
                Some(e) => {
                    let _ = offline_error.try_set(Some(e));
                }
            }
        });
        actions.create_offline(offline_name.get_untracked(), done);
    };
    let signing = InFlight::new();
    let submit_server = move || {
        if !signing.start() {
            return;
        }
        let args = ServerSignIn {
            server: add_step.get_untracked().unwrap_or_default().to_string(),
            login: login.get_untracked(),
            password: password.get_untracked(),
            code: code_asked.get_untracked().then(|| code.get_untracked()),
        };
        let done = Callback::new(move |error: Option<AppError>| {
            signing.done();
            match error {
                None => {
                    let _ = add_open.try_set(false);
                }
                Some(e) => {
                    if e.code == ErrorCode::AuthCodeRequired {
                        let _ = code_asked.try_set(true);
                    }
                    let _ = server_error.try_set(Some(e));
                }
            }
        });
        actions.sign_in_server(args, done);
    };
    // An auth server's account whose session ended: its server's sign-in form again.
    let sign_in_again = Callback::new(move |server: String| {
        if let Some((kind, ..)) = all_kinds.with_untracked(|k| k.iter().find(|k| k.0 == server).copied()) {
            to_server(kind);
            add_open.set(true);
        }
    });
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
                to_offline();
                true
            }
            Some("microsoft") => {
                actions.sign_in();
                true
            }
            _ => false,
        }
    });

    let page_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        let entries = vec![MenuEntry::item("add", i18n.t("add_account")).icon("person_add")];
        let on_select = move |id: String| {
            if id == "add" {
                open_add();
            }
        };
        menu.open_with(ev.client_x(), ev.client_y(), entries, on_select);
    };

    let kinds = Signal::derive(move || {
        let busy = signing_in();
        all_kinds
            .get()
            .into_iter()
            .map(|(value, label, icon, desc)| {
                ChoiceOption::new(value, i18n.t(label), icon)
                    .with_desc(i18n.t(desc))
                    .with_disabled(value == "microsoft" && busy)
            })
            .collect::<Vec<_>>()
    });
    let dialog_title = Signal::derive(move || match add_step.get() {
        Some("offline") => i18n.t("create_offline_profile"),
        Some("elyby") => i18n.t("add_elyby_title"),
        Some(other) if other != "offline" => all_kinds.with(|k| {
            let label = k.iter().find(|k| k.0 == other).map_or(other, |k| k.1);
            i18n.tp("add_site_title", &[("name", i18n.t(label))])
        }),
        _ => i18n.t("add_account_title"),
    });

    view! {
        <PageHeader title_key="profile_title" />
        <div class="profiles" on:contextmenu=page_menu>
            <Show
                when=move || store.profiles.with(|list| !list.is_empty())
                fallback=move || {
                    view! {
                        <EmptyState icon="person" title=t("profiles_empty") desc=t("empty_profiles_desc")>
                            <Button variant=Variant::Primary icon="person_add" on_click=move |_| open_add()>
                                {move || i18n.t("add_account")}
                            </Button>
                        </EmptyState>
                    }
                }
            >
                <div class="profiles__grid">
                    <For
                        each=move || store.profiles.get()
                        key=|p| (p.key.clone(), p.is_default, p.reauth_required, p.id.clone())
                        children=move |profile| view! { <ProfileCard profile=profile skins=skins on_delete=ask_delete on_server=sign_in_again /> }
                    />
                    <button type="button" class="profile profile--add" on:click=move |_| open_add()>
                        <Icon name="person_add" size=28 />
                        <span>{move || i18n.t("add_account")}</span>
                    </button>
                </div>
            </Show>
        </div>
        <Dialog open=add_open title=dialog_title icon="person_add" wide=true>
            <DialogFooter slot>
                {move || match add_step.get() {
                    Some("offline") => view! {
                        <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| add_step.set(None)>
                            {move || i18n.t("back")}
                        </Button>
                        <Button variant=Variant::Primary icon="check" loading=creating.running() on_click=move |_| submit_offline()>
                            {move || i18n.t("create")}
                        </Button>
                    }
                    .into_any(),
                    Some(_) => view! {
                        <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| add_step.set(None)>
                            {move || i18n.t("back")}
                        </Button>
                        <Button variant=Variant::Primary icon="login" loading=signing.running() on_click=move |_| submit_server()>
                            {move || i18n.t("profile_sign_in")}
                        </Button>
                    }
                    .into_any(),
                    _ => view! {
                        <Button variant=Variant::Ghost on_click=move |_| add_open.set(false)>
                            {move || i18n.t("cancel")}
                        </Button>
                    }
                    .into_any(),
                }}
            </DialogFooter>
            {move || match add_step.get() {
                Some("offline") => view! {
                    <Field label=t("enter_nickname") error=Signal::derive(move || offline_error.get().map(|e| i18n.error(&e)))>
                        <TextInput
                            value=offline_name
                            autofocus=true
                            invalid=Signal::derive(move || offline_error.get().is_some())
                            on_enter=Callback::new(move |_| submit_offline())
                        />
                    </Field>
                }
                .into_any(),
                Some(kind) => {
                    let enter = Callback::new(move |_| submit_server());
                    let error = Signal::derive(move || server_error.get().map(|e| i18n.error(&e)));
                    view! {
                        <div class="sign-in" data-server=kind>
                            <Field label=t("auth_login")>
                                <TextInput value=login autofocus=true on_enter=enter />
                            </Field>
                            <Field label=t("auth_password")>
                                <TextInput value=password secret=true on_enter=enter />
                            </Field>
                            <Show when=move || code_asked.get()>
                                <Field label=t("auth_code")>
                                    <TextInput value=code autofocus=true placeholder="123456" on_enter=enter />
                                </Field>
                            </Show>
                            {move || error.get().map(|text| view! { <div class="error"><Icon name="error_outline" size=14 />{text}</div> })}
                        </div>
                    }
                    .into_any()
                }
                _ => view! {
                    <ChoiceCards
                        options=kinds
                        value=kind_choice
                        on_change=chosen
                        columns={if all_kinds.with(Vec::len) > 3 { 2 } else { 3 }}
                        label=t("add_account_title")
                    />
                }
                .into_any(),
            }}
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
fn ProfileCard(
    profile: ProfileDto,
    skins: Signal<bool>,
    on_delete: Callback<ProfileDto>,
    /// Opens the sign-in form of an auth server's account whose session ended (its server's id).
    on_server: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let actions = use_profile_actions();
    let menu = use_context_menu();
    let signing_in = move || store.auth.get() != AuthState::Idle;
    let microsoft = profile.kind == AccountKind::Microsoft;
    let server = matches!(profile.kind, AccountKind::Yggdrasil | AccountKind::Module);
    let shown = profile.clone();
    let server_id = profile.server.clone().unwrap_or_default();
    let is_default = profile.is_default;
    let reauth = profile.reauth_required;
    let name = profile.name.clone();
    let key = profile.key.clone();
    let for_use = profile.clone();
    let for_skin = profile.key.clone();

    let copy = move |text: String| {
        spawn_local(async move {
            if copy_text(text).await {
                toasts.show(Level::Success, i18n.t("profile_copied"), None);
            } else {
                toasts.show(Level::Error, i18n.t("microsoft_auth_copy_failed"), None);
            }
        });
    };
    // The card's menu: from its `⋮` button and from a right click on it.
    let open_menu = {
        let profile = profile.clone();
        move |x: i32, y: i32| {
            let profile = profile.clone();
            let mut entries = vec![
                MenuEntry::item("default", i18n.t("set_as_default"))
                    .icon("star")
                    .disabled(profile.is_default),
                MenuEntry::item("copy_name", i18n.t("profile_copy_name")).icon("content_copy"),
                MenuEntry::item("copy_uuid", i18n.t("profile_copy_uuid")).icon("fingerprint"),
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
                "copy_name" => copy(profile.name.clone()),
                "copy_uuid" => copy(profile.id.clone()),
                "reauth" => actions.sign_in(),
                "delete" => on_delete.run(profile.clone()),
                _ => {}
            };
            menu.open_with(x, y, entries, on_select);
        }
    };
    let on_context = {
        let open_menu = open_menu.clone();
        move |ev: MouseEvent| {
            ev.prevent_default();
            ev.stop_propagation();
            open_menu(ev.client_x(), ev.client_y());
        }
    };
    let on_more = move |ev: MouseEvent| {
        ev.stop_propagation();
        open_menu(ev.client_x(), ev.client_y());
    };
    let on_pick = move |_| actions.set_default(for_use.clone());

    view! {
        <div class="profile" class:is-active=is_default on:contextmenu=on_context>
            <ProfileBody profile_key=key />
            <div class="profile__main">
                <div class="profile__head">
                    <div class="profile__name">{name}</div>
                    {is_default.then(|| view! { <span class="profile__badge">{move || i18n.t("profile_active")}</span> })}
                </div>
                <div class="profile__line">
                    <Icon name=if microsoft { "verified_user" } else if server { "dns" } else { "person" } size=14 />
                    <span>{move || kind_text(&shown, |k| i18n.t(k))}</span>
                </div>
                {(microsoft || server).then(|| {
                    let (class, text) = if reauth {
                        ("profile__line is-warn", "profile_session_expired")
                    } else {
                        ("profile__line is-ok", "profile_session_ok")
                    };
                    view! {
                        <div class=class>
                            <span class="profile__dot"></span>
                            <span>{move || i18n.t(text)}</span>
                        </div>
                    }
                })}
                // Every card keeps its buttons in one row at its right edge, whatever it offers.
                <div class="profile__actions">
                    // The active one says so with its tag; another offers to become it, at the row's
                    // left edge: the buttons at the right stay where they are on every card.
                    {(!is_default).then(|| view! {
                        <Button size=Size::Sm icon="check" class="profile__pick" on_click=on_pick>
                            {move || i18n.t("profile_use")}
                        </Button>
                    })}
                    {move || (microsoft && skins.get()).then(|| {
                        let key = for_skin.clone();
                        view! {
                            <Button size=Size::Sm icon="brand:skin" on_click=move |_| store.go(&skins_path(&key))>
                                {move || i18n.t("profile_change_skin")}
                            </Button>
                        }
                    })}
                    {(server && reauth).then(|| {
                        let id = server_id.clone();
                        view! {
                            <Button size=Size::Sm variant=Variant::Danger icon="login" on_click=move |_| on_server.run(id.clone())>
                                {move || i18n.t("profile_sign_in")}
                            </Button>
                        }
                    })}
                    {(microsoft && reauth).then(|| view! {
                        <Button
                            size=Size::Sm
                            variant=Variant::Danger
                            icon="login"
                            disabled=Signal::derive(signing_in)
                            on_click=move |_| actions.sign_in()
                        >
                            {move || i18n.t("profile_sign_in")}
                        </Button>
                    })}
                    // A button like the others of the row, as tall and as framed, only square.
                    <button
                        type="button"
                        class="btn btn--secondary btn--sm profile__more"
                        aria-label=move || i18n.t("profile_more")
                        data-tip=move || i18n.t("profile_more")
                        data-tip-side="top"
                        on:click=on_more
                    >
                        <Icon name="more_vert" />
                    </button>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_skin_opens_the_skins_page_on_the_account() {
        assert_eq!(skins_path("Steve"), "/skins?account=Steve");
        assert_eq!(skins_path("ely.by:Alex"), "/skins?account=ely.by%3AAlex", "as the skins page reads it");
    }

    #[test]
    fn the_add_dialog_offers_each_kind_with_its_texts() {
        let values: Vec<&str> = ACCOUNT_KINDS.iter().map(|k| k.0).collect();
        assert_eq!(values, ["microsoft", "offline", "elyby"], "no server of one's own (owner, 2026-10-30)");
        for (_, label, _, desc) in ACCOUNT_KINDS {
            assert!(desc.ends_with("_desc") && !label.is_empty());
        }
    }
}
