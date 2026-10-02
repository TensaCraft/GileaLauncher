//! The Play flow: a running build asks before a second copy,
//! `ask_profile_on_launch` picks the account (unless the build has its own), a missing account
//! opens "Profile required". The
//! `LaunchDialogs` host in `App` renders these dialogs for every page.

use launcher_shared::BuildDto;
use leptos::prelude::*;
use ui_kit::ConfirmDialog;
use ui_kit::i18n::use_i18n;

use super::actions::{BuildActions, use_build_actions};
use super::{LaunchStep, find_build, next_launch_step, own_profile};
use crate::profiles::launch::{LaunchProfileSelector, ProfileRequiredDialog};
use crate::store::{AppStore, use_store};

#[derive(Clone, Copy)]
pub struct LaunchFlow {
    store: AppStore,
    actions: BuildActions,
    /// The build waiting for a dialog's answer.
    pending: RwSignal<Option<BuildDto>>,
    /// A second copy of `pending` was confirmed.
    duplicate_ok: RwSignal<bool>,
    confirm_duplicate: RwSignal<bool>,
    pick_profile: RwSignal<bool>,
    need_profile: RwSignal<bool>,
}

impl LaunchFlow {
    /// Play on `build`.
    pub fn start(&self, build: BuildDto) {
        self.duplicate_ok.set(false);
        self.proceed(build);
    }

    /// `--launch-version=<id>`: `false` when no build has that key or version id.
    pub fn start_external(&self, id: &str) -> bool {
        match self.store.builds.with_untracked(|builds| find_build(builds, id).cloned()) {
            Some(build) => {
                self.start(build);
                true
            }
            None => false,
        }
    }

    fn proceed(&self, build: BuildDto) {
        let launching = self.store.launching.with_untracked(|set| set.contains(&build.key));
        let own = self.store.profiles.with_untracked(|profiles| own_profile(&build, profiles));
        let ask_profile = own.is_none() && self.store.settings.with_untracked(|s| s.ask_profile_on_launch);
        let profiles = self.store.profiles.with_untracked(Vec::len);
        let step = next_launch_step(
            launching,
            build.running,
            self.duplicate_ok.get_untracked(),
            ask_profile,
            profiles,
        );
        match step {
            LaunchStep::Ignore => {}
            LaunchStep::ConfirmDuplicate => {
                self.pending.set(Some(build));
                self.confirm_duplicate.set(true);
            }
            LaunchStep::NeedProfile => self.need_profile.set(true),
            LaunchStep::PickProfile => {
                self.pending.set(Some(build));
                self.pick_profile.set(true);
            }
            LaunchStep::Launch => self.launch(build, own),
        }
    }

    fn launch(&self, build: BuildDto, profile_key: Option<String>) {
        self.actions.launch(build, profile_key, self.duplicate_ok.get_untracked(), self.need_profile);
    }
}

pub fn provide_launch_flow() -> LaunchFlow {
    let flow = LaunchFlow {
        store: use_store(),
        actions: use_build_actions(),
        pending: RwSignal::new(None),
        duplicate_ok: RwSignal::new(false),
        confirm_duplicate: RwSignal::new(false),
        pick_profile: RwSignal::new(false),
        need_profile: RwSignal::new(false),
    };
    provide_context(flow);
    flow
}

pub fn use_launch_flow() -> LaunchFlow {
    expect_context::<LaunchFlow>()
}

#[component]
pub fn LaunchDialogs() -> impl IntoView {
    let flow = use_launch_flow();
    let i18n = use_i18n();
    let name =
        Signal::derive(move || flow.pending.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default()));
    let confirm = Callback::new(move |()| {
        flow.duplicate_ok.set(true);
        if let Some(build) = flow.pending.get_untracked() {
            flow.proceed(build);
        }
    });
    let pick = Callback::new(move |profile_key: String| {
        if let Some(build) = flow.pending.get_untracked() {
            flow.launch(build, Some(profile_key));
        }
    });
    view! {
        <ConfirmDialog
            open=flow.confirm_duplicate
            title=Signal::derive(move || i18n.tp("version_already_running_confirm_title", &[("version", name.get())]))
            message=Signal::derive(move || i18n.t("version_already_running_confirm_message"))
            confirm_label=Signal::derive(move || i18n.t("play"))
            cancel_label=Signal::derive(move || i18n.t("cancel"))
            on_confirm=confirm
        />
        <LaunchProfileSelector open=flow.pick_profile version=name on_pick=pick />
        <ProfileRequiredDialog open=flow.need_profile />
    }
}
