//! «Профілі»: a card per account with the player's whole figure (its switch at the figure's foot
//! makes it the active one), the account's kind and whether it needs a sign-in, its few buttons at
//! the card's right. The last card adds an account: a dialog asks which kind first.

use launcher_shared::{AccountKind, AuthState, Level, ProfileDto};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ConfirmDialog, EmptyState, Icon, MenuEntry, Size, Variant, use_context_menu, use_toasts,
};

use crate::profiles::actions::use_profile_actions;
use crate::profiles::add::use_add_account;
use crate::profiles::avatar::ProfileBody;
use crate::profiles::{kind_text, skin_site};
use crate::shell::sidebar::{ModulePages, backend_modules};
use crate::shell::{PageHeader, copy_text, use_url_opener};
use crate::store::use_store;

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
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

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

    // «Додати акаунт» is the app's one dialog (`profiles::add`).
    let add = use_add_account();
    let open_add = move || add.open();
    // An auth server's account whose session ended: its server's sign-in form again.
    let sign_in_again = Callback::new(move |server: String| add.sign_in_again(server));
    let ask_delete = Callback::new(move |profile: ProfileDto| {
        pending_delete.set(Some(profile));
        confirm_open.set(true);
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
    let signing_in = move || store.auth.get() != AuthState::Idle;
    let toasts = use_toasts();
    let actions = use_profile_actions();
    let menu = use_context_menu();
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
    let site = skin_site(&profile);
    let opener = use_url_opener();

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
                            // Just its icon (owner, 2026-10-30): the card's buttons keep one row however narrow it is.
                            <Button
                                size=Size::Sm
                                icon="brand:skin"
                                class="profile__more"
                                title=Signal::derive(move || i18n.t("profile_change_skin"))
                                on_click=move |_| store.go(&skins_path(&key))
                            />
                        }
                    })}
                    {site.map(|url| view! {
                        <Button
                            size=Size::Sm
                            icon="brand:skin"
                            class="profile__more"
                            title=Signal::derive(move || i18n.t("profile_change_skin_site"))
                            on_click=move |_| opener.open(url.to_string())
                        />
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
}
