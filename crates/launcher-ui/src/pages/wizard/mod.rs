//! The build wizard (`/builds/new`): four steps to a new build with its content and settings —
//! «Основне», «Вміст», «Налаштування», «Підсумок» — and one `build_create_planned` at the end.

mod basics;
mod content;
mod settings;
pub mod state;
mod summary;

use launcher_shared::{AppError, BuildDto, CatalogVersion, Level, LoaderCatalog, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Button, Icon, Variant, ipc, use_toasts};

use crate::builds::description::DESCRIPTION_TOO_LONG;
use crate::builds::{check_new_name, default_build_name};
use crate::shell::PageHeader;
use crate::store::{AppStore, use_store};
use state::{Catalogs, WizardState, WizardStep, newest_release};

pub use crate::builds::loader_pick::LOADERS;

#[derive(serde::Serialize)]
struct CatalogArgs {
    snapshots: bool,
}

#[derive(serde::Serialize)]
struct LoaderCatalogArgs {
    loader: LoaderKind,
    unstable: bool,
}

/// What the steps share.
#[derive(Clone, Copy)]
pub struct Wizard {
    pub state: RwSignal<WizardState>,
    /// The name as typed (kept in `state` too).
    pub name: RwSignal<String>,
    /// The name the wizard made for the target: it follows the target until the player types one.
    pub auto_name: RwSignal<String>,
    pub catalogs: RwSignal<Catalogs>,
    /// Minecraft versions: `None` while loading, an error text when they could not be read.
    pub versions: RwSignal<Option<Result<Vec<CatalogVersion>, String>>>,
    store: AppStore,
    i18n: I18nCtx,
}

impl Wizard {
    fn new() -> Wizard {
        Wizard {
            store: use_store(),
            i18n: use_i18n(),
            state: RwSignal::new(WizardState::default()),
            name: RwSignal::new(String::new()),
            auto_name: RwSignal::new(String::new()),
            catalogs: RwSignal::new(Catalogs::new()),
            versions: RwSignal::new(None),
        }
    }

    /// Minecraft `mc` with `loader` (vanilla when it has no build there); a name the player did
    /// not type follows.
    pub fn retarget(&self, mc: &str, loader: LoaderKind) {
        let store = self.store;
        let catalogs = self.catalogs.get_untracked();
        self.state.update(|s| s.set_target(mc, loader, &catalogs));
        let now = self.state.with_untracked(|s| (s.loader, s.mc.clone()));
        let title = format!("{} {}", now.0.display_name(), now.1);
        let fresh = store.builds.with_untracked(|b| default_build_name(&title, b));
        if self.name.get_untracked() == self.auto_name.get_untracked() {
            self.name.set(fresh.clone());
        }
        self.auto_name.set(fresh);
    }

    /// Minecraft's versions (snapshots too when asked); a version no longer listed falls back to
    /// the newest.
    pub fn load_versions(&self) {
        let this = *self;
        let i18n = this.i18n;
        this.versions.set(None);
        let snapshots = this.state.with_untracked(|s| s.snapshots);
        spawn_local(async move {
            match ipc::invoke::<_, Vec<CatalogVersion>>("catalog_minecraft", &CatalogArgs { snapshots }).await
            {
                Ok(list) => this.take_versions(list, snapshots),
                Err(e) => {
                    let _ = this.versions.try_set(Some(Err(i18n.error(&e))));
                }
            }
        });
    }

    /// The versions that came; a version no longer listed falls back to the newest. A wizard
    /// already closed lets them go.
    fn take_versions(&self, list: Vec<CatalogVersion>, snapshots: bool) {
        let Some((mc, loader)) = self.state.try_with_untracked(|s| (s.mc.clone(), s.loader)) else {
            return;
        };
        if !list.iter().any(|v| v.id == mc)
            && let Some(newest) = newest_release(&list, snapshots)
        {
            self.retarget(&newest, loader);
        }
        let _ = self.versions.try_set(Some(Ok(list)));
    }

    /// Every loader's catalog; one that cannot be read is unavailable, the others work.
    pub fn load_loaders(&self) {
        for loader in LOADERS {
            self.load_loader(loader);
        }
    }

    /// `loader`'s catalog (asked again after a failure: its card shows it on its way meanwhile).
    pub fn load_loader(&self, loader: LoaderKind) {
        self.forget_catalog(loader);
        let this = *self;
        spawn_local(async move {
            let args = LoaderCatalogArgs { loader, unstable: false };
            let answer: Result<LoaderCatalog, AppError> = ipc::invoke("catalog_loader", &args).await;
            this.take_catalog(loader, answer.map_err(|e| this.i18n.error(&e)));
        });
    }

    fn forget_catalog(&self, loader: LoaderKind) {
        self.catalogs.update(|all| {
            all.remove(&loader);
        });
    }

    /// `loader`'s catalog that came, or why it could not be read. A wizard already closed lets it
    /// go.
    fn take_catalog(&self, loader: LoaderKind, catalog: Result<LoaderCatalog, String>) {
        let Some(catalogs) = self.catalogs.try_update(|all| {
            all.insert(loader, catalog);
            all.clone()
        }) else {
            return;
        };
        // The chosen loader's recommended build, now that it is known.
        let _ = self.state.try_update(|s| {
            if s.loader == loader && !s.mc.is_empty() {
                let mc = s.mc.clone();
                s.set_target(&mc, loader, &catalogs);
            }
        });
    }
}

/// Whether the step's main button waits: the step's own reason, or another operation running,
/// which the creation would be refused for (and the plan lost with the page already left).
fn waits_for(ready: Result<(), &'static str>, step: WizardStep, busy: bool) -> Result<(), &'static str> {
    ready?;
    if step == WizardStep::Summary && busy {
        return Err("wizard_busy");
    }
    Ok(())
}

/// What the main button says of why it waits: nothing of a reason its field on the step shows.
fn button_reason(
    ready: Result<(), &'static str>,
    name_problem: Option<&'static str>,
    step: WizardStep,
) -> Option<&'static str> {
    let reason = ready.err()?;
    let under_its_field =
        Some(reason) == name_problem || (step == WizardStep::Settings && reason == DESCRIPTION_TOO_LONG);
    (!under_its_field).then_some(reason)
}

/// The note that a change of version or type dropped the picks (a translation key), until the
/// player changes the plan.
fn dropped_note(state: &WizardState) -> Option<&'static str> {
    state.picks_dropped.then_some("wizard_picks_dropped")
}

/// The dropped picks, told on «Вміст» and on «Підсумок» (a player may jump straight there).
#[component]
fn PicksDropped(wizard: Wizard) -> impl IntoView {
    let i18n = use_i18n();
    move || {
        wizard.state.with(dropped_note).map(|key| {
            view! {
                <div class="wizard__status is-info build-wizard__dropped">
                    <Icon name="info" />
                    <span>{move || i18n.t(key)}</span>
                </div>
            }
        })
    }
}

/// The line under the step's title (a plain game's content step names resource packs only).
fn step_desc(step: WizardStep, loader: LoaderKind) -> &'static str {
    match step {
        WizardStep::Basics => "wizard_desc_basics",
        WizardStep::Content if loader == LoaderKind::Minecraft => "wizard_desc_content_vanilla",
        WizardStep::Content => "wizard_desc_content",
        WizardStep::Settings => "wizard_desc_settings",
        WizardStep::Summary => "wizard_desc_summary",
    }
}

#[component]
pub fn BuildWizardPage() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let wizard = Wizard::new();
    let step = RwSignal::new(WizardStep::Basics);
    let reached = RwSignal::new(0usize);
    wizard.load_versions();
    wizard.load_loaders();
    // The typed name goes into the state.
    Effect::new(move |_| {
        let name = wizard.name.get();
        wizard.state.update(|s| s.name = name);
    });
    let name_problem = Memo::new(move |_| {
        let name = wizard.name.get();
        store.builds.with(|b| check_new_name(&name, b).err().map(|p| p.key()))
    });
    let busy = Memo::new(move |_| store.ops.with(|o| o.busy));
    let ready = Memo::new(move |_| {
        let now = step.get();
        let own = wizard
            .catalogs
            .with(|catalogs| wizard.state.with(|s| s.ready(now, name_problem.get(), catalogs)));
        waits_for(own, now, busy.get())
    });
    // Each step starts at its top.
    let body_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        step.track();
        if let Some(body) = body_ref.get_untracked() {
            body.set_scroll_top(0);
        }
    });
    let go = move |to: WizardStep| {
        reached.update(|r| *r = (*r).max(to.index()));
        step.set(to);
    };
    let create = move || {
        let planned = wizard.state.get_untracked().planned();
        store.go("/builds");
        spawn_local(async move {
            match ipc::invoke::<_, BuildDto>("build_create_planned", &json!({ "build": planned })).await {
                // The hints point at the new build.
                Ok(made) => {
                    store.fresh_build.try_set(Some(made.key));
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    };
    // The steps in a row across the top, a line from each to the next (lit once it is passed).
    let steps = move || {
        let now = step.get();
        WizardStep::ALL
            .iter()
            .map(|&each| {
                let i = each.index();
                let done = i < now.index();
                view! {
                    {(i > 0).then(|| view! { <span class="build-wizard__line" class:is-done=i <= now.index()></span> })}
                    <button
                        type="button"
                        class="build-wizard__step"
                        class:is-on=each == now
                        class:is-done=done
                        aria-current=(each == now).then_some("step")
                        disabled=move || {
                            reached.get() < i
                                || !wizard.catalogs.with(|catalogs| {
                                    wizard.state.with(|s| s.can_open(each, name_problem.get(), catalogs))
                                })
                        }
                        on:click=move |_| step.set(each)
                    >
                        <span class="build-wizard__mark">
                            {if done {
                                view! { <Icon name="check" /> }.into_any()
                            } else {
                                view! { <span>{i + 1}</span> }.into_any()
                            }}
                        </span>
                        <span>{i18n.t(each.title_key())}</span>
                    </button>
                }
            })
            .collect_view()
    };
    let desc_key = move || step_desc(step.get(), wizard.state.with(|s| s.loader));
    let body = move || match step.get() {
        WizardStep::Basics => view! { <basics::Basics wizard=wizard name_problem=name_problem /> }.into_any(),
        WizardStep::Content => view! { <content::Content wizard=wizard /> }.into_any(),
        WizardStep::Settings => view! { <settings::Settings wizard=wizard /> }.into_any(),
        WizardStep::Summary => view! { <summary::Summary wizard=wizard /> }.into_any(),
    };
    // Why «Далі» waits.
    let why = move || button_reason(ready.get(), name_problem.get(), step.get()).map(|reason| i18n.t(reason));

    view! {
        <PageHeader title_key="wizard_title" back="/builds" />
        <div class="build-wizard">
            <section class="wizard__panel">
                <nav class="build-wizard__steps">{steps}</nav>
                <header class="wizard__head">
                    <h2>{move || i18n.t(step.get().title_key())}</h2>
                    <p>{move || i18n.t(desc_key())}</p>
                </header>
                <div node_ref=body_ref class="wizard__body">{body}</div>
                <footer class="wizard__foot">
                    {move || match step.get().prev() {
                        Some(prev) => view! {
                            <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| step.set(prev)>
                                {move || i18n.t("wizard_back")}
                            </Button>
                        }.into_any(),
                        None => view! {
                            <Button variant=Variant::Ghost icon="close" on_click=move |_| store.go("/builds")>
                                {move || i18n.t("cancel")}
                            </Button>
                        }.into_any(),
                    }}
                    <span class="wizard__gap"></span>
                    {move || why().map(|text| view! { <span class="build-wizard__why">{text}</span> })}
                    {move || match step.get().next() {
                        Some(next) => view! {
                            <Button
                                variant=Variant::Primary
                                icon="arrow_forward"
                                disabled=Signal::derive(move || ready.get().is_err())
                                on_click=move |_| go(next)
                            >
                                {move || i18n.t("wizard_next")}
                            </Button>
                        }.into_any(),
                        None => view! {
                            <Button
                                variant=Variant::Primary
                                icon="download"
                                disabled=Signal::derive(move || ready.get().is_err())
                                on_click=move |_| create()
                            >
                                {move || i18n.t("wizard_create")}
                            </Button>
                        }.into_any(),
                    }}
                </footer>
            </section>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{LoaderBuild, LoaderOption};
    use ui_kit::i18n::{Dictionary, I18n, provide_i18n};
    use ui_kit::provide_toasts;

    use super::state::{LoaderState, loader_state};
    use super::*;
    use crate::store::provide_store;

    fn app() -> Owner {
        let app = Owner::new();
        app.with(|| {
            provide_i18n(I18n::new("uk_UA", Dictionary::default(), Dictionary::default()));
            provide_toasts();
            provide_store();
        });
        app
    }

    #[test]
    fn answers_that_come_after_the_wizard_closed_are_let_go() {
        let app = app();
        let page = app.with(Owner::new);
        let wizard = page.with(Wizard::new);
        // The player left for another page while the catalogs were on their way.
        page.cleanup();
        let version = CatalogVersion { id: "1.21.1".into(), kind: "release".into(), release_time: None };
        wizard.take_versions(vec![version], false);
        wizard.take_catalog(LoaderKind::Fabric, Err("offline".into()));
        app.cleanup();
    }

    #[test]
    fn a_loader_asked_again_is_on_its_way_until_its_answer_comes() {
        let app = app();
        let wizard = app.with(Wizard::new);
        let forge =
            move || wizard.catalogs.with_untracked(|all| loader_state(LoaderKind::Forge, "1.21.1", all));
        wizard.take_catalog(LoaderKind::Forge, Err("offline".into()));
        assert_eq!(forge(), LoaderState::Failed("offline".into()));
        wizard.forget_catalog(LoaderKind::Forge);
        assert_eq!(forge(), LoaderState::Loading, "«Спробувати ще» shows it on its way");
        let option = LoaderOption {
            mc: "1.21.1".into(),
            snapshot: false,
            builds: vec![LoaderBuild { version: "52.1.0".into(), stable: true }],
            default_version: "52.1.0".into(),
        };
        wizard.take_catalog(LoaderKind::Forge, Ok(LoaderCatalog::pack(vec![option])));
        assert_eq!(forge(), LoaderState::Available);
        app.cleanup();
    }

    #[test]
    fn creating_waits_while_another_operation_runs() {
        assert_eq!(waits_for(Ok(()), WizardStep::Summary, true), Err("wizard_busy"));
        assert_eq!(waits_for(Ok(()), WizardStep::Summary, false), Ok(()));
        assert_eq!(waits_for(Ok(()), WizardStep::Content, true), Ok(()), "only the creation collides");
        assert_eq!(waits_for(Err("wizard_no_version"), WizardStep::Summary, true), Err("wizard_no_version"));
    }

    #[test]
    fn a_reason_its_field_shows_is_not_said_again_by_the_button() {
        let name = Some("version_name_empty");
        assert_eq!(button_reason(Err("version_name_empty"), name, WizardStep::Basics), None);
        assert_eq!(
            button_reason(Err("wizard_no_version"), name, WizardStep::Basics),
            Some("wizard_no_version")
        );
        let long = Err("build_description_too_long");
        assert_eq!(button_reason(long, None, WizardStep::Settings), None, "under the description");
        assert_eq!(
            button_reason(long, None, WizardStep::Summary),
            Some("build_description_too_long"),
            "the field is a step back"
        );
        assert_eq!(button_reason(Ok(()), None, WizardStep::Summary), None);
    }

    #[test]
    fn vanilla_s_content_step_says_nothing_of_mods() {
        assert_eq!(step_desc(WizardStep::Content, LoaderKind::Minecraft), "wizard_desc_content_vanilla");
        assert_eq!(step_desc(WizardStep::Content, LoaderKind::Fabric), "wizard_desc_content");
        assert_eq!(step_desc(WizardStep::Summary, LoaderKind::Minecraft), "wizard_desc_summary");
    }

    #[test]
    fn the_dropped_picks_are_noted_until_the_player_changes_the_plan() {
        let sodium = launcher_shared::PlannedPick {
            provider: "modrinth".into(),
            kind: launcher_shared::ContentKind::Mods,
            project_id: "sodium".into(),
            slug: "sodium".into(),
            title: "Sodium".into(),
            icon_url: None,
        };
        let mut state = WizardState::default();
        state.set_target("1.21.1", LoaderKind::Minecraft, &state::Catalogs::new());
        assert_eq!(dropped_note(&state), None, "nothing picked, nothing dropped");
        state.add_pick(sodium.clone());
        state.set_target("1.20.1", LoaderKind::Minecraft, &state::Catalogs::new());
        assert_eq!(dropped_note(&state), Some("wizard_picks_dropped"), "seen on «Вміст» or «Підсумок»");
        state.add_pick(sodium);
        assert_eq!(dropped_note(&state), None);
    }
}
