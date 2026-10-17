//! «Вміст»: the providers' mods, resource packs and shader packs for the build to be, each one
//! put into the plan (or taken out) instead of installed; the plan itself above the list. The tabs,
//! the search and the pages stay where they are: only the list scrolls.

use launcher_shared::provider::{ProjectHit, SearchArgs, SearchPage, SearchTarget};
use launcher_shared::{ContentKind, PlannedPick};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ChipDef, ChipTabs, EmptyState, Icon, IconAction, LatestRequest, SegOption, Segmented, TextInput,
    Variant,
};

use super::state::content_kinds;
use super::{PicksDropped, Wizard};
use crate::pages::content::{tab_icon, tab_of, tab_tone};
use crate::providers::cards::{ListSkeleton, Pager, ProjectRow};
use crate::providers::{DEBOUNCE, api, content_providers, providers, source_label};
use crate::store::use_store;

/// A page of results with what it was searched on: its picks are that provider's and that kind's,
/// whatever the tabs show by the time it is clicked.
#[derive(Clone)]
struct Found {
    provider: String,
    kind: ContentKind,
    page: SearchPage,
}

impl Found {
    fn pick(&self, hit: &ProjectHit) -> PlannedPick {
        PlannedPick {
            provider: self.provider.clone(),
            kind: self.kind,
            project_id: hit.project_id.clone(),
            slug: hit.slug.clone(),
            title: hit.title.clone(),
            icon_url: hit.icon_url.clone(),
        }
    }
}

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

/// The kind's icon, as the build's own content tabs show it.
pub fn kind_icon(kind: ContentKind) -> &'static str {
    tab_icon(tab_of(kind))
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
    // The same chips, icons and colours as the build's own content tabs.
    let chips = kinds
        .iter()
        .map(|&kind| ChipDef {
            id: tab_of(kind),
            icon: kind_icon(kind),
            label: Signal::derive(move || i18n.t(kind_label(kind))),
            tone: tab_tone(tab_of(kind)),
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
    let per_page = Memo::new(move |_| store.settings.with(|s| s.items_per_page));
    let page = RwSignal::new(None::<Result<Found, String>>);
    // Only the last search's answer is shown: an earlier one may come after it.
    let latest = StoredValue::new_local(LatestRequest::new());
    let search = move |offset: u32| {
        let provider = source.get_untracked();
        if provider.is_empty() {
            return;
        }
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        let (game_version, loader) = wizard.state.with_untracked(|s| (s.mc.clone(), s.loader));
        let kind = kind_of(tab.get_untracked());
        let args = SearchArgs {
            key: String::new(),
            kind,
            query: query.get_untracked().trim().to_string(),
            offset,
            target: Some(SearchTarget { game_version, loader }),
            limit: Some(per_page.get_untracked()),
        };
        page.set(None);
        spawn_local(async move {
            let answer = api::search(&provider, &args).await;
            let found = answer.map(|page| Found { provider, kind, page }).map_err(|e| i18n.error(&e));
            if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                let _ = page.try_set(Some(found));
            }
        });
    };
    Effect::new(move |_| {
        tab.track();
        source.track();
        per_page.track();
        search(0);
    });
    // Typing searches again after a pause; Enter at once.
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let cancel_timer = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    Effect::new(move |previous: Option<String>| {
        let text = query.get().trim().to_string();
        if previous.is_some_and(|p| p != text) {
            cancel_timer();
            timer.set_value(set_timeout_with_handle(move || search(0), DEBOUNCE).ok());
        }
        text
    });
    on_cleanup(cancel_timer);
    let shown_page = Signal::derive(move || page.get().and_then(Result::ok).map(|found| found.page));
    let show_plan = RwSignal::new(false);
    let count = move || wizard.state.with(|s| s.picks.len());
    let toggle = move |pick: PlannedPick| {
        wizard.state.update(|s| {
            if s.has_pick(&pick.provider, &pick.project_id) {
                s.remove_pick(&pick.provider, &pick.project_id);
            } else {
                s.add_pick(pick);
            }
        });
    };
    let rows = move || {
        match page.get() {
        None => view! { <ListSkeleton /> }.into_any(),
        Some(Err(e)) => view! { <div class="wizard__status is-error"><Icon name="error_outline" /><span>{e}</span></div> }.into_any(),
        Some(Ok(found)) if found.page.hits.is_empty() => {
            view! { <EmptyState icon="search_off" title=Signal::derive(move || i18n.t("wizard_nothing_found")) /> }.into_any()
        }
        Some(Ok(found)) => found
            .page
            .hits
            .iter()
            .map(|hit| {
                let pick = found.pick(hit);
                let (provider, id) = (pick.provider.clone(), pick.project_id.clone());
                let picked = Signal::derive(move || wizard.state.with(|s| s.has_pick(&provider, &id)));
                view! {
                    <ProjectRow hit=hit.clone()>
                        {move || {
                            let pick = pick.clone();
                            if picked.get() {
                                view! {
                                    <Button variant=Variant::Ghost icon="check" on_click=move |_| toggle(pick.clone())>
                                        {move || i18n.t("wizard_added")}
                                    </Button>
                                }.into_any()
                            } else {
                                view! {
                                    <Button variant=Variant::Primary icon="playlist_add" on_click=move |_| toggle(pick.clone())>
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

    let list = NodeRef::<leptos::html::Div>::new();
    let to_page = move |offset: u32| {
        if let Some(list) = list.get_untracked() {
            list.set_scroll_top(0);
        }
        search(offset);
    };

    view! {
        <div class="build-wizard__content">
            <PicksDropped wizard=wizard />
            <nav class="content__nav">
                <ChipTabs chips=chips active=tab />
                <Button
                    variant=Variant::Ghost
                    icon="checklist"
                    on_click=move |_| show_plan.update(|v| *v = !*v)
                >
                    {move || i18n.tp("wizard_plan", &[("count", count().to_string())])}
                </Button>
            </nav>
            <Show when=move || show_plan.get()>
                <div class="build-wizard__plan">{plan}</div>
            </Show>
            <div class="create__bar">
                <div class="create__tools">
                    <div class="create__search">
                        <TextInput
                            value=query
                            icon="search"
                            placeholder=Signal::derive(move || i18n.t("wizard_search"))
                            on_enter=Callback::new(move |()| {
                                cancel_timer();
                                search(0);
                            })
                        />
                    </div>
                    <Show when=move || !offered.with(Vec::is_empty)>
                        <Segmented options=source_options value=source />
                    </Show>
                </div>
            </div>
            <div node_ref=list class="builds__list build-wizard__list">{rows}</div>
            <Pager page=shown_page on_page=Callback::new(to_page) />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_belongs_to_the_search_it_came_from() {
        let hit = ProjectHit {
            project_id: "abc".into(),
            slug: "faithful".into(),
            title: "Faithful".into(),
            author: "x".into(),
            description: String::new(),
            downloads: 1,
            icon_url: Some("https://cdn.example/faithful.png".into()),
            url: None,
        };
        // The answer of a search made on CurseForge's resource packs, though the tabs moved on.
        let found = Found {
            provider: "curseforge".into(),
            kind: ContentKind::ResourcePacks,
            page: SearchPage::default(),
        };
        let pick = found.pick(&hit);
        assert_eq!((pick.provider.as_str(), pick.kind), ("curseforge", ContentKind::ResourcePacks));
        assert_eq!(
            (pick.project_id.as_str(), pick.slug.as_str(), pick.title.as_str()),
            ("abc", "faithful", "Faithful")
        );
        assert_eq!(pick.icon_url.as_deref(), Some("https://cdn.example/faithful.png"), "for the summary");
    }
}
