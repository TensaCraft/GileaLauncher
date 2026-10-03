//! The Play flow: a running build asks before a second copy,
//! `ask_profile_on_launch` picks the account (unless the build has its own), a missing account
//! opens "Profile required", too little free memory beside a running game asks first. The
//! `LaunchDialogs` host in `App` renders these dialogs for every page.

use launcher_shared::recent::Join;
use launcher_shared::{AppError, BuildDto};
use leptos::prelude::*;
use ui_kit::ConfirmDialog;
use ui_kit::i18n::use_i18n;

use super::actions::{BuildActions, LaunchAsks, use_build_actions};
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
    /// The account `pending` starts with, kept while the memory question is open.
    pending_profile: RwSignal<Option<String>>,
    /// Starting with little free memory was confirmed.
    memory_ok: RwSignal<bool>,
    /// Why memory may run short (the backend's words), and the question about it.
    low_memory: RwSignal<Option<AppError>>,
    confirm_memory: RwSignal<bool>,
    /// Where `pending` takes the player (Home's «Грати»), kept while its questions are open.
    join: RwSignal<Option<Join>>,
    /// Opens the memory question. Made with the flow, not per launch: a shortcut's launch starts
    /// from an effect whose run is dropped before the backend answers.
    ask_memory: Callback<AppError>,
}

impl LaunchFlow {
    /// Play on `build`.
    pub fn start(&self, build: BuildDto) {
        self.start_into(build, None);
    }

    /// Play on `build` straight into `join`, a server or a world.
    pub fn start_into(&self, build: BuildDto, join: Option<Join>) {
        self.duplicate_ok.set(false);
        self.memory_ok.set(false);
        self.join.set(join);
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

    /// What this launch was allowed, and where its questions go.
    fn asks(&self) -> LaunchAsks {
        LaunchAsks {
            allow_duplicate: self.duplicate_ok.get_untracked(),
            allow_low_memory: self.memory_ok.get_untracked(),
            need_profile: self.need_profile,
            low_memory: self.ask_memory,
            join: self.join.get_untracked(),
        }
    }

    fn launch(&self, build: BuildDto, profile_key: Option<String>) {
        self.pending.set(Some(build.clone()));
        self.pending_profile.set(profile_key.clone());
        self.actions.launch(build, profile_key, self.asks());
    }
}

pub fn provide_launch_flow() -> LaunchFlow {
    let (low_memory, confirm_memory) = (RwSignal::new(None), RwSignal::new(false));
    let flow = LaunchFlow {
        store: use_store(),
        actions: use_build_actions(),
        pending: RwSignal::new(None),
        duplicate_ok: RwSignal::new(false),
        confirm_duplicate: RwSignal::new(false),
        pick_profile: RwSignal::new(false),
        need_profile: RwSignal::new(false),
        pending_profile: RwSignal::new(None),
        memory_ok: RwSignal::new(false),
        low_memory,
        confirm_memory,
        join: RwSignal::new(None),
        ask_memory: Callback::new(move |e: AppError| {
            low_memory.set(Some(e));
            confirm_memory.set(true);
        }),
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
    // Little free memory beside a running game: asked, then started as before.
    let memory_text = Signal::derive(move || {
        flow.low_memory.with(|e| e.as_ref().map(|e| i18n.error(e)).unwrap_or_default())
    });
    let start_anyway = Callback::new(move |()| {
        flow.memory_ok.set(true);
        if let Some(build) = flow.pending.get_untracked() {
            flow.launch(build, flow.pending_profile.get_untracked());
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
        <ConfirmDialog
            open=flow.confirm_memory
            title=Signal::derive(move || i18n.t("version_low_memory_confirm_title"))
            message=memory_text
            confirm_label=Signal::derive(move || i18n.t("launch_anyway"))
            cancel_label=Signal::derive(move || i18n.t("cancel"))
            on_confirm=start_anyway
        />
        <LaunchProfileSelector open=flow.pick_profile version=name on_pick=pick />
        <ProfileRequiredDialog open=flow.need_profile />
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::ErrorCode;
    use ui_kit::i18n::{Dictionary, I18n, provide_i18n};
    use ui_kit::provide_toasts;

    use super::*;
    use crate::store::provide_store;

    #[test]
    fn a_shortcut_s_launch_still_asks_about_memory() {
        let app = Owner::new();
        let flow = app.with(|| {
            provide_i18n(I18n::new("uk_UA", Dictionary::default(), Dictionary::default()));
            provide_toasts();
            provide_store();
            provide_launch_flow()
        });
        // A shortcut's build is played from an effect that runs again at once, dropping what its
        // run made, while the backend still answers.
        let effect_run = app.with(Owner::new);
        let asks = effect_run.with(|| flow.asks());
        effect_run.cleanup();
        let _ = asks.low_memory.try_run(AppError::new(ErrorCode::LowMemory, "6 GiB free"));
        assert!(flow.confirm_memory.get_untracked(), "the memory question opens");
        assert!(flow.low_memory.with_untracked(Option::is_some));
        app.cleanup();
    }

    #[test]
    fn home_s_play_carries_its_server_through_the_questions() {
        let app = Owner::new();
        let flow = app.with(|| {
            provide_i18n(I18n::new("uk_UA", Dictionary::default(), Dictionary::default()));
            provide_toasts();
            provide_store();
            provide_launch_flow()
        });
        let server = Join::Server { host: "tensa.co.ua".into(), port: 25565 };
        flow.join.set(Some(server.clone()));
        assert_eq!(flow.asks().join, Some(server), "asked again after a question, it still goes there");
        flow.join.set(None);
        assert_eq!(flow.asks().join, None);
        app.cleanup();
    }
}
