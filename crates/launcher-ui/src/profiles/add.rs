//! «Додати акаунт», one dialog for the whole app: which kind first, then that kind's form. The
//! Profiles page opens it, and so do a launch with no profile, the sidebar's profile button
//! (right-click with none yet) and the skins page.

use launcher_shared::{AppError, AuthState, ErrorCode};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ChoiceCards, ChoiceOption, Dialog, DialogFooter, Field, Icon, InFlight, TextInput, Variant,
};

use crate::modules::use_module_parts;
use crate::profiles::actions::{ServerSignIn, use_profile_actions};
use crate::store::use_store;

/// The kinds of account the add dialog offers: value, label key, icon, description key; the
/// modules' follow (`UiModule::account_kinds`).
pub const ACCOUNT_KINDS: [(&str, &str, &str, &str); 3] = [
    ("microsoft", "microsoft_account", "verified_user", "account_microsoft_desc"),
    ("offline", "offline_account", "person", "account_offline_desc"),
    ("elyby", "account_elyby", "public", "account_elyby_desc"),
];

/// Opens the dialog: on its kinds, on a launch that found no profile (it says why), or on an auth
/// server's form to sign in again.
#[derive(Clone, Copy)]
pub struct AddAccount {
    open: RwSignal<bool>,
    /// The kind whose form shows; `None`: the kinds.
    step: RwSignal<Option<&'static str>>,
    /// Translation key of the line under the title (why it opened); none for a plain «Додати».
    reason: RwSignal<Option<&'static str>>,
    /// An auth server asked for by id (a sign-in again), found among the kinds once mounted.
    asked_server: RwSignal<Option<String>>,
}

impl AddAccount {
    /// The kinds to choose from.
    pub fn open(&self) {
        self.reason.set(None);
        self.step.set(None);
        self.open.set(true);
    }

    /// The kinds, saying a launch needs a profile first.
    pub fn open_for_launch(&self) {
        self.step.set(None);
        self.reason.set(Some("profile_required_message"));
        self.open.set(true);
    }

    /// The sign-in form of auth server `server` (an account whose session ended there).
    pub fn sign_in_again(&self, server: String) {
        self.reason.set(None);
        self.asked_server.set(Some(server));
        self.open.set(true);
    }
}

pub fn provide_add_account() -> AddAccount {
    let add = AddAccount {
        open: RwSignal::new(false),
        step: RwSignal::new(None),
        reason: RwSignal::new(None),
        asked_server: RwSignal::new(None),
    };
    provide_context(add);
    add
}

pub fn use_add_account() -> AddAccount {
    expect_context::<AddAccount>()
}

#[component]
pub fn AddAccountDialog() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let actions = use_profile_actions();
    let add = use_add_account();
    let (add_open, add_step) = (add.open, add.step);
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let signing_in = move || store.auth.get() != AuthState::Idle;

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

    // Every kind on offer: the core's, then the modules' whose backend runs.
    let parts = use_module_parts();
    let all_kinds = Signal::derive(move || {
        let mut kinds: Vec<(&'static str, &'static str, &'static str, &'static str)> = ACCOUNT_KINDS.to_vec();
        parts.with(|p| kinds.extend(p.account_kinds.iter().map(|k| (k.id, k.label, k.icon, k.desc))));
        kinds
    });
    let to_offline = move || {
        offline_name.set(String::new());
        offline_error.set(None);
        add_step.set(Some("offline"));
    };
    let to_server = move |kind: &'static str| {
        for field in [login, password, code] {
            field.set(String::new());
        }
        code_asked.set(false);
        server_error.set(None);
        add_step.set(Some(kind));
    };
    // Each opening starts with nothing chosen; a sign-in again goes to its server's form.
    Effect::new(move |_| {
        if !add_open.get() {
            return;
        }
        kind_choice.set(String::new());
        if let Some(server) = add.asked_server.get_untracked() {
            add.asked_server.set(None);
            match all_kinds.with_untracked(|k| k.iter().find(|k| k.0 == server).map(|k| k.0)) {
                Some(kind) => to_server(kind),
                None => add_step.set(None),
            }
        }
    });
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
        // The answer may come after the dialog is gone.
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
                // Signed in: the password and code are not kept in the form.
                None => {
                    let _ = password.try_set(String::new());
                    let _ = code.try_set(String::new());
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
        Some(other) => all_kinds.with(|k| {
            let label = k.iter().find(|k| k.0 == other).map_or(other, |k| k.1);
            i18n.tp("add_site_title", &[("name", i18n.t(label))])
        }),
        None => i18n.t("add_account_title"),
    });
    let subtitle = Signal::derive(move || {
        add_step.with(Option::is_none).then(|| add.reason.get().map(|key| i18n.t(key))).flatten()
    });

    view! {
        <Dialog open=add_open title=dialog_title subtitle=subtitle icon="person_add" wide=true>
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
                    None => view! {
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
                None => view! {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_add_dialog_offers_each_kind_with_its_texts() {
        let values: Vec<&str> = ACCOUNT_KINDS.iter().map(|k| k.0).collect();
        assert_eq!(values, ["microsoft", "offline", "elyby"], "no server of one's own (owner, 2026-10-30)");
        for (_, label, _, desc) in ACCOUNT_KINDS {
            assert!(desc.ends_with("_desc") && !label.is_empty());
        }
    }

    /// A launch with no profile opens the kinds and says why; a plain opening says nothing.
    #[test]
    fn a_launch_without_a_profile_opens_the_kinds_and_says_why() {
        let owner = Owner::new();
        owner.with(|| {
            let add = provide_add_account();
            add.step.set(Some("offline"));
            add.open_for_launch();
            assert!(add.open.get_untracked());
            assert_eq!(add.step.get_untracked(), None, "the kinds first");
            assert_eq!(add.reason.get_untracked(), Some("profile_required_message"));
            add.open();
            assert_eq!(add.reason.get_untracked(), None);
            add.sign_in_again("elyby".into());
            assert_eq!(add.asked_server.get_untracked().as_deref(), Some("elyby"));
        });
    }
}
