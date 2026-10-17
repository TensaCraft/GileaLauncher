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

use crate::builds::{check_new_name, default_build_name};
use crate::shell::PageHeader;
use crate::store::{AppStore, use_store};
use state::{Catalogs, WizardState, WizardStep, newest_release};

/// The mod loaders the wizard offers, after vanilla.
pub const LOADERS: [LoaderKind; 4] =
    [LoaderKind::Fabric, LoaderKind::NeoForge, LoaderKind::Forge, LoaderKind::Quilt];

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
    /// The picks were dropped by a change of version or type.
    pub dropped: RwSignal<bool>,
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
            dropped: RwSignal::new(false),
        }
    }

    /// Minecraft `mc` with `loader` (vanilla when it has no build there); a name the player did
    /// not type follows.
    pub fn retarget(&self, mc: &str, loader: LoaderKind) {
        let store = self.store;
        let catalogs = self.catalogs.get_untracked();
        let mut dropped = false;
        self.state.update(|s| dropped = s.set_target(mc, loader, &catalogs));
        if dropped {
            self.dropped.set(true);
        }
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
                Ok(list) => {
                    let (mc, loader) = this.state.with_untracked(|s| (s.mc.clone(), s.loader));
                    if !list.iter().any(|v| v.id == mc)
                        && let Some(newest) = newest_release(&list, snapshots)
                    {
                        this.retarget(&newest, loader);
                    }
                    let _ = this.versions.try_set(Some(Ok(list)));
                }
                Err(e) => {
                    let _ = this.versions.try_set(Some(Err(i18n.error(&e))));
                }
            }
        });
    }

    /// Every loader's catalog; one that cannot be read is unavailable, the others work.
    pub fn load_loaders(&self) {
        for loader in LOADERS {
            let this = *self;
            spawn_local(async move {
                let args = LoaderCatalogArgs { loader, unstable: false };
                let answer: Result<LoaderCatalog, AppError> = ipc::invoke("catalog_loader", &args).await;
                let _ = this.catalogs.try_update(|all| {
                    all.insert(loader, answer.ok());
                });
                // The chosen loader's recommended build, now that it is known.
                let (mc, chosen) = this.state.with_untracked(|s| (s.mc.clone(), s.loader));
                if chosen == loader && !mc.is_empty() {
                    let catalogs = this.catalogs.get_untracked();
                    this.state.update(|s| {
                        s.set_target(&mc, chosen, &catalogs);
                    });
                }
            });
        }
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
    let ready = Memo::new(move |_| {
        let now = step.get();
        let catalogs = wizard.catalogs.get();
        wizard.state.with(|s| s.ready(now, name_problem.get(), &catalogs))
    });
    // Each step starts at its top.
    Effect::new(move |_| {
        step.track();
        if let Ok(Some(page)) = document().query_selector(".content") {
            page.set_scroll_top(0);
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
            if let Err(e) =
                ipc::invoke::<_, BuildDto>("build_create_planned", &json!({ "build": planned })).await
            {
                toasts.show(Level::Error, i18n.error(&e), None);
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
                        disabled=move || reached.get() < i
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
    let desc_key = move || match step.get() {
        WizardStep::Basics => "wizard_desc_basics",
        WizardStep::Content => "wizard_desc_content",
        WizardStep::Settings => "wizard_desc_settings",
        WizardStep::Summary => "wizard_desc_summary",
    };
    let body = move || match step.get() {
        WizardStep::Basics => view! { <basics::Basics wizard=wizard name_problem=name_problem /> }.into_any(),
        WizardStep::Content => view! { <content::Content wizard=wizard /> }.into_any(),
        WizardStep::Settings => view! { <settings::Settings wizard=wizard /> }.into_any(),
        WizardStep::Summary => view! { <summary::Summary wizard=wizard /> }.into_any(),
    };
    // Why «Далі» waits (the name's own problem shows under its field).
    let why = move || match ready.get() {
        Err(reason) if Some(reason) != name_problem.get() => Some(i18n.t(reason)),
        _ => None,
    };

    view! {
        <PageHeader title_key="wizard_title" back="/builds" />
        <div class="build-wizard">
            <section class="wizard__panel">
                <nav class="build-wizard__steps">{steps}</nav>
                <header class="wizard__head">
                    <h2>{move || i18n.t(step.get().title_key())}</h2>
                    <p>{move || i18n.t(desc_key())}</p>
                </header>
                <div class="wizard__body">{body}</div>
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
