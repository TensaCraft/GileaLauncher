//! Account pickers used when launching. The Play button opens
//! `ProfileRequiredDialog` when there are no profiles and `LaunchProfileSelector` when
//! `ask_profile_on_launch` is on.

use launcher_shared::{ProfileDto, sort_profiles};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Icon, Variant};

use super::avatar::ProfileAvatar;
use super::{kind_key, kind_line};
use crate::store::use_store;

/// Default first, then by name.
pub fn launch_rows(list: &[ProfileDto]) -> Vec<ProfileDto> {
    let mut rows = list.to_vec();
    sort_profiles(&mut rows);
    rows
}

#[component]
pub fn ProfileRequiredDialog(open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let go = move |action: &'static str| {
        open.set(false);
        store.go(&format!("/profiles?add={action}"));
    };
    view! {
        <Dialog open=open title=Signal::derive(move || i18n.t("profile_required_title")) icon="person_off">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
                <Button icon="person_add" on_click=move |_| go("offline")>{move || i18n.t("offline_account")}</Button>
                <Button variant=Variant::Primary icon="verified_user" on_click=move |_| go("microsoft")>
                    {move || i18n.t("microsoft_account")}
                </Button>
            </DialogFooter>
            <p style="margin:0">{move || i18n.t("profile_required_message")}</p>
        </Dialog>
    }
}

#[component]
pub fn LaunchProfileSelector(
    open: RwSignal<bool>,
    #[prop(into)] version: Signal<String>,
    on_pick: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    view! {
        <Dialog open=open title=Signal::derive(move || i18n.t("launch_profile_select_title")) icon="sports_esports">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
            </DialogFooter>
            <p style="margin:0">
                {move || i18n.tp("launch_profile_select_message", &[("version", version.get())])}
            </p>
            <div class="pick">
                <For
                    each=move || launch_rows(&store.profiles.get())
                    key=|p| (p.key.clone(), p.is_default)
                    children=move |p| {
                        let pick_key = p.key.clone();
                        let avatar_key = p.key.clone();
                        let (kind, is_default) = (p.kind, p.is_default);
                        view! {
                            <button
                                type="button"
                                class="pick__row"
                                on:click=move |_| {
                                    open.set(false);
                                    on_pick.run(pick_key.clone());
                                }
                            >
                                <ProfileAvatar profile_key=Signal::derive(move || Some(avatar_key.clone())) size=32 />
                                <span class="pick__text">
                                    <span class="pick__name">{p.name.clone()}</span>
                                    <span class="pick__kind">
                                        {move || kind_line(i18n.t(kind_key(kind)), is_default.then(|| i18n.t("default")))}
                                    </span>
                                </span>
                                <Icon name="chevron_right" />
                            </button>
                        }
                    }
                />
            </div>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::AccountKind;

    fn p(name: &str, is_default: bool) -> ProfileDto {
        ProfileDto {
            key: name.into(),
            name: name.into(),
            id: String::new(),
            kind: AccountKind::Offline,
            is_default,
            reauth_required: false,
            reauth_reason: None,
        }
    }

    #[test]
    fn launch_rows_put_the_default_first() {
        let rows = launch_rows(&[p("zed", false), p("Mia", true), p("alex", false)]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Mia", "alex", "zed"]);
    }
}
