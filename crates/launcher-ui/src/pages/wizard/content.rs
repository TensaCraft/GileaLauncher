//! «Вміст»: the providers' mods, resource packs and shader packs for the build to be, each one
//! put into the plan (or taken out) instead of installed; the plan itself above the list.

use launcher_shared::provider::{ProjectHit, SearchArgs, SearchPage, SearchTarget};
use launcher_shared::{ContentKind, PlannedPick};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, EmptyState, Icon, IconAction, SegOption, Segmented, TabDef, Tabs, TextInput, Variant};

use super::Wizard;
use super::state::content_kinds;
use crate::pages::content::tab_of;
use crate::providers::cards::{ListSkeleton, Pager, ProjectRow};
use crate::providers::{api, content_providers, providers, source_label};
use crate::store::use_store;

fn kind_of(tab: &str) -> ContentKind {
    ContentKind::ALL.into_iter().find(|k| tab_of(*k) == tab).unwrap_or(ContentKind::Mods)
}

fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mods_content_tab",
        ContentKind::ResourcePacks => "resourcepacks",
        ContentKind::ShaderPacks => "shaders_content_tab",
    }
}

fn kind_icon(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "extension",
        ContentKind::ResourcePacks => "palette",
        ContentKind::ShaderPacks => "wb_sunny",
    }
}

#[component]
pub fn Content(wizard: Wizard) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let all = Memo::new(move |_| store.info.with(|i| providers(i.as_ref())));
    // Fabric API (QFAPI for Quilt) in the plan the first time this step opens for the target.
    wizard.state.update(|s| s.offer_starters(&all.get_untracked()));
    let kinds = wizard.state.with_untracked(|s| content_kinds(s.loader));
    let tab = RwSignal::new(tab_of(kinds[0]));
    let tabs = kinds
        .iter()
        .map(|&kind| TabDef {
            id: tab_of(kind),
            icon: kind_icon(kind),
            label: Signal::derive(move || i18n.t(kind_label(kind))),
        })
        .collect::<Vec<_>>();
    let offered = Memo::new(move |_| {
        let kind = kind_of(tab.get());
        let client = wizard.state.with(|s| s.loader.display_name().to_string());
        content_providers(&all.get(), kind, Some(&client))
            .into_iter()
            .filter(|p| p.content_clients.is_empty())
            .collect::<Vec<_>>()
    });
    let source = RwSignal::new(String::new());
    Effect::new(move |_| {
        let ids: Vec<String> = offered.with(|ps| ps.iter().map(|p| p.id.clone()).collect());
        if !ids.contains(&source.get_untracked()) {
            source.set(ids.first().cloned().unwrap_or_default());
        }
    });
    let source_options = Signal::derive(move || {
        offered
            .get()
            .into_iter()
            .map(|p| {
                let label = source_label(&p, |key| i18n.t(key));
                SegOption::new(p.id.clone(), label).with_icon(p.icon.clone())
            })
            .collect::<Vec<_>>()
    });
    let query = RwSignal::new(String::new());
    let page = RwSignal::new(None::<Result<SearchPage, String>>);
    let search = move |offset: u32| {
        let provider = source.get_untracked();
        if provider.is_empty() {
            return;
        }
        let (game_version, loader) = wizard.state.with_untracked(|s| (s.mc.clone(), s.loader));
        let args = SearchArgs {
            key: String::new(),
            kind: kind_of(tab.get_untracked()),
            query: query.get_untracked().trim().to_string(),
            offset,
            target: Some(SearchTarget { game_version, loader }),
        };
        page.set(None);
        spawn_local(async move {
            let answer = api::search(&provider, &args).await.map_err(|e| i18n.error(&e));
            let _ = page.try_set(Some(answer));
        });
    };
    Effect::new(move |_| {
        tab.track();
        source.track();
        search(0);
    });
    let shown_page = Signal::derive(move || page.get().and_then(Result::ok));
    let show_plan = RwSignal::new(false);
    let count = move || wizard.state.with(|s| s.picks.len());
    let toggle = move |hit: ProjectHit| {
        let (provider, kind) = (source.get_untracked(), kind_of(tab.get_untracked()));
        wizard.dropped.set(false);
        wizard.state.update(|s| {
            if s.has_pick(&provider, &hit.project_id) {
                s.remove_pick(&provider, &hit.project_id);
            } else {
                s.add_pick(PlannedPick {
                    provider: provider.clone(),
                    kind,
                    project_id: hit.project_id.clone(),
                    slug: hit.slug.clone(),
                    title: hit.title.clone(),
                });
            }
        });
    };
    let rows = move || {
        match page.get() {
        None => view! { <ListSkeleton /> }.into_any(),
        Some(Err(e)) => view! { <div class="wizard__status is-error"><Icon name="error_outline" /><span>{e}</span></div> }.into_any(),
        Some(Ok(found)) if found.hits.is_empty() => {
            view! { <EmptyState icon="search_off" title=Signal::derive(move || i18n.t("wizard_nothing_found")) /> }.into_any()
        }
        Some(Ok(found)) => found
            .hits
            .into_iter()
            .map(|hit| {
                let (provider, id) = (source.get_untracked(), hit.project_id.clone());
                let picked = Signal::derive(move || wizard.state.with(|s| s.has_pick(&provider, &id)));
                let for_click = hit.clone();
                view! {
                    <ProjectRow hit=hit>
                        {move || {
                            let hit = for_click.clone();
                            if picked.get() {
                                view! {
                                    <Button variant=Variant::Ghost icon="check" on_click=move |_| toggle(hit.clone())>
                                        {move || i18n.t("wizard_added")}
                                    </Button>
                                }.into_any()
                            } else {
                                view! {
                                    <Button variant=Variant::Primary icon="playlist_add" on_click=move |_| toggle(hit.clone())>
                                        {move || i18n.t("wizard_add")}
                                    </Button>
                                }.into_any()
                            }
                        }}
                    </ProjectRow>
                }
            })
            .collect_view()
            .into_any(),
    }
    };
    let plan = move || {
        let picks = wizard.state.with(|s| s.picks.clone());
        if picks.is_empty() {
            return view! { <p class="hint">{move || i18n.t("wizard_plan_empty")}</p> }.into_any();
        }
        picks
            .into_iter()
            .map(|pick| {
                let (provider, id) = (pick.provider.clone(), pick.project_id.clone());
                let source = all.with(|ps| {
                    ps.iter().find(|p| p.id == pick.provider).map_or_else(|| pick.provider.clone(), |p| p.name.clone())
                });
                view! {
                    <div class="build-wizard__pick">
                        <Icon name=kind_icon(pick.kind) />
                        <span class="grow">{pick.title.clone()}</span>
                        <span class="hint">{source}</span>
                        <IconAction
                            icon="close"
                            title=Signal::derive(move || i18n.t("wizard_remove"))
                            on_click=Callback::new(move |()| wizard.state.update(|s| s.remove_pick(&provider, &id)))
                        />
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    view! {
        <div class="build-wizard__content">
            <Show when=move || wizard.dropped.get()>
                <div class="wizard__status is-error"><Icon name="info" /><span>{move || i18n.t("wizard_picks_dropped")}</span></div>
            </Show>
            <div class="build-wizard__toolbar">
                <Tabs tabs=tabs active=tab />
                <span class="wizard__gap"></span>
                <Button
                    variant=Variant::Ghost
                    icon="checklist"
                    on_click=move |_| show_plan.update(|v| *v = !*v)
                >
                    {move || i18n.tp("wizard_plan", &[("count", count().to_string())])}
                </Button>
            </div>
            <Show when=move || show_plan.get()>
                <div class="build-wizard__plan">{plan}</div>
            </Show>
            <div class="build-wizard__search">
                <div class="grow">
                    <TextInput
                        value=query
                        placeholder=Signal::derive(move || i18n.t("wizard_search"))
                        on_enter=Callback::new(move |()| search(0))
                    />
                </div>
                <Show when=move || !offered.with(Vec::is_empty)>
                    <Segmented options=source_options value=source />
                </Show>
            </div>
            <div class="builds__list">{rows}</div>
            <Pager page=shown_page on_page=Callback::new(move |offset: u32| search(offset)) />
        </div>
    }
}
