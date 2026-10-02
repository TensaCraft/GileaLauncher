//! One install flow for every place that installs from a provider — its tab and the Installed
//! list's "Update": plan → the dependency dialog when the plan asks for it → install; a plan that
//! changed meanwhile goes back to the dialog. One install at a time in the session, whatever the
//! provider.

use std::collections::HashSet;

use launcher_shared::provider::{InstallAnswer, InstallArgs, PlanDto, PlanItem, ProviderInfo};
use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Toasts, use_toasts};

use super::dependencies::DependencyDialog;
use super::held::watch_downloads;
use super::{api, describe, open_page};

/// Installs in flight in this session, shared by every provider panel: a panel opened again
/// mid-install (another tab and back) still shows it, and every open panel reloads what is
/// installed when one ends. Arc signals, so no panel's disposal takes them along.
#[derive(Clone)]
pub(crate) struct Installs {
    /// `(provider, build key, project id)`.
    pending: ArcRwSignal<HashSet<(String, String, String)>>,
    /// Counts the installs that ended, however they ended.
    pub ended: ArcRwSignal<u64>,
}

fn flight(provider: &str, key: &str, project: &str) -> (String, String, String) {
    (provider.to_string(), key.to_string(), project.to_string())
}

impl Installs {
    /// Marks an install as started; `false` while another runs (one at a time, as the original).
    pub fn start(&self, provider: &str, key: &str, project: &str) -> bool {
        if self.pending.with_untracked(|p| !p.is_empty()) {
            return false;
        }
        self.pending.update(|p| {
            p.insert(flight(provider, key, project));
        });
        true
    }

    pub fn end(&self, provider: &str, key: &str, project: &str) {
        self.pending.update(|p| {
            p.remove(&flight(provider, key, project));
        });
        self.ended.update(|n| *n += 1);
    }

    pub fn running(&self, provider: &str, key: &str, project: &str) -> bool {
        self.pending.with(|p| p.contains(&flight(provider, key, project)))
    }
}

pub(crate) fn installs() -> Installs {
    thread_local! {
        static INSTALLS: Installs =
            Installs { pending: ArcRwSignal::new(HashSet::new()), ended: ArcRwSignal::new(0) };
    }
    INSTALLS.with(Installs::clone)
}

/// Tells `done` a project was installed — unless the component that asked is gone by now (the
/// user left the tab mid-install): there is nothing left to refresh then.
pub(crate) fn report_done(done: Callback<String>, project: String) {
    done.try_run(project);
}

/// One build tab's install flow with one provider. `done` hears each installed project's id. Made
/// during a component's setup: it keeps the translations and toasts, since event handlers and
/// spawned tasks have no context to look them up in.
#[derive(Clone, Copy)]
pub(crate) struct InstallFlow {
    provider: StoredValue<ProviderInfo>,
    open: RwSignal<bool>,
    plan: RwSignal<Option<PlanDto>>,
    args: StoredValue<Option<InstallArgs>>,
    done: Callback<String>,
    i18n: I18nCtx,
    toasts: Toasts,
}

impl InstallFlow {
    pub fn new(provider: ProviderInfo, done: Callback<String>) -> InstallFlow {
        InstallFlow {
            provider: StoredValue::new(provider),
            open: RwSignal::new(false),
            plan: RwSignal::new(None),
            args: StoredValue::new(None),
            done,
            i18n: use_i18n(),
            toasts: use_toasts(),
        }
    }

    fn show(self, args: InstallArgs, plan: PlanDto) {
        self.args.try_set_value(Some(args));
        self.plan.try_set(Some(plan));
        self.open.try_set(true);
    }

    /// What an install's answer does: the flight ends, then the project is reported, the new plan
    /// shown or the error told. `provider` was taken before the wait: the tab — and what it stored —
    /// may be gone by now (the user switched tabs mid-install).
    pub(crate) fn settle(
        self,
        provider: &ProviderInfo,
        args: InstallArgs,
        answer: Result<InstallAnswer, AppError>,
    ) {
        installs().end(&provider.id, &args.key, &args.project_id);
        match answer {
            Ok(InstallAnswer::Installed(done)) => report_done(self.done, done.project_id),
            Ok(InstallAnswer::Replanned(plan)) => self.show(args, *plan),
            Err(e) => self.toasts.show(Level::Error, describe(self.i18n, provider, &e), None),
        }
    }

    async fn finish(self, provider: ProviderInfo, args: InstallArgs) {
        let answer = api::install(&provider.id, &args).await;
        self.settle(&provider, args, answer);
    }

    /// Plans `args`; installs straight away when the plan needs no word from the user.
    pub fn start(self, args: InstallArgs) {
        let Some(provider) = self.provider.try_get_value() else { return };
        if !installs().start(&provider.id, &args.key, &args.project_id) {
            self.toasts.show(Level::Info, self.i18n.t("installation_already_running"), None);
            return;
        }
        spawn_local(async move {
            match api::plan(&provider.id, &args).await {
                Ok(plan) if plan.requires_confirmation() => {
                    installs().end(&provider.id, &args.key, &args.project_id);
                    self.show(args, plan);
                }
                Ok(plan) => self.finish(provider, args.approve(&plan, &[])).await,
                Err(e) => self.settle(&provider, args, Err(e)),
            }
        });
    }

    /// The dependency dialog of this flow.
    pub fn dialog(self) -> impl IntoView {
        let (i18n, toasts) = (self.i18n, self.toasts);
        // The user approved the plan in the dialog, with these optional picks.
        let approve = Callback::new(move |picks: Vec<PlanItem>| {
            let (Some(base), Some(plan), Some(provider)) =
                (self.args.get_value(), self.plan.get_untracked(), self.provider.try_get_value())
            else {
                return;
            };
            if !installs().start(&provider.id, &base.key, &base.project_id) {
                toasts.show(Level::Info, i18n.t("installation_already_running"), None);
                return;
            }
            spawn_local(self.finish(provider, base.approve(&plan, &picks)));
        });
        let provider = self.provider.get_value();
        let open_page = {
            let provider = provider.clone();
            Callback::new(move |url: String| open_page(i18n, toasts, &provider, url))
        };
        // While the dialog names files to download by hand, the Downloads folder is watched; one
        // that turns up there plans the install again, and a plan that needs no word from the user
        // then installs at once.
        let held = Memo::new(move |_| {
            if !self.open.get() {
                return Vec::new();
            }
            self.plan.with(|p| p.as_ref().map(PlanDto::held_files).unwrap_or_default())
        });
        let found = RwSignal::new(Vec::<bool>::new());
        watch_downloads(held, found);
        Effect::new(move |_| {
            if !found.with(|f| f.iter().any(|there| *there)) {
                return;
            }
            let (Some(args), Some(provider)) = (self.args.get_value(), self.provider.try_get_value()) else {
                return;
            };
            spawn_local(async move {
                let Ok(plan) = api::plan(&provider.id, &args).await else {
                    // Asked again at the next look, however that one failed.
                    let _ = found.try_update(|f| f.iter_mut().for_each(|there| *there = false));
                    return;
                };
                let open = self.open.try_get_untracked().unwrap_or(false);
                if open
                    && !plan.requires_confirmation()
                    && installs().start(&provider.id, &args.key, &args.project_id)
                {
                    let _ = self.open.try_set(false);
                    self.finish(provider, args.approve(&plan, &[])).await;
                } else {
                    let _ = self.plan.try_set(Some(plan));
                }
            });
        });
        view! {
            <DependencyDialog provider=provider open=self.open plan=self.plan on_install=approve on_open=open_page />
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_that_ends_after_its_tab_closed_reports_to_no_one() {
        let panel = Owner::new();
        let done = panel.with(|| Callback::new(|_: String| {}));
        // The user switched tabs mid-install: the panel and its callback are gone.
        panel.cleanup();
        report_done(done, "sodium".into());
    }

    #[test]
    fn installs_in_flight_outlive_the_panel_that_started_them() {
        let panel = Owner::new();
        panel.with(|| assert!(installs().start("modrinth", "aero", "sodium")));
        // The user opens another tab: the panel is gone, its install is not.
        panel.cleanup();
        let flight = installs();
        assert!(flight.running("modrinth", "aero", "sodium"));
        assert!(!flight.start("modrinth", "aero", "lithium"), "one install at a time");
        let ended = flight.ended.get_untracked();
        flight.end("modrinth", "aero", "sodium");
        assert!(!flight.running("modrinth", "aero", "sodium"));
        assert_eq!(flight.ended.get_untracked(), ended + 1, "open panels reload what is installed");
        assert!(flight.start("modrinth", "aero", "lithium"));
        flight.end("modrinth", "aero", "lithium");
    }

    #[test]
    fn one_install_at_a_time_across_providers() {
        let flight = installs();
        assert!(flight.start("modrinth", "aero", "sodium"));
        assert!(!flight.start("curseforge", "zeta", "jei"), "another provider waits too");
        assert!(flight.running("modrinth", "aero", "sodium"));
        assert!(!flight.running("curseforge", "aero", "sodium"), "a project is its provider's");
        flight.end("modrinth", "aero", "sodium");
        assert!(flight.start("curseforge", "zeta", "jei"));
        flight.end("curseforge", "zeta", "jei");
    }

    fn modrinth() -> ProviderInfo {
        ProviderInfo {
            id: "modrinth".into(),
            name: "Modrinth".into(),
            icon: "search".into(),
            content: Vec::new(),
            updates: Vec::new(),
            modpacks: false,
            modpack_updates: false,
        }
    }

    #[test]
    fn a_flow_s_answer_after_its_tab_closed_still_ends_the_install() {
        use launcher_shared::ContentKind;
        use launcher_shared::ErrorCode;
        use ui_kit::i18n::{Dictionary, I18n, provide_i18n};
        use ui_kit::provide_toasts;
        let app = Owner::new();
        app.with(|| {
            provide_i18n(I18n::new("uk_UA", Dictionary::default(), Dictionary::default()));
            provide_toasts();
        });
        let tab = app.with(Owner::new);
        let flow = tab.with(|| InstallFlow::new(modrinth(), Callback::new(|_: String| {})));
        assert!(installs().start("modrinth", "aero", "sodium"));
        // The user leaves the tab while the provider answers.
        tab.cleanup();
        let args = InstallArgs::new("aero", ContentKind::Mods, "sodium", "sodium", "Sodium");
        flow.settle(&modrinth(), args, Err(AppError::new(ErrorCode::Network, "offline")));
        assert!(!installs().running("modrinth", "aero", "sodium"), "the flight ended");
        assert!(installs().start("modrinth", "aero", "lithium"), "the next install may go");
        installs().end("modrinth", "aero", "lithium");
        app.cleanup();
    }
}
