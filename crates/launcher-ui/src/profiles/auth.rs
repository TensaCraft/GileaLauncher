//! The device code dialog: shown while `app://auth` is `DeviceCode`.

use launcher_shared::{AuthState, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Variant, use_toasts};

use super::actions::use_profile_actions;
use crate::shell::{copy_text, use_url_opener};
use crate::store::use_store;

/// Which sign-in dialog the current stage calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInDialog {
    None,
    Browser,
    DeviceCode,
}

pub fn sign_in_dialog(state: &AuthState) -> SignInDialog {
    match state {
        AuthState::Browser => SignInDialog::Browser,
        AuthState::DeviceCode { .. } => SignInDialog::DeviceCode,
        AuthState::Idle | AuthState::Finishing => SignInDialog::None,
    }
}

/// While the Microsoft page is open in the browser: explains the wait and lets the user give up
/// (after closing the tab the buttons would otherwise stay disabled for up to three minutes).
#[component]
pub fn BrowserWaitDialog() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let actions = use_profile_actions();
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(sign_in_dialog(&store.auth.get()) == SignInDialog::Browser));
    view! {
        <Dialog
            open=open
            title=Signal::derive(move || i18n.t("microsoft_auth_starting"))
            icon="open_in_browser"
            on_close=Callback::new(move |_| actions.cancel_sign_in())
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| actions.cancel_sign_in()>{move || i18n.t("cancel")}</Button>
            </DialogFooter>
            <p style="margin:0">{move || i18n.t("microsoft_auth_browser_opening")}</p>
        </Dialog>
    }
}

#[component]
pub fn DeviceCodeDialog() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let toasts = use_toasts();
    let actions = use_profile_actions();
    let opener = use_url_opener();
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(sign_in_dialog(&store.auth.get()) == SignInDialog::DeviceCode));
    let field =
        move |pick: fn(&AuthState) -> Option<String>| move || pick(&store.auth.get()).unwrap_or_default();
    let code = field(|s| match s {
        AuthState::DeviceCode { user_code, .. } => Some(user_code.clone()),
        _ => None,
    });
    let uri = field(|s| match s {
        AuthState::DeviceCode { verification_uri, .. } => Some(verification_uri.clone()),
        _ => None,
    });
    let open_url = field(|s| match s {
        AuthState::DeviceCode { open_url, .. } => Some(open_url.clone()),
        _ => None,
    });
    let copy = move |text: String, done_key: &'static str| {
        spawn_local(async move {
            if copy_text(text).await {
                toasts.show(Level::Success, i18n.t(done_key), None);
            } else {
                toasts.show(Level::Error, i18n.t("microsoft_auth_copy_failed"), None);
            }
        });
    };
    view! {
        <Dialog
            open=open
            title=Signal::derive(move || i18n.t("microsoft_auth_device_code_title"))
            icon="lock"
            on_close=Callback::new(move |_| actions.cancel_sign_in())
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost icon="link" on_click=move |_| copy(uri(), "microsoft_auth_url_copied")>
                    {move || i18n.t("microsoft_auth_device_code_copy_url")}
                </Button>
                <Button variant=Variant::Ghost on_click=move |_| actions.cancel_sign_in()>{move || i18n.t("cancel")}</Button>
            </DialogFooter>
            <p style="margin:0">{move || i18n.t("microsoft_auth_device_code_waiting")}</p>
            <p class="hint" style="margin:0">{move || i18n.t("microsoft_auth_device_code_browser_hint")}</p>
            <div class="device__label">{move || i18n.t("microsoft_auth_device_code_code_label")}</div>
            <div class="device__code">{code}</div>
            <div class="wrap">
                <Button icon="content_copy" on_click=move |_| copy(code(), "microsoft_auth_code_copied")>
                    {move || i18n.t("microsoft_auth_device_code_copy_code")}
                </Button>
                <Button variant=Variant::Primary icon="open_in_new" on_click=move |_| opener.open(open_url())>
                    {move || i18n.t("microsoft_auth_device_code_open_url")}
                </Button>
            </div>
            <div class="device__label">{move || i18n.t("microsoft_auth_device_code_url_label")}</div>
            <div class="device__url">{uri}</div>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_sign_in_stage_has_its_dialog() {
        assert_eq!(sign_in_dialog(&AuthState::Browser), SignInDialog::Browser);
        let device = AuthState::DeviceCode {
            user_code: "A".into(),
            verification_uri: "https://www.microsoft.com/link".into(),
            open_url: "https://www.microsoft.com/link?otc=A".into(),
        };
        assert_eq!(sign_in_dialog(&device), SignInDialog::DeviceCode);
        assert_eq!(sign_in_dialog(&AuthState::Idle), SignInDialog::None);
        assert_eq!(sign_in_dialog(&AuthState::Finishing), SignInDialog::None);
    }
}
