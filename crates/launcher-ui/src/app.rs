use launcher_shared::{
    ActivityEntry, Alert, AppInfo, AuthState, BuildsSnapshot, ExternalLaunch, GameEvent, Level, OpsSnapshot,
    ProfilesSnapshot, SettingsSnapshot, SetupState, Text, Toast, UpdateStatus, names,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use ui_kit::i18n::{build_i18n, provide_i18n};
use ui_kit::menu::ContextMenuHost;
use ui_kit::toast::duration_for;
use ui_kit::{TipLayer, ToastItem, Toaster, ipc, provide_context_menu, provide_toasts, sound};

use crate::builds::actions::use_build_actions;
use crate::builds::dialogs::{BuildDialogsHost, provide_build_dialogs};
use crate::builds::launch::{LaunchDialogs, provide_launch_flow};
use crate::pages::builds::BuildsPage;
use crate::pages::components::ComponentsPage;
use crate::pages::content::ContentRoute;
use crate::pages::create::CreateBuildPage;
use crate::pages::home::HomePage;
use crate::pages::kit::KitPage;
use crate::pages::modpacks::ModpacksPage;
use crate::pages::profiles::ProfilesPage;
use crate::pages::settings::SettingsPage;
use crate::pages::setup::SetupPage;
use crate::profiles::auth::{BrowserWaitDialog, DeviceCodeDialog};
use crate::shell::{
    AlertHost, PageHeader, QuitConfirm, Shell, SupportDialog, provide_header, provide_support,
};
use crate::store::{provide_store, use_store};
use crate::update::UpdateDialogs;

pub fn core_locale(lang: &str) -> Option<&'static str> {
    match lang {
        "uk_UA" => Some(include_str!("../../../assets/langs/uk_UA.json")),
        "en_US" => Some(include_str!("../../../assets/langs/en_US.json")),
        _ => None,
    }
}

fn browser_lang() -> String {
    let lang = web_sys::window().and_then(|w| w.navigator().language()).unwrap_or_default();
    if lang.to_lowercase().starts_with("uk") { "uk_UA".into() } else { "en_US".into() }
}

/// The page's title in the launcher's language: what the app is, not a brand.
fn set_document_title(title: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        doc.set_title(title);
    }
}

fn set_document_lang(lang: &str) {
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
        let _ = el.set_attribute("lang", if lang == "uk_UA" { "uk" } else { "en" });
    }
}

#[component]
pub fn App() -> impl IntoView {
    if !ipc::is_tauri() {
        crate::mock::install();
    }
    // An app window, not a browser: no browser menu or keys.
    crate::shell::browser::install();
    let modules = StoredValue::new_local(crate::modules::ui_modules());
    provide_context(crate::shell::sidebar::ModulePages(
        modules.with_value(|m| m.iter().flat_map(|m| m.pages()).collect()),
    ));
    provide_context(modules.with_value(|m| crate::modules::ModuleParts::of(m)));
    let i18n = provide_i18n(modules.with_value(|m| build_i18n(&browser_lang(), core_locale, m)));
    Effect::new(move |_| set_document_title(&i18n.t("app_title")));
    let store = provide_store();
    let toasts = provide_toasts();
    provide_context_menu();
    provide_header();
    provide_support();
    let build_actions = use_build_actions();
    provide_launch_flow();
    crate::recent::provide_recent();
    provide_build_dialogs();
    // What modules may ask of the app: a build's content tab, a build started as Play starts it.
    let flow = crate::builds::launch::use_launch_flow();
    ui_kit::module::provide_module_host(ui_kit::module::ModuleHost {
        open_content: Callback::new(move |(key, tab): (String, String)| {
            store.go(&crate::pages::content::content_path(&key, &tab))
        }),
        launch: Callback::new(move |key: String| {
            if let Some(build) = store.builds.with_untracked(|b| b.iter().find(|b| b.key == key).cloned()) {
                flow.start(build);
                return;
            }
            // A build a module has just made may not have reached the list yet.
            leptos::task::spawn_local(async move {
                if let Ok(snapshot) = ipc::call::<BuildsSnapshot>("builds_list").await {
                    let build = snapshot.builds.iter().find(|b| b.key == key).cloned();
                    store.builds.set(snapshot.builds);
                    if let Some(build) = build {
                        flow.start(build);
                    }
                }
            });
        }),
    });

    Effect::new(move |previous: Option<String>| {
        let s = store.settings.get();
        sound::configure(s.click_sound_enabled, s.click_sound);
        if !s.lang.is_empty() && previous.as_deref() != Some(s.lang.as_str()) {
            i18n.set(modules.with_value(|m| build_i18n(&s.lang, core_locale, m)));
            set_document_lang(&s.lang);
        }
        s.lang
    });

    ipc::listen::<OpsSnapshot>(names::OPS, move |s| store.show_ops(s));
    ipc::listen::<SettingsSnapshot>(names::SETTINGS, move |s| store.show_settings(s));
    ipc::listen::<Alert>(names::ALERT, move |a| store.alert.set(Some(a)));
    ipc::listen::<ExternalLaunch>(names::EXTERNAL_LAUNCH, move |ev| store.pending_launch.set(ev.version_id));
    ipc::listen::<Toast>(names::TOAST, move |t| {
        toasts.push(ToastItem {
            id: t.id,
            level: t.level,
            title: i18n.text(&t.title),
            message: t.message.as_ref().map(|m| i18n.text(m)),
            action: t.action.as_ref().map(|a| (i18n.text(&a.label), a.action.clone())),
            duration_ms: duration_for(t.level),
        })
    });
    ipc::listen::<UpdateStatus>(names::UPDATE, move |s| store.update.set(Some(s)));
    ipc::listen::<ProfilesSnapshot>(names::PROFILES, move |s| store.profiles.set(s.profiles));
    ipc::listen::<BuildsSnapshot>(names::BUILDS, move |s| {
        store.builds.set(s.builds);
        store.builds_loaded.set(true);
    });
    // Game events only change `running`; the backend itself alerts about crashes.
    ipc::listen::<GameEvent>(names::GAME, move |_| build_actions.refresh());
    ipc::listen::<AuthState>(names::AUTH, move |s| store.auth.set(s));
    ipc::listen::<ActivityEntry>(names::ACTIVITY, move |entry| {
        store.activity.update(|list| crate::pages::settings::activity::push_activity(list, entry))
    });

    spawn_local(async move {
        match ipc::call::<AppInfo>("app_info").await {
            Ok(info) => {
                modules.with_value(|m| crate::modules::report_mismatch(&info, m));
                store.info.set(Some(info));
            }
            Err(e) => web_sys::console::error_1(&format!("app_info failed: {e}").into()),
        }
        if let Ok(s) = ipc::call::<SettingsSnapshot>("settings_get").await {
            store.show_settings(s);
        }
        if let Ok(setup) = ipc::call::<SetupState>("setup_state").await {
            store.setup.set(Some(setup));
        }
        if let Ok(warnings) = ipc::call::<Vec<Text>>("startup_warnings").await
            && let Some(first) = warnings.into_iter().next()
        {
            store.alert.set(Some(Alert {
                id: 0,
                title: Text::key("warning"),
                message: first,
                allow_report: false,
            }));
        }
        if let Ok(ops) = ipc::call::<OpsSnapshot>("ops_snapshot").await {
            store.show_ops(ops);
        }
        if let Ok(Some(version)) = ipc::call::<Option<String>>("take_pending_launch").await {
            store.pending_launch.set(Some(version));
        }
        #[derive(serde::Serialize)]
        struct LimitArgs {
            limit: usize,
        }
        if let Ok(list) =
            ipc::invoke::<_, Vec<ActivityEntry>>("activity_recent", &LimitArgs { limit: 100 }).await
        {
            store.activity.update(|shown| crate::pages::settings::activity::merge_activity(shown, list));
        }
        if let Ok(snapshot) = ipc::call::<ProfilesSnapshot>("profiles_list").await {
            store.profiles.set(snapshot.profiles);
        }
        if let Ok(snapshot) = ipc::call::<BuildsSnapshot>("builds_list").await {
            store.builds.set(snapshot.builds);
            store.builds_loaded.set(true);
        }
        if let Ok(state) = ipc::call::<AuthState>("auth_state").await {
            store.auth.set(state);
        }
        if let Ok(status) = ipc::call::<UpdateStatus>("update_status").await {
            if status.updated_from.is_some() {
                let version = status.current_version.clone();
                toasts.show(Level::Success, i18n.tp("update_applied_toast", &[("version", version)]), None);
            }
            store.update.set(Some(status));
        }
        store.ready.set(true);
    });

    view! {
        <Router>
            <Shell>
                <Routes fallback=RouteFallback>
                    <Route path=path!("/") view=HomePage />
                    <Route path=path!("/builds") view=BuildsPage />
                    <Route path=path!("/builds/create") view=CreateBuildPage />
                    <Route path=path!("/builds/components") view=ComponentsPage />
                    <Route path=path!("/builds/content") view=ContentRoute />
                    <Route path=path!("/modpacks") view=ModpacksPage />
                    <Route path=path!("/profiles") view=ProfilesPage />
                    <Route path=path!("/screenshots") view=crate::pages::screenshots::ScreenshotsPage />
                    <Route path=path!("/settings") view=SettingsPage />
                    <Route path=path!("/setup") view=SetupPage />
                    <Route path=path!("/dev/kit") view=KitPage />
                </Routes>
            </Shell>
            <Toaster close_label=Signal::derive(move || i18n.t("toast_close")) />
            <ContextMenuHost on_select=Callback::new(|_id: String| {}) />
            <TipLayer />
            <AlertHost />
            <SupportDialog />
            <QuitConfirm />
            <crate::modules::ModuleOverlays />
            <UpdateDialogs />
            <DeviceCodeDialog />
            <BrowserWaitDialog />
            <LaunchDialogs />
            <BuildDialogsHost />
        </Router>
    }
}

/// A module's page by its address — under a header titled with its sidebar label — otherwise Home.
#[component]
fn RouteFallback() -> impl IntoView {
    let store = use_store();
    let location = leptos_router::hooks::use_location();
    let pages = StoredValue::new(use_context::<crate::shell::sidebar::ModulePages>().unwrap_or_default().0);
    move || {
        let backend = store.info.with(|i| crate::shell::sidebar::backend_modules(i.as_ref()));
        let path = location.pathname.get();
        match pages.with_value(|p| crate::shell::sidebar::page_for(p, &backend, &path).copied()) {
            Some(page) => view! { <PageHeader title_key=page.label /> {(page.view)()} }.into_any(),
            None => view! { <HomePage /> }.into_any(),
        }
    }
}
