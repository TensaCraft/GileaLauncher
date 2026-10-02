//! A provider's tab of "Build content": this build's
//! kind of content from the provider, a page at a time, narrowed by its Minecraft version (and
//! loader, for mods); one project installs at a time. A project installed here with a newer version
//! offers "Update" on its card, as its row in the Installed list does.

use std::time::Duration;

use launcher_shared::provider::{
    InstallArgs, ProjectHit, ProviderInfo, SearchArgs, SearchPage, filter_text, not_found_key, search_key,
};
use launcher_shared::{BuildDto, ContentKind, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionGap, ActionTone, Button, EmptyState, Icon, IconAction, Reloadable, Size, TextInput, Variant,
    use_toasts,
};

use super::cards::{ListSkeleton, Pager, ProjectRow};
use super::flow::{InstallFlow, installs};
use super::installed::{UpdateConfirm, load};
use super::store::{UpdateAsk, store};
use super::{api, describe, open_page};

/// The original's pause after typing.
const DEBOUNCE: Duration = Duration::from_millis(350);

#[component]
pub fn ProviderPanel(
    provider: ProviderInfo,
    key: String,
    kind: ContentKind,
    current: Signal<Option<BuildDto>>,
    /// The page's switch of where to look, placed right after the search.
    #[prop(optional)]
    sources: Option<ViewFn>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let key = StoredValue::new(key);
    let id = StoredValue::new(provider.id.clone());
    let info = StoredValue::new(provider.clone());
    let query = RwSignal::new(String::new());
    // `None` until the first answer: a new search keeps the page shown until its own answer comes,
    // and a failed one keeps it with a toast (the page does not collapse and jump to the top). The
    // error is already translated.
    let pages = Reloadable::<SearchPage>::new();
    let page = pages.shown();
    let searching = RwSignal::new(false);
    // The top of the list: a page change brings it into view on purpose.
    let top = NodeRef::<leptos::html::Div>::new();
    let to_top = StoredValue::new(false);
    let timer = StoredValue::new(None::<TimeoutHandle>);

    let search = move |offset: u32| {
        let Some(request) = pages.begin() else { return };
        searching.set(true);
        let args = SearchArgs {
            key: key.get_value(),
            kind,
            query: query.get_untracked().trim().to_string(),
            offset,
        };
        // Taken now: the tab may be gone when the answer comes.
        let Some(provider) = info.try_get_value() else { return };
        spawn_local(async move {
            let result = api::search(&provider.id, &args).await;
            if pages.is_current(request) {
                let told = pages.finish(request, result.map_err(|e| describe(i18n, &provider, &e)));
                let _ = searching.try_set(false);
                if let Some(told) = told {
                    toasts.show(Level::Error, told, None);
                } else if to_top.try_get_value().unwrap_or(false) {
                    to_top.set_value(false);
                    if let Some(anchor) = top.get_untracked() {
                        anchor.scroll_into_view();
                    }
                }
            }
        });
    };
    // What is installed, and which of it has a newer version: the tab's overview in the store.
    let load_installed = move || info.with_value(|p| load(p, key.get_value(), kind));
    let cancel_timer = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    search(0);
    // What is installed: now, and again whenever an install ends (this panel's or an earlier one's).
    let ended = RwSignal::from(installs().ended);
    Effect::new(move |_| {
        ended.track();
        load_installed();
    });
    // Typing searches again after a pause; the first run only remembers the text.
    Effect::new(move |previous: Option<String>| {
        let text = query.get().trim().to_string();
        if previous.is_some_and(|p| p != text) {
            cancel_timer();
            timer.set_value(set_timeout_with_handle(move || search(0), DEBOUNCE).ok());
        }
        text
    });
    on_cleanup(cancel_timer);
    let offset_now =
        move || page.with_untracked(|p| p.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, |p| p.offset));

    let flow = InstallFlow::new(provider.clone(), Callback::new(move |_| load_installed()));
    let confirm = UpdateConfirm::new(flow);
    let install = move |hit: ProjectHit| {
        flow.start(InstallArgs::new(key.get_value(), kind, hit.project_id, hit.slug, hit.title));
    };

    let card = move |hit: ProjectHit| {
        let project = hit.project_id.clone();
        let is_installed = {
            let project = project.clone();
            Signal::derive(move || {
                store()
                    .overview(&id.get_value(), &key.get_value(), kind)
                    .is_some_and(|o| o.notes.iter().any(|n| n.project_id == project))
            })
        };
        let newer = {
            let project = project.clone();
            Memo::new(move |_| store().update_of(&id.get_value(), &key.get_value(), kind, &project))
        };
        let busy = Signal::derive(move || installs().running(&id.get_value(), &key.get_value(), &project));
        let (site, for_install, title) = (hit.url.clone(), hit.clone(), hit.title.clone());
        // Installed with a newer version: "Update", after the same confirmation as in the list.
        let act = move || match newer.get_untracked() {
            Some(note) => confirm.ask(UpdateAsk {
                provider: id.get_value(),
                key: key.get_value(),
                kind,
                note,
                name: title.clone(),
            }),
            None => install(for_install.clone()),
        };
        view! {
            <ProjectRow hit=hit>
                {match site {
                    Some(url) => view! {
                        <IconAction
                            icon="open_in_new"
                            tone=ActionTone::Info
                            title=t("open_on_site")
                            on_click=Callback::new(move |()| info.with_value(|p| open_page(i18n, toasts, p, url.clone())))
                        />
                    }
                    .into_any(),
                    None => view! { <ActionGap /> }.into_any(),
                }}
                <Button
                    size=Size::Md
                    variant=Variant::Primary
                    icon="download"
                    disabled=Signal::derive(move || is_installed.get() && newer.with(Option::is_none))
                    loading=busy
                    on_click=Callback::new(move |()| act())
                >
                    {move || match (newer.with(Option::is_some), is_installed.get()) {
                        (true, _) => i18n.t("update_modrinth_content"),
                        (false, true) => i18n.t("installed"),
                        (false, false) => i18n.t("minecraft_components_install_action"),
                    }}
                </Button>
            </ProjectRow>
        }
    };
    let body = move || {
        match page.get() {
        None => view! { <ListSkeleton /> }.into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="cloud_off" title=message>
                <Button icon="refresh" on_click=move |_| search(offset_now())>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(p)) if p.hits.is_empty() => {
            view! { <EmptyState icon="search_off" title=t(not_found_key(kind)) /> }.into_any()
        }
        Some(Ok(p)) => {
            view! { <div class="builds__list">{p.hits.into_iter().map(card).collect_view()}</div> }.into_any()
        }
    }
    };
    let filter = move || {
        current
            .with(|b| {
                b.as_ref().and_then(|b| {
                    filter_text(kind, b.loader.as_deref(), b.client.as_deref(), b.version.as_deref())
                })
            })
            .map(|(text_key, params)| i18n.tp(text_key, &params))
    };

    // A module's word for this build (a server's build: what is added here may go at its next sync).
    let parts = crate::modules::use_module_parts();
    let notices =
        move || current.with(|b| b.as_ref().map(|b| parts.with(|p| p.notices_for(b))).unwrap_or_default());

    view! {
        <div node_ref=top class="create__bar">
            {move || filter().map(|text| view! { <span class="provider__filter"><Icon name="filter_alt" />{text}</span> })}
            <div class="create__tools">
                <div class="create__search">
                    <TextInput
                        value=query
                        icon="search"
                        placeholder=t(search_key(kind))
                        on_enter=Callback::new(move |()| {
                            cancel_timer();
                            search(0);
                        })
                    />
                </div>
                {sources.map(|s| s.run())}
                <IconAction
                    icon="refresh"
                    title=t("refresh")
                    loading=Signal::derive(move || searching.get())
                    on_click=Callback::new(move |()| {
                        search(offset_now());
                        load_installed();
                    })
                />
            </div>
        </div>
        // Right above the list it is about.
        {move || {
            notices()
                .into_iter()
                .map(|text| view! { <div class="content-notice"><Icon name="warning" /><span>{i18n.t(text)}</span></div> })
                .collect_view()
        }}
        {body}
        <Pager
            page=Signal::derive(move || page.with(|p| p.as_ref().and_then(|r| r.as_ref().ok()).cloned()))
            on_page=Callback::new(move |offset| {
                to_top.set_value(true);
                search(offset);
            })
        />
        {flow.dialog()}
        {confirm.view()}
    }
}
