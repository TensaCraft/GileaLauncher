use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{BTreeMap, BTreeSet};

use launcher_shared::{
    ActivityEntry, Alert, AppInfo, AuthState, BuildDto, Level, OpsSnapshot, ProfileDto, SettingUpdate,
    SettingsSnapshot, SetupState, UpdateStatus,
};
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Toasts, ipc, use_toasts};

#[derive(Clone, Copy)]
pub struct AppStore {
    pub info: RwSignal<Option<AppInfo>>,
    pub settings: RwSignal<SettingsSnapshot>,
    pub ops: RwSignal<OpsSnapshot>,
    /// Newest first, at most 100 entries.
    pub activity: RwSignal<Vec<ActivityEntry>>,
    /// Launcher self-update state (`app://update`).
    pub update: RwSignal<Option<UpdateStatus>>,
    /// Accounts, default first (`app://profiles`).
    pub profiles: RwSignal<Vec<ProfileDto>>,
    /// `profiles` holds the backend's answer (no hint to add one before).
    pub profiles_loaded: RwSignal<bool>,
    /// Every build, sorted by name (`app://builds`).
    pub builds: RwSignal<Vec<BuildDto>>,
    /// `builds` holds the backend's answer (external launches wait for it).
    pub builds_loaded: RwSignal<bool>,
    /// Keys of builds whose launch is still being started.
    pub launching: RwSignal<BTreeSet<String>>,
    /// Microsoft sign-in progress (`app://auth`).
    pub auth: RwSignal<AuthState>,
    /// Avatar `src` per profile key; `None` while loading or when there is none.
    pub avatars: RwSignal<BTreeMap<String, Option<String>>>,
    pub alert: RwSignal<Option<Alert>>,
    pub setup: RwSignal<Option<SetupState>>,
    pub pending_launch: RwSignal<Option<String>>,
    /// Navigation request handled by `Shell` (keeps router closures out of `Send + Sync` callbacks).
    pub nav_to: RwSignal<Option<String>>,
    pub ready: RwSignal<bool>,
}

impl AppStore {
    pub fn go(&self, path: &str) {
        self.nav_to.set(Some(path.to_string()));
    }

    /// Shows `arrived` unless a later snapshot of the operations is shown already (the one asked
    /// for at start and their events may arrive in any order).
    pub fn show_ops(&self, arrived: OpsSnapshot) {
        self.ops.maybe_update(|shown| {
            let newer = arrived.replaces(shown);
            if newer {
                *shown = arrived;
            }
            newer
        });
    }

    /// Shows `arrived` unless a later snapshot is shown already (answers and the backend's
    /// announcements of changes saved side by side may arrive in any order).
    pub fn show_settings(&self, arrived: SettingsSnapshot) {
        self.settings.maybe_update(|shown| {
            let newer = arrived.replaces(shown);
            if newer {
                *shown = arrived;
            }
            newer
        });
    }
}

pub fn provide_store() -> AppStore {
    let store = AppStore {
        info: RwSignal::new(None),
        settings: RwSignal::new(SettingsSnapshot {
            compact_sidebar: true,
            click_sound_enabled: true,
            ..Default::default()
        }),
        ops: RwSignal::new(OpsSnapshot::default()),
        activity: RwSignal::new(Vec::new()),
        update: RwSignal::new(None),
        profiles: RwSignal::new(Vec::new()),
        profiles_loaded: RwSignal::new(false),
        builds: RwSignal::new(Vec::new()),
        builds_loaded: RwSignal::new(false),
        launching: RwSignal::new(BTreeSet::new()),
        auth: RwSignal::new(AuthState::Idle),
        avatars: RwSignal::new(BTreeMap::new()),
        alert: RwSignal::new(None),
        setup: RwSignal::new(None),
        pending_launch: RwSignal::new(None),
        nav_to: RwSignal::new(None),
        ready: RwSignal::new(false),
    };
    provide_context(store);
    store
}

pub fn use_store() -> AppStore {
    expect_context::<AppStore>()
}

#[derive(serde::Serialize)]
struct UpdateArgs {
    update: SettingUpdate,
}

/// Persists settings. Created during component setup (contexts are resolved there, never inside
/// event handlers, which run without a reactive owner).
#[derive(Clone, Copy)]
pub struct SettingsWriter {
    store: AppStore,
    toasts: Toasts,
    i18n: I18nCtx,
}

impl SettingsWriter {
    /// Persists one setting; the backend echoes the new snapshot. Errors become a toast.
    pub fn apply(&self, update: SettingUpdate) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            match ipc::invoke::<_, SettingsSnapshot>("settings_set", &UpdateArgs { update }).await {
                Ok(snapshot) => store.show_settings(snapshot),
                Err(err) => toasts.show(Level::Error, i18n.error(&err), None),
            }
        });
    }
}

pub fn use_settings_writer() -> SettingsWriter {
    SettingsWriter { store: use_store(), toasts: use_toasts(), i18n: use_i18n() }
}
