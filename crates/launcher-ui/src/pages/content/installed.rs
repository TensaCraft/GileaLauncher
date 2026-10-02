//! A build's installed mods, resource packs or shader packs: search, switch
//! on or off, delete, put a mod back from its backup — one change at a time. Content providers add
//! their parts beside the tools and before each row's actions.

use launcher_shared::provider::{Need, ProviderInfo, UpdateSummary, UpdatesStatus, updates_text};
use launcher_shared::{BuildDto, ContentItem, ContentKind, ContentList, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionGroup, ActionTone, Button, ConfirmDialog, EmptyState, IconAction, Skeleton, TextInput, ipc,
    use_toasts,
};

use super::{display_name, matches, subtitle, texts};
use crate::builds::LatestRequest;
use crate::providers::installed::{ContentIcon, ProviderRowActions, ProviderTools, updates_badge};
use crate::providers::store::store;

#[derive(serde::Serialize)]
struct ListArgs {
    key: String,
    kind: ContentKind,
}

#[derive(serde::Serialize)]
struct FileArgs {
    key: String,
    kind: ContentKind,
    file: String,
    /// For a switch: the state the user asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    enable: Option<bool>,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListState {
    /// The first list is on its way.
    Loading,
    Failed(String),
    Unsupported,
    Empty,
    /// Nothing matches the search.
    NoMatch,
    Rows,
}

/// The panel's state from its list and how many of its items the search leaves.
pub fn list_state(list: &Option<Result<ContentList, String>>, visible: usize) -> ListState {
    match list {
        None => ListState::Loading,
        Some(Err(message)) => ListState::Failed(message.clone()),
        Some(Ok(l)) if !l.supported => ListState::Unsupported,
        Some(Ok(l)) if l.items.is_empty() => ListState::Empty,
        Some(Ok(_)) if visible == 0 => ListState::NoMatch,
        Some(Ok(_)) => ListState::Rows,
    }
}

#[component]
pub fn InstalledPanel(
    key: String,
    kind: ContentKind,
    current: Signal<Option<BuildDto>>,
    /// The providers whose parts the list shows.
    #[prop(optional)]
    providers: Vec<ProviderInfo>,
    /// The page's switch of where to look, placed right after the search.
    #[prop(optional)]
    sources: Option<ViewFn>,
    /// Told how many the list has.
    #[prop(optional)]
    count: Option<RwSignal<Option<usize>>>,
) -> impl IntoView {
    let build = current;
    let providers = StoredValue::new(providers);
    // Grows each time a list arrives, so the providers' parts follow it.
    let revision = RwSignal::new(0_u64);
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let key = StoredValue::new(key);
    let texts = texts(kind);
    // `None` until the first list; a reload keeps what is shown. The error is already translated.
    let list = RwSignal::new(None::<Result<ContentList, String>>);
    let query = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let latest = StoredValue::new_local(LatestRequest::new());
    let load = move || {
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        let args = ListArgs { key: key.get_value(), kind };
        spawn_local(async move {
            let result = ipc::invoke::<_, ContentList>("content_list", &args).await;
            if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                let arrived = result.is_ok();
                let Some(previous) = list.try_get_untracked() else { return };
                let (shown, told) = ui_kit::lists::settle(previous, result.map_err(|e| i18n.error(&e)));
                let _ = list.try_set(shown);
                if let Some(error) = told {
                    toasts.show(Level::Error, error, None);
                }
                if arrived {
                    let _ = revision.try_update(|n| *n += 1);
                }
            }
        });
    };
    load();
    let reload = Callback::new(move |()| load());
    // The game may have switched packs while it ran: list again once it has closed.
    Effect::new(move |was_running: Option<bool>| {
        let running = build.with(|b| b.as_ref().is_some_and(|b| b.running));
        if was_running == Some(true) && !running {
            load();
        }
        running
    });
    // One change at a time; its answer is the fresh list (the backend announces what it did).
    let change = move |command: &'static str, item: ContentItem, enable: Option<bool>| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        let args = FileArgs { key: key.get_value(), kind, file: item.file, enable };
        spawn_local(async move {
            match ipc::invoke::<_, ContentList>(command, &args).await {
                Ok(fresh) => {
                    if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                        let _ = list.try_set(Some(Ok(fresh)));
                        let _ = revision.try_update(|n| *n += 1);
                    }
                }
                Err(e) => {
                    toasts.show(Level::Error, i18n.error(&e), None);
                    load();
                }
            }
            let _ = pending.try_set(false);
        });
    };
    let open_dir = move || {
        let args = ListArgs { key: key.get_value(), kind };
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, Value>("content_open_dir", &args).await {
                toasts.show(Level::Error, i18n.error(&e), None);
            }
        });
    };

    let delete_of = RwSignal::new(None::<ContentItem>);
    let delete_open = RwSignal::new(false);
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        if let Some(item) = delete_of.get_untracked() {
            change("content_delete", item, None);
        }
    });
    let delete_text = Signal::derive(move || {
        delete_of
            .with(|i| i.as_ref().map(|i| i18n.tp(texts.confirm_delete, &[("name", display_name(i))])))
            .unwrap_or_default()
    });

    let restore_of = RwSignal::new(None::<ContentItem>);
    let restore_open = RwSignal::new(false);
    let confirm_restore = Callback::new(move |()| {
        restore_open.set(false);
        if let Some(item) = restore_of.get_untracked() {
            change("content_restore", item, None);
        }
    });
    let restore_text = Signal::derive(move || {
        restore_of
            .with(|i| i.as_ref().map(|i| i18n.tp("confirm_restore_backup", &[("name", display_name(i))])))
            .unwrap_or_default()
    });

    let row = move |item: ContentItem| {
        let (for_toggle, for_delete, for_restore) = (item.clone(), item.clone(), item.clone());
        // One provider's actions: the one the row is shown with (`Store::owner`).
        let module_actions = {
            let all = providers.get_value();
            let order: Vec<String> = all.iter().map(|p| p.id.clone()).collect();
            let file = item.file.clone();
            let owner = Memo::new(move |_| store().owner(&order, &key.get_value(), kind, &file));
            let item = item.clone();
            move || {
                let id = owner.get()?;
                let p = all.iter().find(|p| p.id == id)?.clone();
                Some(
                    view! { <ProviderRowActions provider=p key=key.get_value() kind=kind item=item.clone() /> },
                )
            }
        };
        let sub = subtitle(&item);
        let meta = format!("{} • {}", item.filename, launcher_shared::units::size(item.size, &i18n.lang()));
        let toggle_title = if item.enabled { texts.disable } else { texts.enable };
        let (toggle_icon, toggle_tone) =
            if item.enabled { ("toggle_on", ActionTone::Ok) } else { ("toggle_off", ActionTone::Neutral) };
        view! {
            <div class="list-row build-row content-row" class:is-off=!item.enabled>
                <ContentIcon
                    providers=providers.get_value()
                    key=key.get_value()
                    kind=kind
                    file=item.file.clone()
                    enabled=item.enabled
                />
                <div class="build-row__text">
                    <div class="build-row__name">{display_name(&item)}</div>
                    {(!sub.is_empty()).then(|| view! { <div class="build-row__sub">{sub}</div> })}
                    <div class="build-row__sub component__meta">{meta}</div>
                </div>
                // Occasional actions first (restore, the providers' update and page), then the
                // permanent ones: a row without some leaves no hole between buttons.
                <div class="build-row__actions">
                    {item.has_backup.then(|| {
                        view! {
                            <IconAction
                                icon="settings_backup_restore"
                                tone=ActionTone::Info
                                title=t("restore_backup")
                                on_click=Callback::new(move |()| {
                                    restore_of.set(Some(for_restore.clone()));
                                    restore_open.set(true);
                                })
                            />
                        }
                    })}
                    {module_actions}
                    {item.toggle_supported.then(|| view! {
                        <IconAction
                            icon=toggle_icon
                            tone=toggle_tone
                            title=t(toggle_title)
                            on_click=Callback::new(move |()| change("content_toggle", for_toggle.clone(), Some(!for_toggle.enabled)))
                        />
                    })}
                    <IconAction
                        icon="delete_outline"
                        tone=ActionTone::Danger
                        title=t("delete")
                        on_click=Callback::new(move |()| {
                            delete_of.set(Some(for_delete.clone()));
                            delete_open.set(true);
                        })
                    />
                </div>
            </div>
        }
    };
    // The rows the search leaves; a row is rebuilt only when its file changes (the rest stay put).
    let visible = Memo::new(move |_| {
        list.with(|l| match l {
            Some(Ok(l)) => l.items.iter().filter(|i| matches(i, &query.get())).cloned().collect::<Vec<_>>(),
            _ => Vec::new(),
        })
    });
    let state = Memo::new(move |_| list.with(|l| list_state(l, visible.with(Vec::len))));
    let body = move || {
        match state.get() {
        ListState::Loading => view! {
            <div class="builds__list">
                {(0..4)
                    .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=48 /><Skeleton width=260 /></div> })
                    .collect_view()}
            </div>
        }
        .into_any(),
        ListState::Failed(message) => view! {
            <EmptyState icon="error_outline" title=t("unknown_error") desc=message>
                <Button icon="refresh" on_click=move |_| load()>{move || i18n.t("refresh")}</Button>
            </EmptyState>
        }
        .into_any(),
        ListState::Unsupported => {
            view! { <EmptyState icon="block" title=t("mods_not_supported") desc=t("mods_not_supported_desc") /> }
                .into_any()
        }
        ListState::Empty => view! { <EmptyState icon="inventory_2" title=t(texts.empty) /> }.into_any(),
        ListState::NoMatch => {
            view! { <EmptyState icon="search_off" title=t("no_installed_content_found") /> }.into_any()
        }
        ListState::Rows => view! {
            <div class="builds__list">
                <For
                    each=move || visible.get()
                    key=|i: &ContentItem| (i.file.clone(), i.enabled, i.has_backup, i.size, i.version.clone(), i.name.clone())
                    children=row
                />
            </div>
        }
        .into_any(),
    }
    };
    // The providers that check this kind's updates: the toolbar's "Check for updates".
    let checking: Vec<ProviderInfo> =
        providers.get_value().into_iter().filter(|p| p.offers(Need::Updates(kind))).collect();
    // How many there are: the page shows it on "Installed".
    if let Some(count) = count {
        Effect::new(move |_| {
            let n = list.with(|l| match l {
                Some(Ok(l)) if l.supported => Some(l.items.len()),
                _ => None,
            });
            if n.is_some() {
                count.set(n);
            }
        });
    }
    // The updates waiting, all providers' together: a badge on "Check for updates", one line in
    // its tooltip. A row shown with one provider counts once.
    let waiting = {
        let checking = checking.clone();
        move || -> usize {
            let order: Vec<String> = providers.with_value(|all| all.iter().map(|p| p.id.clone()).collect());
            checking.iter().map(|p| store().owned_updates(&order, &p.id, &key.get_value(), kind)).sum()
        }
    };

    view! {
        <div class="create__bar">
            // At the right, in this order: the search, where to look, refresh, updates and the folder.
            <div class="create__tools">
                <div class="create__search">
                    <TextInput value=query icon="search" placeholder=t(texts.search) />
                </div>
                {sources.map(|s| s.run())}
                <ActionGroup>
                    <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load()) />
                    {(!checking.is_empty()).then(|| {
                        let busy = checking.clone();
                        let checking = checking.clone();
                        let (for_badge, for_tip) = (waiting.clone(), waiting.clone());
                        view! {
                            <IconAction
                                icon="update"
                                tone=ActionTone::Info
                                title=Signal::derive(move || {
                                    match for_tip() {
                                        0 => i18n.t("check_updates_now"),
                                        available => {
                                            let summary = UpdateSummary { status: UpdatesStatus::Available, available, unchecked: 0 };
                                            let (text_key, params) = updates_text(&summary, kind);
                                            i18n.tp(text_key, &params)
                                        }
                                    }
                                })
                                badge=Signal::derive(move || updates_badge(for_badge()))
                                loading=Signal::derive(move || busy.iter().any(|p| store().checking(&p.id, &key.get_value(), kind)))
                                on_click=Callback::new(move |()| {
                                    for p in &checking {
                                        crate::providers::installed::load(p, key.get_value(), kind);
                                    }
                                })
                            />
                        }
                    })}
                    <IconAction
                        icon="folder_open"
                        tone=ActionTone::Info
                        title=t("open_directory")
                        on_click=Callback::new(move |()| open_dir())
                    />
                </ActionGroup>
            </div>
        </div>
        // Made once: a provider's part (its update check and dialogs) keeps its own state and
        // follows `revision`.
        {providers
            .get_value()
            .into_iter()
            .map(|p| view! {
                <ProviderTools provider=p key=key.get_value() kind=kind revision=Signal::from(revision) reload=reload />
            })
            .collect_view()}
        {body}
        <ConfirmDialog
            open=delete_open
            danger=true
            title=t("confirmation")
            message=delete_text
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=confirm_delete
        />
        <ConfirmDialog
            open=restore_open
            title=t("confirmation")
            message=restore_text
            confirm_label=t("restore_backup")
            cancel_label=t("cancel")
            on_confirm=confirm_restore
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_names_the_wanted_state() {
        let toggle = FileArgs {
            key: "aero".into(),
            kind: ContentKind::Mods,
            file: "a.jar".into(),
            enable: Some(false),
        };
        assert_eq!(
            serde_json::to_value(toggle).unwrap(),
            serde_json::json!({"key": "aero", "kind": "mods", "file": "a.jar", "enable": false})
        );
        let delete = FileArgs {
            key: "aero".into(),
            kind: ContentKind::ShaderPacks,
            file: "b.zip".into(),
            enable: None,
        };
        assert_eq!(
            serde_json::to_value(delete).unwrap(),
            serde_json::json!({"key": "aero", "kind": "shaderpacks", "file": "b.zip"})
        );
    }

    fn list(files: &[&str]) -> ContentList {
        ContentList {
            kind: ContentKind::Mods,
            supported: true,
            items: files
                .iter()
                .map(|f| ContentItem {
                    file: (*f).into(),
                    filename: (*f).into(),
                    size: 1,
                    enabled: true,
                    folder: false,
                    toggle_supported: true,
                    name: None,
                    version: None,
                    description: None,
                    mod_id: None,
                    has_backup: false,
                })
                .collect(),
        }
    }

    #[test]
    fn the_first_load_shows_skeletons() {
        assert_eq!(list_state(&None, 0), ListState::Loading);
    }

    #[test]
    fn a_reload_keeps_the_list_on_screen() {
        // A reload that fails while a list is shown keeps the list and tells the error.
        let shown = Some(Ok(list(&["a.jar", "b.jar"])));
        let (kept, told) = ui_kit::lists::settle(shown.clone(), Err("offline".to_string()));
        assert_eq!((kept, told.as_deref()), (shown.clone(), Some("offline")));
        let (fresh, told) = ui_kit::lists::settle(shown.clone(), Ok(list(&["a.jar"])));
        assert_eq!((fresh, told), (Some(Ok(list(&["a.jar"]))), None));
        assert_eq!(
            ui_kit::lists::settle::<ContentList>(None, Err("x".into())),
            (Some(Err("x".into())), None)
        );
        assert_eq!(list_state(&shown, 2), ListState::Rows);
        assert_eq!(list_state(&shown, 0), ListState::NoMatch);
    }
}
