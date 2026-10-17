//! «Основне»: the build's name, its Minecraft version and vanilla or a mod loader (its build under
//! «Додатково»), and the way to the old catalog.

use launcher_shared::{CatalogVersion, LoaderKind};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Checkbox, ChoiceCards, ChoiceOption, Field, Icon, Select, SelectOption, TextInput, Variant,
};

use super::state::{Catalogs, LoaderState, loader_state};
use super::{LOADERS, Wizard};
use crate::builds::loader_icon;
use crate::store::use_store;

/// A loader as the cards and the state name it.
fn loader_value(kind: LoaderKind) -> String {
    kind.display_name().to_lowercase()
}

fn loader_of(value: &str) -> LoaderKind {
    LOADERS.into_iter().find(|k| loader_value(*k) == value).unwrap_or(LoaderKind::Minecraft)
}

fn loader_mark(kind: LoaderKind) -> &'static str {
    loader_icon(Some(kind.display_name()))
}

/// The line under a loader's card.
fn loader_hint(state: &LoaderState) -> &'static str {
    match state {
        LoaderState::Available => "wizard_loader_desc",
        LoaderState::Loading => "wizard_loading",
        LoaderState::Failed(_) => "wizard_loader_failed_hint",
        LoaderState::Missing => "wizard_loader_unavailable_hint",
    }
}

/// The loaders whose catalog could not be read, in the cards' order, with the first one's reason.
fn unread_loaders(catalogs: &Catalogs) -> Option<(Vec<LoaderKind>, String)> {
    let unread: Vec<(LoaderKind, &String)> =
        LOADERS.into_iter().filter_map(|k| catalogs.get(&k)?.as_ref().err().map(|e| (k, e))).collect();
    let reason = unread.first()?.1.clone();
    Some((unread.into_iter().map(|(k, _)| k).collect(), reason))
}

/// The name's problem, said once the versions came: until then the wizard has given no name.
fn name_problem_shown(
    problem: Option<&'static str>,
    versions: &Option<Result<Vec<CatalogVersion>, String>>,
) -> Option<&'static str> {
    problem.filter(|_| versions.is_some())
}

/// `value` into `field`, waking those who read it only when it is another.
fn follow<T: PartialEq + Send + Sync + 'static>(field: RwSignal<T>, value: T) {
    field.maybe_update(|shown| {
        let changed = *shown != value;
        if changed {
            *shown = value;
        }
        changed
    });
}

#[component]
pub fn Basics(wizard: Wizard, name_problem: Memo<Option<&'static str>>) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let mc = RwSignal::new(wizard.state.with_untracked(|s| s.mc.clone()));
    let snapshots = RwSignal::new(wizard.state.with_untracked(|s| s.snapshots));
    let loader = RwSignal::new(wizard.state.with_untracked(|s| loader_value(s.loader)));
    let loader_build = RwSignal::new(wizard.state.with_untracked(|s| s.loader_version.clone()));
    let advanced = RwSignal::new(false);
    // The fields follow the state (a version list that came later, a loader that fell back).
    Effect::new(move |_| {
        let (now_mc, now_loader, now_build) =
            wizard.state.with(|s| (s.mc.clone(), s.loader, s.loader_version.clone()));
        follow(mc, now_mc);
        follow(loader, loader_value(now_loader));
        follow(loader_build, now_build);
    });
    let name_error = Signal::derive(move || {
        let problem = wizard.versions.with(|v| name_problem_shown(name_problem.get(), v));
        problem.map(|key| i18n.tp(key, &[("name", wizard.name.get().trim().to_string())]))
    });
    let version_options = Signal::derive(move || {
        wizard.versions.with(|v| match v {
            Some(Ok(list)) => list.iter().map(|v| SelectOption::new(v.id.clone(), v.id.clone())).collect(),
            _ => Vec::new(),
        })
    });
    let on_version = Callback::new(move |picked: String| {
        let kind = wizard.state.with_untracked(|s| s.loader);
        wizard.retarget(&picked, kind);
    });
    let on_snapshots = Callback::new(move |on: bool| {
        wizard.state.update(|s| s.snapshots = on);
        wizard.load_versions();
    });
    let loader_options = Signal::derive(move || {
        let version = mc.get();
        let mut options = vec![
            ChoiceOption::new(
                loader_value(LoaderKind::Minecraft),
                i18n.t("wizard_vanilla"),
                loader_mark(LoaderKind::Minecraft),
            )
            .with_desc(i18n.t("wizard_vanilla_desc")),
        ];
        wizard.catalogs.with(|catalogs| {
            for kind in LOADERS {
                let state = loader_state(kind, &version, catalogs);
                options.push(
                    ChoiceOption::new(loader_value(kind), kind.display_name(), loader_mark(kind))
                        .with_desc(i18n.t(loader_hint(&state)))
                        .with_disabled(state != LoaderState::Available),
                );
            }
        });
        options
    });
    let unread = Memo::new(move |_| wizard.catalogs.with(unread_loaders));
    let retry_loaders = move || {
        for kind in unread.get_untracked().map(|(kinds, _)| kinds).unwrap_or_default() {
            wizard.load_loader(kind);
        }
    };
    let on_loader = Callback::new(move |picked: String| {
        let version = wizard.state.with_untracked(|s| s.mc.clone());
        wizard.retarget(&version, loader_of(&picked));
    });
    let build_options = Signal::derive(move || {
        let (version, kind) = wizard.state.with(|s| (s.mc.clone(), s.loader));
        let beta = i18n.t("version_create_unstable_loader_badge");
        let builds = wizard.catalogs.with(|all| {
            all.get(&kind).and_then(|c| c.as_ref().ok()).map(|c| c.builds_for(&version)).unwrap_or_default()
        });
        builds
            .into_iter()
            .map(|b| {
                let label = if b.stable { b.version.clone() } else { format!("{} · {beta}", b.version) };
                SelectOption::new(b.version, label)
            })
            .collect::<Vec<_>>()
    });
    let on_build = Callback::new(move |picked: String| wizard.state.update(|s| s.loader_version = picked));
    let is_loader = move || wizard.state.with(|s| s.loader != LoaderKind::Minecraft);

    view! {
        <div class="build-wizard__basics">
            <Field label=t("version_name_label") error=name_error>
                <TextInput value=wizard.name autofocus=true invalid=Signal::derive(move || name_error.get().is_some()) />
            </Field>
            <Field label=t("wizard_version")>
                {move || match wizard.versions.get() {
                    None => view! { <div class="hint">{move || i18n.t("wizard_loading")}</div> }.into_any(),
                    Some(Err(e)) => view! {
                        <div class="wizard__status is-error">
                            <Icon name="error_outline" />
                            <span class="grow">{e}</span>
                            <Button variant=Variant::Ghost icon="refresh" on_click=move |_| wizard.load_versions()>
                                {move || i18n.t("wizard_retry")}
                            </Button>
                        </div>
                    }.into_any(),
                    Some(Ok(_)) => view! {
                        <div class="wrap">
                            <div class="grow"><Select options=version_options value=mc on_change=on_version /></div>
                            <Checkbox checked=snapshots label=t("wizard_snapshots") on_change=on_snapshots />
                        </div>
                    }.into_any(),
                }}
            </Field>
            <div class="wizard__ask">{move || i18n.t("wizard_type")}</div>
            <ChoiceCards options=loader_options value=loader on_change=on_loader columns=5 label=t("wizard_type") />
            {move || unread.get().map(|(kinds, reason)| {
                let names = kinds.iter().map(|k| k.display_name()).collect::<Vec<_>>().join(", ");
                view! {
                    <div class="wizard__status is-error build-wizard__unread">
                        <Icon name="error_outline" />
                        <span class="grow">{i18n.tp("wizard_loaders_failed", &[("loaders", names), ("error", reason)])}</span>
                        <Button variant=Variant::Ghost icon="refresh" on_click=move |_| retry_loaders()>
                            {move || i18n.t("wizard_retry")}
                        </Button>
                    </div>
                }
            })}
            <Show when=is_loader>
                <button type="button" class="build-wizard__more" on:click=move |_| advanced.update(|a| *a = !*a)>
                    {move || view! { <Icon name=if advanced.get() { "expand_less" } else { "expand_more" } /> }}
                    {move || i18n.t("wizard_advanced")}
                </button>
                <Show when=move || advanced.get()>
                    <Field label=t("minecraft_components_loader_build_label")>
                        <Select options=build_options value=loader_build on_change=on_build />
                    </Field>
                </Show>
            </Show>
            <div class="wizard__later">
                <Button variant=Variant::Ghost icon="list" on_click=move |_| store.go("/builds/create")>
                    {move || i18n.t("wizard_classic")}
                </Button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use launcher_shared::{CatalogVersion, LoaderCatalog};

    use super::*;
    use crate::pages::wizard::state::Catalogs;

    #[test]
    fn a_card_tells_a_catalog_on_its_way_or_unread_apart_from_one_without_the_version() {
        assert_eq!(loader_hint(&LoaderState::Available), "wizard_loader_desc");
        assert_eq!(loader_hint(&LoaderState::Loading), "wizard_loading");
        assert_eq!(loader_hint(&LoaderState::Failed("offline".into())), "wizard_loader_failed_hint");
        assert_eq!(loader_hint(&LoaderState::Missing), "wizard_loader_unavailable_hint");
    }

    #[test]
    fn the_unread_catalogs_are_named_in_the_cards_order_with_the_first_reason() {
        let mut all = Catalogs::new();
        all.insert(LoaderKind::Quilt, Err("timed out".into()));
        all.insert(LoaderKind::Fabric, Ok(LoaderCatalog::default()));
        all.insert(LoaderKind::Forge, Err("offline".into()));
        assert_eq!(
            unread_loaders(&all),
            Some((vec![LoaderKind::Forge, LoaderKind::Quilt], "offline".to_string()))
        );
        all.remove(&LoaderKind::Forge);
        all.remove(&LoaderKind::Quilt);
        assert_eq!(unread_loaders(&all), None, "on its way or read");
    }

    #[test]
    fn the_name_says_nothing_while_the_versions_are_on_their_way() {
        let came = Some(Ok(vec![CatalogVersion {
            id: "1.21.1".into(),
            kind: "release".into(),
            release_time: None,
        }]));
        assert_eq!(name_problem_shown(Some("version_name_empty"), &None), None);
        assert_eq!(name_problem_shown(Some("version_name_empty"), &came), Some("version_name_empty"));
        assert_eq!(
            name_problem_shown(Some("version_name_empty"), &Some(Err("offline".into()))),
            Some("version_name_empty"),
            "they will not come by themselves"
        );
        assert_eq!(name_problem_shown(None, &came), None);
    }

    #[test]
    fn a_field_that_already_shows_the_value_wakes_nobody() {
        let owner = Owner::new();
        owner.with(|| {
            let field = RwSignal::new("1.21.1".to_string());
            let reads = Arc::new(AtomicUsize::new(0));
            let watcher = {
                let reads = reads.clone();
                Memo::new_with_compare(
                    move |_| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        field.get()
                    },
                    |_, _| true,
                )
            };
            let _ = watcher.get();
            let heard = reads.load(Ordering::SeqCst);
            follow(field, "1.21.1".to_string());
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), heard, "the same value");
            follow(field, "1.20.1".to_string());
            let _ = watcher.get();
            assert_eq!(reads.load(Ordering::SeqCst), heard + 1, "another one");
            assert_eq!(field.get_untracked(), "1.20.1");
        });
        owner.cleanup();
    }
}
