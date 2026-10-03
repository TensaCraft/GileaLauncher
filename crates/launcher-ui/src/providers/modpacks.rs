//! A provider's modpacks on the app's "Modpacks" page: a page at a time; a pack's
//! page on the provider's site and "Install" — a new build from the version picked in the dialog
//! (the original's ModpackInstallModal).

use std::time::Duration;

use launcher_shared::naming::{check_new_name, default_build_name};
use launcher_shared::provider::{
    HeldFile, PackArgs, PackInstallArgs, PackVersion, PacksArgs, ProjectHit, ProviderInfo, SearchPage,
    held_files,
};
use launcher_shared::{BuildDto, BuildsSnapshot, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionGap, ActionTone, Button, Dialog, DialogFooter, EmptyState, Field, IconAction, LatestRequest,
    Reloadable, Select, SelectOption, TextInput, Variant, ipc, use_toasts,
};

use super::cards::{ListSkeleton, Pager, ProjectRow};
use super::held::HeldDialog;
use super::{api, describe, open_page};

/// The original's pause after typing.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// Characters of a modpack's description.
const DESCRIPTION: usize = 160;

#[component]
pub fn ProviderModpacks(
    provider: ProviderInfo,
    /// The page's switch between sources, placed right after the search.
    #[prop(optional)]
    sources: Option<ViewFn>,
) -> impl IntoView {
    let (i18n, toasts) = (use_i18n(), use_toasts());
    let info = StoredValue::new(provider.clone());
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let query = RwSignal::new(String::new());
    // Reloads in place: a new search keeps the page shown until its answer; the error is
    // already translated.
    let pages = Reloadable::<SearchPage>::new();
    let page = pages.shown();
    let searching = RwSignal::new(false);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let search = move |offset: u32| {
        let Some(request) = pages.begin() else { return };
        searching.set(true);
        let args = PacksArgs { query: query.get_untracked().trim().to_string(), offset };
        // Taken now: the page may be gone when the answer comes.
        let Some(provider) = info.try_get_value() else { return };
        spawn_local(async move {
            let result = api::modpacks(&provider.id, &args).await;
            let _ = searching.try_set(false);
            if let Some(told) = pages.finish(request, result.map_err(|e| describe(i18n, &provider, &e))) {
                toasts.show(Level::Error, told, None);
            }
        });
    };
    let cancel_timer = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    search(0);
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

    let dialog_open = RwSignal::new(false);
    let chosen = RwSignal::new(None::<ProjectHit>);
    let card = move |hit: ProjectHit| {
        let (for_site, for_install) = (hit.clone(), hit.clone());
        view! {
            <ProjectRow hit=hit description=DESCRIPTION>
                {match for_site.url.clone() {
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
                    variant=Variant::Primary
                    icon="download"
                    on_click=Callback::new(move |()| {
                        chosen.set(Some(for_install.clone()));
                        dialog_open.set(true);
                    })
                >
                    {move || i18n.t("minecraft_components_install_action")}
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
            view! { <EmptyState icon="search_off" title=t("no_modpacks_found") /> }.into_any()
        }
        Some(Ok(p)) => {
            view! { <div class="builds__list">{p.hits.into_iter().map(card).collect_view()}</div> }.into_any()
        }
    }
    };

    view! {
        <div class="create__bar">
            <div class="create__tools">
                <div class="create__search">
                    <TextInput
                        value=query
                        icon="search"
                        autofocus=true
                        placeholder=t("search_modpacks")
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
                    on_click=Callback::new(move |()| search(offset_now()))
                />
            </div>
        </div>
        {body}
        <Pager
            page=Signal::derive(move || page.with(|p| p.as_ref().and_then(|r| r.as_ref().ok()).cloned()))
            on_page=Callback::new(search)
        />
        <PackInstallDialog provider=provider open=dialog_open hit=chosen />
    }
}

/// The original's ModpackInstallModal: the build's name (the pack's, made unique) and the pack's
/// version, newest first; the name is checked as the app's own dialogs check it.
#[component]
fn PackInstallDialog(
    provider: ProviderInfo,
    open: RwSignal<bool>,
    hit: RwSignal<Option<ProjectHit>>,
) -> impl IntoView {
    let (i18n, toasts) = (use_i18n(), use_toasts());
    let info = StoredValue::new(provider);
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let navigate = StoredValue::new(use_navigate());
    let latest = StoredValue::new_local(LatestRequest::new());
    let name = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let builds = RwSignal::new(Vec::<BuildDto>::new());
    // `None` while loading; the error is already translated.
    let versions = RwSignal::new(None::<Result<Vec<PackVersion>, String>>);
    let picked = RwSignal::new(String::new());
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(h) = hit.get_untracked() else { return };
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        name.set(h.title.clone());
        error.set(None);
        versions.set(None);
        let args = PackArgs { project_id: h.project_id.clone() };
        let Some(provider) = info.try_get_value() else { return };
        spawn_local(async move {
            let list = ipc::call::<BuildsSnapshot>("builds_list").await.map(|s| s.builds).unwrap_or_default();
            let answer = match api::modpack_versions(&provider.id, &args).await {
                Ok(found) if !found.is_empty() => Ok(found),
                Ok(_) => Err(i18n.t("modpack_details_not_found")),
                Err(e) => Err(describe(i18n, &provider, &e)),
            };
            if !latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                return;
            }
            if let Some(free) = name
                .try_with_untracked(|typed| free_name(typed, &h.title, default_build_name(&h.title, &list)))
                .flatten()
            {
                let _ = name.try_set(free);
            }
            let _ = builds.try_set(list);
            if let Ok(found) = &answer {
                let _ = picked.try_set(found[0].id.clone());
            }
            let _ = versions.try_set(Some(answer));
        });
    });
    let loaded = move || versions.with(|v| matches!(v, Some(Ok(_))));
    let options = Signal::derive(move || {
        versions.with(|v| match v {
            Some(Ok(found)) => found.iter().map(|v| SelectOption::new(v.id.clone(), v.label())).collect(),
            _ => Vec::new(),
        })
    });
    // Files to download by hand before the install can go on, and the install that waits on them.
    let held = RwSignal::new(Vec::<HeldFile>::new());
    let waiting = StoredValue::new(None::<PackInstallArgs>);
    let run = Callback::new(move |args: PackInstallArgs| {
        // Taken now: the page may be gone when the install ends.
        let Some(provider) = info.try_get_value() else { return };
        spawn_local(async move {
            match api::install_modpack(&provider.id, &args).await {
                Ok(_) => {
                    let _ = navigate.try_with_value(|go| go("/builds", Default::default()));
                }
                Err(e) if !held_files(&e).is_empty() => {
                    waiting.try_set_value(Some(args));
                    // The page is gone by now: the user still hears which files to download.
                    if held.try_set(held_files(&e)).is_some() {
                        toasts.show(Level::Error, describe(i18n, &provider, &e), None);
                    }
                }
                Err(e) => toasts.show(Level::Error, describe(i18n, &provider, &e), None),
            }
        });
    });
    let go_on = Callback::new(move |()| {
        if let Some(args) = waiting.try_update_value(Option::take).flatten() {
            run.run(args);
        }
    });
    let open_held = Callback::new(move |url: String| {
        if let Some(provider) = info.try_get_value() {
            open_page(i18n, toasts, &provider, url);
        }
    });
    let submit = move || {
        let Some(h) = hit.get_untracked().filter(|_| loaded()) else { return };
        let raw = name.get_untracked();
        match builds.with_untracked(|b| check_new_name(&raw, b)) {
            Err(problem) => error.set(Some(i18n.tp(problem.key(), &[("name", raw.trim().to_string())]))),
            Ok(name) => {
                open.set(false);
                run.run(PackInstallArgs {
                    project_id: h.project_id,
                    version_id: picked.get_untracked(),
                    name,
                    icon_url: h.icon_url,
                });
            }
        }
    };
    let title = move || hit.with(|h| h.as_ref().map(|h| h.title.clone()).unwrap_or_default());

    view! {
        <Dialog
            open=open
            title=Signal::derive(move || i18n.tp("version_create_install_confirm_title", &[("version", title())]))
            subtitle=t("version_create_install_confirm_message")
            icon="download"
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="download"
                    disabled=Signal::derive(move || !loaded())
                    on_click=move |_| submit()
                >
                    {move || i18n.t("minecraft_components_install_action")}
                </Button>
            </DialogFooter>
            <Field label=t("version_name_label") error=Signal::derive(move || error.get())>
                <TextInput
                    value=name
                    autofocus=true
                    invalid=Signal::derive(move || error.get().is_some())
                    on_enter=Callback::new(move |()| submit())
                />
            </Field>
            <Field
                label=t("modpack_version_label")
                hint=Signal::derive(move || versions.with(Option::is_none).then(|| i18n.t("loading_modpack_details")))
                error=Signal::derive(move || versions.with(|v| v.as_ref().and_then(|r| r.as_ref().err().cloned())))
            >
                <Select options=options value=picked disabled=Signal::derive(move || !loaded()) />
            </Field>
        </Dialog>
        <HeldDialog
            provider=Signal::derive(move || info.try_with_value(|p| p.name.clone()).unwrap_or_default())
            held=held
            on_continue=go_on
            on_open=open_held
        />
    }
}

/// The free name for the field (`free`) while it still shows the pack's title; a name the user
/// typed stays.
fn free_name(typed: &str, title: &str, free: String) -> Option<String> {
    (typed == title).then_some(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_name_outlives_the_list_of_builds() {
        // The field shows the pack's title at once and takes focus; the free name comes with the
        // list of builds a moment later, and must not replace what the user typed meanwhile.
        assert_eq!(free_name("Pack", "Pack", "Pack (2)".into()), Some("Pack (2)".to_string()));
        assert_eq!(free_name("My pack", "Pack", "Pack (2)".into()), None);
        assert_eq!(free_name("", "Pack", "Pack (2)".into()), None);
    }
}
