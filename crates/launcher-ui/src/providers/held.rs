//! Files a provider may not hand to other apps (`ErrorCode::ProviderFilesHeld`, a plan's
//! `file_blocked`): the user downloads them from their pages, and the launcher watches their
//! Downloads folder (`held_files_found`) to go on by itself once they are there.

use std::time::Duration;

use launcher_shared::provider::HeldFile;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, Button, Dialog, DialogFooter, IconAction, Variant};

use super::api;

/// How often the Downloads folder is looked at.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// Which of `files` are in the user's Downloads folder, kept in `found` while there are any (one
/// look at a time).
pub fn watch_downloads(files: Memo<Vec<HeldFile>>, found: RwSignal<Vec<bool>>) {
    let timer = StoredValue::new(None::<IntervalHandle>);
    let looking = StoredValue::new(false);
    let stop = move || {
        if let Some(handle) = timer.try_update_value(Option::take).flatten() {
            handle.clear();
        }
    };
    Effect::new(move |_| {
        let wanted = files.get();
        stop();
        found.set(vec![false; wanted.len()]);
        if wanted.is_empty() {
            return;
        }
        let look = move || {
            if looking.try_get_value().unwrap_or(true) {
                return;
            }
            looking.set_value(true);
            let wanted = files.get_untracked();
            spawn_local(async move {
                if let Ok(now) = api::held_found(&wanted).await
                    && files.try_with_untracked(|f| *f == wanted).unwrap_or(false)
                    && found.try_with_untracked(|f| *f != now).unwrap_or(false)
                {
                    let _ = found.try_set(now);
                }
                looking.try_set_value(false);
            });
        };
        look();
        timer.set_value(set_interval_with_handle(look, WATCH_EVERY).ok());
    });
    on_cleanup(stop);
}

/// The files to download by hand before an install goes on: each with its page and whether it is
/// in the Downloads folder yet. Once all are there the dialog closes and `on_continue` runs;
/// "Continue" runs it at once. An empty `held` closes it.
#[component]
pub fn HeldDialog(
    /// The provider that holds them back.
    provider: Signal<String>,
    held: RwSignal<Vec<HeldFile>>,
    on_continue: Callback<()>,
    on_open: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let files = Memo::new(move |_| held.get());
    let found = RwSignal::new(Vec::<bool>::new());
    watch_downloads(files, found);
    Effect::new(move |_| open.set(!files.with(Vec::is_empty)));
    // Closed by hand: nothing waits any more.
    Effect::new(move |previous: Option<bool>| {
        let now = open.get();
        if previous == Some(true) && !now {
            held.set(Vec::new());
        }
        now
    });
    let go_on = move || {
        held.set(Vec::new());
        on_continue.run(());
    };
    Effect::new(move |_| {
        let all = found.with(|f| !f.is_empty() && f.iter().all(|there| *there));
        if all && files.with_untracked(|f| f.len()) == found.with_untracked(Vec::len) {
            go_on();
        }
    });
    let rows = move || {
        files.with(|list| {
            list.iter()
                .enumerate()
                .map(|(i, file)| {
                    let there = Signal::derive(move || found.with(|f| f.get(i).copied().unwrap_or(false)));
                    let url = file.url.clone();
                    let line = format!("{} — {}", file.title, file.file_name);
                    view! {
                        <div class="provider-deps__row">
                            <span class="provider-deps__text">{line}</span>
                            <span class="provider-held__state" class:is-found=there>
                                {move || if there.get() { i18n.t("provider_held_found") } else { i18n.t("provider_held_waiting") }}
                            </span>
                            {url.map(|url| view! {
                                <IconAction
                                    icon="open_in_new"
                                    tone=ActionTone::Info
                                    title=Signal::derive(move || i18n.t("modrinth_dependency_open"))
                                    on_click=Callback::new(move |()| on_open.run(url.clone()))
                                />
                            })}
                        </div>
                    }
                })
                .collect_view()
        })
    };
    view! {
        <Dialog open=open title=Signal::derive(move || i18n.t("provider_held_title")) icon="download" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="download" on_click=move |_| go_on()>
                    {move || i18n.t("provider_held_continue")}
                </Button>
            </DialogFooter>
            <p class="provider-deps__lead">{move || i18n.tp("provider_held_lead", &[("provider", provider.get())])}</p>
            <section class="provider-deps__section is-danger">{rows}</section>
        </Dialog>
    }
}
