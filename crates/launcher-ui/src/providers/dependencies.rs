//! The dependency dialog (the original's `mods_manager_dependency_dialog`): what
//! installing a project brings along — new files, replacements, what is already there, optional
//! picks — and what keeps it from going ahead.

use std::collections::HashSet;

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, Button, Checkbox, Dialog, DialogFooter, IconAction, Variant};

use launcher_shared::provider::{Action, PlanDto, PlanIssue, PlanItem, ProviderInfo, issue_text, item_text};

/// What the dialog lists to replace: the project itself first when it replaces a file found only
/// by its name, then the dependencies.
pub fn replacements(plan: &PlanDto) -> Vec<PlanItem> {
    plan.main.iter().filter(|m| m.unrecognized).chain(&plan.replace).cloned().collect()
}

#[component]
pub fn DependencyDialog(
    /// Whose plan it is: the dialog names it.
    provider: ProviderInfo,
    open: RwSignal<bool>,
    plan: RwSignal<Option<PlanDto>>,
    /// Installs the plan with the picked optional dependencies.
    on_install: Callback<Vec<PlanItem>>,
    /// Opens a project's page.
    on_open: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let provider_name = StoredValue::new(provider.name);
    let picked = RwSignal::new(HashSet::<String>::new());
    Effect::new(move |_| {
        plan.track();
        picked.set(HashSet::new());
    });
    let can_install = Signal::derive(move || plan.with(|p| p.as_ref().is_some_and(PlanDto::can_install)));
    let item_line = move |item: &PlanItem| {
        let (key, params) = item_text(item);
        i18n.tp(key, &params)
    };
    let issue_line = move |issue: &PlanIssue| match issue_text(issue, &i18n.t("unknown")) {
        (Some(key), params) => i18n.tp(key, &params),
        (None, params) => params.into_iter().next().map(|(_, name)| name).unwrap_or_default(),
    };
    let open_page = move |url: Option<String>| {
        url.map(|url| {
            view! {
                <IconAction
                    icon="open_in_new"
                    tone=ActionTone::Info
                    title=t("modrinth_dependency_open")
                    on_click=Callback::new(move |()| on_open.run(url.clone()))
                />
            }
        })
    };
    let section = move |title: &'static str, tone: &'static str, rows: Vec<AnyView>| {
        (!rows.is_empty()).then(|| {
            view! {
                <section class=format!("provider-deps__section is-{tone}")>
                    <div class="provider-deps__title">{move || i18n.t(title)}</div>
                    {rows}
                </section>
            }
        })
    };
    let items = move |list: &[PlanItem]| -> Vec<AnyView> {
        list.iter()
            .map(|item| {
                let (line, url) = (item_line(item), item.url.clone());
                view! { <div class="provider-deps__row"><span class="provider-deps__text">{line}</span>{open_page(url)}</div> }.into_any()
            })
            .collect()
    };
    let issues = move |list: &[PlanIssue]| -> Vec<AnyView> {
        list.iter()
            .map(|issue| {
                let (line, url, blocking) = (issue_line(issue), issue.url.clone(), issue.blocking);
                view! {
                    <div class="provider-deps__row" class:is-blocking=blocking>
                        <span class="provider-deps__text">{line}</span>
                        {open_page(url)}
                    </div>
                }
                .into_any()
            })
            .collect()
    };
    let optional = move |list: &[PlanItem]| -> Vec<AnyView> {
        list.iter()
            .filter(|i| i.action != Action::Satisfied)
            .map(|item| {
                let (id, line, url) = (item.project_id.clone(), item_line(item), item.url.clone());
                let checked = RwSignal::new(false);
                let toggle = Callback::new(move |on: bool| {
                    picked.update(|p| {
                        if on {
                            p.insert(id.clone());
                        } else {
                            p.remove(&id);
                        }
                    })
                });
                view! {
                    <div class="provider-deps__row">
                        <div class="provider-deps__text"><Checkbox checked=checked label=line on_change=toggle /></div>
                        {open_page(url)}
                    </div>
                }
                .into_any()
            })
            .collect()
    };
    let body = move || {
        plan.get().map(|p| {
            let provider = provider_name.get_value();
            let name = p.main.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| provider.clone());
            let by_hand = (!p.held_files().is_empty()).then(|| {
                let hint = i18n.tp("provider_held_hint", &[("provider", provider.clone())]);
                view! { <p class="provider-deps__lead">{hint}</p> }
            });
            view! {
                <p class="provider-deps__lead">{i18n.tp(lead_key(&p), &[("name", name), ("provider", provider)])}</p>
                {by_hand}
                {section("modrinth_dependencies_to_install", "info", items(&p.install))}
                {section("modrinth_dependencies_to_replace", "primary", items(&replacements(&p)))}
                {section("modrinth_dependencies_satisfied", "info", items(&p.satisfied))}
                {section("modrinth_dependencies_optional", "primary", optional(&p.optional))}
                {section("modrinth_dependencies_optional_unavailable", "muted", issues(&p.optional_issues))}
                {section("modrinth_dependencies_embedded", "muted", issues(&p.embedded))}
                {section("modrinth_dependencies_blocked", "danger", issues(&p.blocking))}
            }
        })
    };
    let confirm = move |_| {
        let chosen: Vec<PlanItem> = plan.with_untracked(|p| {
            p.as_ref()
                .map(|p| {
                    p.optional
                        .iter()
                        .filter(|i| picked.with_untracked(|s| s.contains(&i.project_id)))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        open.set(false);
        on_install.run(chosen);
    };

    view! {
        <Dialog
            open=open
            title=Signal::derive(move || i18n.tp("provider_dependencies_title", &[("provider", provider_name.get_value())]))
            icon="account_tree"
            wide=true
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>
                    {move || if can_install.get() { i18n.t("cancel") } else { i18n.t("close") }}
                </Button>
                <Show when=move || can_install.get()>
                    <Button variant=Variant::Primary icon="download" on_click=confirm>
                        {move || i18n.t("modrinth_dependencies_install")}
                    </Button>
                </Show>
            </DialogFooter>
            {body}
        </Dialog>
    }
}

/// The dialog's first line: what the plan brings, or that it stops with nothing else to show.
fn lead_key(plan: &PlanDto) -> &'static str {
    let brings = !plan.install.is_empty()
        || !replacements(plan).is_empty()
        || !plan.satisfied.is_empty()
        || !plan.optional.is_empty();
    if !brings && !plan.blocking.is_empty() {
        "provider_install_blocked_message"
    } else {
        "provider_dependencies_message"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_lists_an_unrecognized_main_replacement() {
        use launcher_shared::provider::{Action, PlanDto, PlanItem};
        let item = |id: &str, unrecognized: bool| PlanItem {
            project_id: id.into(),
            version_id: "v".into(),
            title: id.into(),
            version_number: "1".into(),
            filename: "f.jar".into(),
            action: Action::Replace,
            current: Some("old.jar".into()),
            url: None,
            unrecognized,
        };
        let plan = PlanDto {
            main: Some(item("main", true)),
            replace: vec![item("dep", false)],
            ..PlanDto::default()
        };
        let ids: Vec<String> = replacements(&plan).into_iter().map(|i| i.project_id).collect();
        assert_eq!(ids, ["main", "dep"]);
        let plain = PlanDto { main: Some(item("main", false)), ..plan };
        assert_eq!(replacements(&plain).len(), 1, "an ordinary update is not listed");
    }

    #[test]
    fn a_plan_that_only_stops_says_so() {
        use launcher_shared::provider::{PlanDto, PlanIssue};
        let stop = PlanIssue {
            code: "file_blocked".into(),
            name: None,
            file_name: None,
            url: None,
            blocking: true,
            held: None,
        };
        let blocked = PlanDto { blocking: vec![stop.clone()], ..PlanDto::default() };
        assert_eq!(lead_key(&blocked), "provider_install_blocked_message");
        let dependency = launcher_shared::provider::PlanItem {
            project_id: "lib".into(),
            version_id: "1".into(),
            title: "Lib".into(),
            version_number: "1".into(),
            filename: "lib.jar".into(),
            action: launcher_shared::provider::Action::Install,
            current: None,
            url: None,
            unrecognized: false,
        };
        let with_dependency =
            PlanDto { install: vec![dependency], blocking: vec![stop], ..PlanDto::default() };
        assert_eq!(lead_key(&with_dependency), "provider_dependencies_message");
        assert_eq!(lead_key(&PlanDto::default()), "provider_dependencies_message");
    }
}
