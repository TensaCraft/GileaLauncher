//! «Основне»: the build's name, its Minecraft version and vanilla or a mod loader (its build under
//! «Додатково»), and the way to the old catalog.

use launcher_shared::LoaderKind;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Checkbox, ChoiceCards, ChoiceOption, Field, Icon, Select, SelectOption, TextInput, Variant,
};

use super::state::loader_available;
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
        mc.set(now_mc);
        loader.set(loader_value(now_loader));
        loader_build.set(now_build);
    });
    let name_error = Signal::derive(move || {
        name_problem.get().map(|key| i18n.tp(key, &[("name", wizard.name.get().trim().to_string())]))
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
        let catalogs = wizard.catalogs.get();
        let mut options = vec![
            ChoiceOption::new(
                loader_value(LoaderKind::Minecraft),
                i18n.t("wizard_vanilla"),
                loader_mark(LoaderKind::Minecraft),
            )
            .with_desc(i18n.t("wizard_vanilla_desc")),
        ];
        for kind in LOADERS {
            let available = loader_available(kind, &version, &catalogs);
            let desc = if available {
                i18n.t("wizard_loader_desc")
            } else {
                i18n.t("wizard_loader_unavailable_hint")
            };
            options.push(
                ChoiceOption::new(loader_value(kind), kind.display_name(), loader_mark(kind))
                    .with_desc(desc)
                    .with_disabled(!available),
            );
        }
        options
    });
    let on_loader = Callback::new(move |picked: String| {
        let version = wizard.state.with_untracked(|s| s.mc.clone());
        wizard.retarget(&version, loader_of(&picked));
    });
    let build_options = Signal::derive(move || {
        let (version, kind) = wizard.state.with(|s| (s.mc.clone(), s.loader));
        let beta = i18n.t("version_create_unstable_loader_badge");
        wizard.catalogs.with(|all| {
            all.get(&kind)
                .and_then(Option::as_ref)
                .map(|catalog| {
                    catalog
                        .clone()
                        .unpack()
                        .into_iter()
                        .find(|o| o.mc == version)
                        .map(|o| o.builds)
                        .unwrap_or_default()
                })
                .unwrap_or_default()
                .into_iter()
                .map(|b| {
                    let label = if b.stable { b.version.clone() } else { format!("{} · {beta}", b.version) };
                    SelectOption::new(b.version, label)
                })
                .collect::<Vec<_>>()
        })
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
