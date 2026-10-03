//! A provider's parts of the Installed list: where it checks updates, the status line (the list's
//! toolbar checks again); on each row of one of its projects "Open on the site" and, with a newer version,
//! "Update" — after a confirmation, through the usual install flow (the current file is backed up
//! first).

use launcher_shared::provider::{FileNote, Need, OverviewArgs, ProviderInfo, update_texts};
use launcher_shared::{ContentItem, ContentKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, ConfirmDialog, Icon, IconAction, use_toasts};

use super::flow::{InstallFlow, installs};
use super::store::{UpdateAsk, store};
use super::{api, open_page};

/// A row's name, as the Installed list shows it.
pub(crate) fn display_name(item: &ContentItem) -> String {
    item.name.clone().unwrap_or_else(|| item.filename.clone())
}

/// A row's "open the page" action: the provider's mark, so two providers' pages tell apart.
fn page_icon(provider: &ProviderInfo) -> String {
    if provider.icon.starts_with("brand:") { provider.icon.clone() } else { "open_in_new".to_string() }
}

/// The quick look before a check for updates: names and icons show without waiting for it.
fn first_look(args: &OverviewArgs) -> Option<OverviewArgs> {
    args.check_updates.then(|| OverviewArgs { check_updates: false, ..args.clone() })
}

/// Asks for the tab's overview (and, where the provider checks them, its updates) and keeps it in
/// the store; a failed ask leaves an empty overview (the rows lose the provider's actions).
/// One look at a time per tab: asked again while one runs, one more follows it (the build's files
/// are read and hashed once, not by two looks side by side).
pub(crate) fn load(provider: &ProviderInfo, key: String, kind: ContentKind) {
    let (s, id) = (store(), provider.id.clone());
    // Untracked: the effect that calls this must not run again when the look ends.
    if untrack(|| s.checking(&id, &key, kind)) {
        s.look_again(&id, &key, kind);
        return;
    }
    s.set_checking(&id, &key, kind, true);
    let args = OverviewArgs { key: key.clone(), kind, check_updates: provider.offers(Need::Updates(kind)) };
    spawn_local(async move {
        let mut quick = match first_look(&args) {
            Some(look) => api::overview(&id, &look).await.ok(),
            None => None,
        };
        loop {
            if let Some(quick) = &quick {
                s.set_overview(&id, &key, kind, quick.clone());
            }
            // The full look replaces the quick one; when it fails, the quick one stays.
            let overview = match api::overview(&id, &args).await {
                Ok(full) => full,
                Err(_) => quick.take().unwrap_or_else(|| s.overview(&id, &key, kind).unwrap_or_default()),
            };
            s.set_overview(&id, &key, kind, overview);
            // Asked again meanwhile (a change, a refresh): the names are there; one full look more.
            if !s.take_again(&id, &key, kind) {
                break;
            }
            quick = None;
        }
        s.set_checking(&id, &key, kind, false);
    });
}

/// The icon a row shows: the first https picture a provider knows for its file.
pub fn row_icon(notes: &[Option<FileNote>]) -> Option<String> {
    notes.iter().flatten().filter_map(|n| n.icon_url.clone()).find(|url| url.starts_with("https://"))
}

/// A row's picture: its project's icon when a provider knows it, else whether it is on.
#[component]
pub fn ContentIcon(
    providers: Vec<ProviderInfo>,
    key: String,
    kind: ContentKind,
    file: String,
    enabled: bool,
) -> impl IntoView {
    let icon = Memo::new(move |_| {
        let order: Vec<String> = providers.iter().map(|p| p.id.clone()).collect();
        // Once the row's provider is settled: its picture first, so the row's never changes.
        let owner = store().owner(&order, &key, kind, &file)?;
        let mut notes: Vec<(bool, Option<FileNote>)> =
            providers.iter().map(|p| (p.id != owner, store().note(&p.id, &key, kind, &file))).collect();
        notes.sort_by_key(|(other, _)| *other);
        row_icon(&notes.into_iter().map(|(_, n)| n).collect::<Vec<_>>())
    });
    move || {
        match icon.get() {
        Some(src) => {
            view! { <div class="build-row__icon"><img src=src alt="" loading="lazy" draggable="false" /></div> }
                .into_any()
        }
        None => view! {
            <div class="build-row__icon content-row__state">
                <Icon name=if enabled { "check_circle" } else { "cancel" } />
            </div>
        }
        .into_any(),
    }
    }
}

/// Whether the tab is looked at on `revision`: each arrival of the list (from 1); not the mount
/// before it (0), or every opening would look twice.
fn looks_at(revision: u64) -> bool {
    revision > 0
}

#[component]
pub fn ProviderTools(
    provider: ProviderInfo,
    key: String,
    kind: ContentKind,
    /// Grows each time the list arrives again (after a change, a refresh, the game closing).
    revision: Signal<u64>,
    /// Loads the list again (after the provider changed files).
    reload: Callback<()>,
) -> impl IntoView {
    let key = StoredValue::new(key);
    let info = StoredValue::new(provider.clone());
    let id = StoredValue::new(provider.id.clone());
    // Whenever the list arrives (the page asks for it on opening).
    Effect::new(move |_| {
        if looks_at(revision.get()) {
            info.with_value(|p| load(p, key.get_value(), kind));
        }
    });
    let flow = InstallFlow::new(provider.clone(), Callback::new(move |_| reload.run(())));
    let confirm = UpdateConfirm::new(flow);
    // A row of this tab asked this provider for an update.
    Effect::new(move |_| {
        if let Some(ask) = store().take_ask(&id.get_value(), &key.get_value(), kind) {
            confirm.ask(ask);
        }
    });
    // What it found shows on the toolbar's "Check for updates" (a badge and its tooltip).
    view! {
        {flow.dialog()}
        {confirm.view()}
    }
}

/// The badge on "Check for updates": how many updates wait (none at zero, "99+" past 99).
pub fn updates_badge(available: usize) -> Option<String> {
    match available {
        0 => None,
        1..=99 => Some(available.to_string()),
        _ => Some("99+".to_string()),
    }
}

#[component]
pub fn ProviderRowActions(
    provider: ProviderInfo,
    key: String,
    kind: ContentKind,
    item: ContentItem,
) -> impl IntoView {
    let (i18n, toasts) = (use_i18n(), use_toasts());
    let (file, name, enabled) = (item.file.clone(), display_name(&item), item.enabled);
    let info = StoredValue::new(provider.clone());
    let id = provider.id.clone();
    let note = {
        let (id, key, file) = (id.clone(), key.clone(), file.clone());
        Memo::new(move |_| store().note(&id, &key, kind, &file))
    };
    // Only what this row has: the list puts occasional actions before the permanent ones, so a
    // missing one leaves no hole between buttons.
    move || {
        let n = note.get()?;
        Some({
            // A switched-off mod is switched on first; a pack updates whether it is on or not.
            let update = match n.update.clone().filter(|_| enabled || kind != ContentKind::Mods) {
                Some(_) => {
                    let (id, key, n, name) = (id.clone(), key.clone(), n.clone(), name.clone());
                    let busy = {
                        let (id, key, project) = (id.clone(), key.clone(), n.project_id.clone());
                        Signal::derive(move || installs().running(&id, &key, &project))
                    };
                    view! {
                        <IconAction
                            icon="upgrade"
                            tone=ActionTone::Ok
                            title=Signal::derive(move || i18n.t(update_texts(kind).action))
                            loading=busy
                            on_click=Callback::new(move |()| {
                                store().ask(UpdateAsk {
                                    provider: id.clone(),
                                    key: key.clone(),
                                    kind,
                                    note: n.clone(),
                                    name: name.clone(),
                                })
                            })
                        />
                    }
                    .into_any()
                }
                None => ().into_any(),
            };
            let page = match n.url.clone() {
                Some(url) => view! {
                    <IconAction
                        icon=info.with_value(page_icon)
                        tone=ActionTone::Info
                        title=Signal::derive(move || {
                            i18n.tp("open_on_provider", &[("provider", info.with_value(|p| p.name.clone()))])
                        })
                        on_click=Callback::new(move |()| info.with_value(|p| open_page(i18n, toasts, p, url.clone())))
                    />
                }
                .into_any(),
                None => ().into_any(),
            };
            view! { {update}{page} }.into_any()
        })
    }
}

/// "Update <mod>? Current version … New version …" before the update's install flow.
#[derive(Clone, Copy)]
pub(crate) struct UpdateConfirm {
    open: RwSignal<bool>,
    asked: RwSignal<Option<UpdateAsk>>,
    flow: InstallFlow,
}

impl UpdateConfirm {
    pub fn new(flow: InstallFlow) -> UpdateConfirm {
        UpdateConfirm { open: RwSignal::new(false), asked: RwSignal::new(None), flow }
    }

    pub fn ask(self, ask: UpdateAsk) {
        self.asked.set(Some(ask));
        self.open.set(true);
    }

    pub fn view(self) -> impl IntoView {
        let i18n = use_i18n();
        let message = Signal::derive(move || {
            self.asked
                .with(|a| {
                    a.as_ref().map(|a| {
                        let new =
                            a.note.update.as_ref().map(|u| u.version_number.clone()).unwrap_or_default();
                        i18n.tp(
                            update_texts(a.kind).confirm,
                            &[
                                ("name", a.name.clone()),
                                ("current", a.note.version_number.clone()),
                                ("new", new),
                            ],
                        )
                    })
                })
                .unwrap_or_default()
        });
        let confirm = Callback::new(move |()| {
            self.open.set(false);
            if let Some(a) = self.asked.get_untracked()
                && let Some(args) = a.note.update_args(&a.key, a.kind, &a.name)
            {
                self.flow.start(args);
            }
        });
        view! {
            <ConfirmDialog
                open=self.open
                title=Signal::derive(move || {
                    self.asked.with(|a| a.as_ref().map(|a| i18n.t(update_texts(a.kind).action))).unwrap_or_default()
                })
                message=message
                confirm_label=Signal::derive(move || i18n.t("update_modrinth_content"))
                cancel_label=Signal::derive(move || i18n.t("cancel"))
                on_confirm=confirm
            />
        }
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::provider::FileNote;

    #[test]
    fn a_tab_is_looked_at_once_its_list_arrives_not_before() {
        // Opening the tab: the list's first arrival starts the look; the mount before it did too,
        // so every opening checked updates twice (files hashed, the provider asked).
        assert!(!super::looks_at(0));
        assert!(super::looks_at(1) && super::looks_at(7));
    }

    #[test]
    fn a_page_is_opened_with_its_provider_s_mark() {
        let mut provider = launcher_shared::provider::ProviderInfo {
            id: "modrinth".into(),
            name: "Modrinth".into(),
            icon: "brand:modrinth".into(),
            content: Vec::new(),
            updates: Vec::new(),
            modpacks: false,
            modpack_updates: false,
        };
        assert_eq!(super::page_icon(&provider), "brand:modrinth");
        provider.icon = "search".into();
        assert_eq!(super::page_icon(&provider), "open_in_new", "a plain icon says nothing of a page");
    }

    #[test]
    fn a_check_for_updates_looks_quickly_first() {
        use launcher_shared::ContentKind;
        use launcher_shared::provider::OverviewArgs;
        let checking = OverviewArgs { key: "aero".into(), kind: ContentKind::Mods, check_updates: true };
        assert_eq!(
            super::first_look(&checking),
            Some(OverviewArgs { check_updates: false, ..checking.clone() })
        );
        let plain = OverviewArgs { check_updates: false, ..checking };
        assert_eq!(super::first_look(&plain), None, "nothing slower follows: one look");
    }

    #[test]
    fn the_updates_badge_counts_what_waits() {
        assert_eq!(super::updates_badge(0), None, "nothing waits: no badge");
        assert_eq!(super::updates_badge(10).as_deref(), Some("10"));
        assert_eq!(super::updates_badge(99).as_deref(), Some("99"));
        assert_eq!(super::updates_badge(250).as_deref(), Some("99+"));
    }

    use super::row_icon;

    fn note(icon: Option<&str>) -> FileNote {
        FileNote {
            file: "sodium.jar".into(),
            project_id: "AANobbMI".into(),
            slug: "sodium".into(),
            title: "Sodium".into(),
            version_number: "0.6".into(),
            update: None,
            url: None,
            icon_url: icon.map(str::to_string),
            installed: false,
        }
    }

    #[test]
    fn a_row_takes_the_first_known_icon() {
        assert_eq!(
            row_icon(&[None, Some(note(None)), Some(note(Some("https://cdn/a.png")))]).as_deref(),
            Some("https://cdn/a.png")
        );
        assert_eq!(row_icon(&[None, Some(note(None))]), None, "no provider knows one: the state icon stays");
        assert_eq!(row_icon(&[Some(note(Some("http://cdn/a.png")))]), None, "only https pictures");
    }
}
