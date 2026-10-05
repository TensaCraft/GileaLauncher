//! Files a provider may not hand to other apps (`ErrorCode::ProviderFilesHeld`, a plan's
//! `file_blocked`): the user downloads them from their pages, and the launcher watches their
//! Downloads folder (`held_files_found`) to go on by itself once they are there. The same mod on
//! another provider can be taken instead, or the files left out and added by hand later.

use std::time::Duration;

use launcher_shared::provider::{HeldFile, ProviderInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, Button, Dialog, DialogFooter, Icon, IconAction, Variant, use_toasts};

use super::{api, open_page};
use crate::builds::actions::use_build_actions;

/// How often the Downloads folder is looked at.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// How the install goes on: some held files taken from their alternative (by SHA-1), the rest
/// left out when `skip`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldGoOn {
    pub replace: Vec<String>,
    pub skip: bool,
}

/// Every file is either in the Downloads folder or taken from elsewhere.
pub fn all_settled(files: &[HeldFile], found: &[bool], taken: &[String]) -> bool {
    !files.is_empty()
        && files.len() == found.len()
        && files.iter().zip(found).all(|(file, there)| *there || taken.contains(&file.sha1))
}

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

/// The files to download by hand before an install goes on: each with its page, whether it is in
/// the Downloads folder yet and the same mod elsewhere when there is one. Once all are there (or
/// taken from elsewhere) the dialog closes and `on_continue` runs; "Continue" runs it at once,
/// "Skip" without the missing ones. An empty `held` closes it.
#[component]
pub fn HeldDialog(
    /// The provider that holds them back.
    provider: Signal<String>,
    held: RwSignal<Vec<HeldFile>>,
    on_continue: Callback<HeldGoOn>,
    on_open: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let files = Memo::new(move |_| held.get());
    let found = RwSignal::new(Vec::<bool>::new());
    // The files taken from another provider instead (by SHA-1).
    let taken = RwSignal::new(Vec::<String>::new());
    watch_downloads(files, found);
    Effect::new(move |_| {
        files.track();
        taken.set(Vec::new());
    });
    // Where the launcher looks: the user saves the files there.
    let folder = RwSignal::new(None::<String>);
    spawn_local(async move {
        if let Ok(path) = api::downloads_folder().await {
            let _ = folder.try_set(path);
        }
    });
    Effect::new(move |_| open.set(!files.with(Vec::is_empty)));
    // Closed by hand: nothing waits any more.
    Effect::new(move |previous: Option<bool>| {
        let now = open.get();
        if previous == Some(true) && !now {
            held.set(Vec::new());
        }
        now
    });
    let go_on = move |skip: bool| {
        let replace = taken.get_untracked();
        held.set(Vec::new());
        on_continue.run(HeldGoOn { replace, skip });
    };
    Effect::new(move |_| {
        let settled = files.with(|f| found.with(|there| taken.with(|t| all_settled(f, there, t))));
        if settled {
            go_on(false);
        }
    });
    let rows = move || {
        files.with(|list| {
            list.iter()
                .enumerate()
                .map(|(i, file)| {
                    let there = Signal::derive(move || found.with(|f| f.get(i).copied().unwrap_or(false)));
                    let sha1 = file.sha1.clone();
                    let chosen = Signal::derive({
                        let sha1 = sha1.clone();
                        move || taken.with(|t| t.contains(&sha1))
                    });
                    let url = file.url.clone();
                    let line = format!("{} — {}", file.title, file.file_name);
                    let alternative = file.alternative.clone();
                    let elsewhere = alternative.as_ref().map(|a| {
                        i18n.tp(
                            "provider_held_alternative",
                            &[("provider", a.provider.clone()), ("title", a.title.clone()), ("version", a.version.clone())],
                        )
                    });
                    let state = move || {
                        if there.get() {
                            i18n.t("provider_held_found")
                        } else if chosen.get() {
                            alternative
                                .as_ref()
                                .map(|a| i18n.tp("provider_held_taken", &[("provider", a.provider.clone())]))
                                .unwrap_or_default()
                        } else {
                            i18n.t("provider_held_waiting")
                        }
                    };
                    let take = file.alternative.clone().map(|a| {
                        let label = i18n.tp("provider_held_take", &[("provider", a.provider.clone())]);
                        let sha1 = sha1.clone();
                        view! {
                            <Button
                                icon="download"
                                title=Signal::derive(move || i18n.t("provider_held_take_tip"))
                                disabled=Signal::derive(move || there.get() || chosen.get())
                                on_click=move |_| taken.update(|t| t.push(sha1.clone()))
                            >
                                {label}
                            </Button>
                        }
                    });
                    view! {
                        <div class="provider-held__row">
                            <div class="provider-held__text">
                                <span class="provider-held__name">{line}</span>
                                {elsewhere.map(|text| view! { <span class="provider-held__elsewhere">{text}</span> })}
                            </div>
                            <span class="provider-held__state" class:is-found=move || there.get() || chosen.get()>{state}</span>
                            {take}
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
                <Button
                    variant=Variant::Ghost
                    icon="skip_next"
                    title=Signal::derive(move || i18n.t("provider_held_skip_tip"))
                    on_click=move |_| go_on(true)
                >
                    {move || i18n.t("provider_held_skip")}
                </Button>
                <Button variant=Variant::Primary icon="download" on_click=move |_| go_on(false)>
                    {move || i18n.t("provider_held_continue")}
                </Button>
            </DialogFooter>
            <p class="provider-deps__lead">{move || i18n.tp("provider_held_lead", &[("provider", provider.get())])}</p>
            <div class="provider-held__folder">
                <Icon name="folder" outlined=true />
                <span>{move || i18n.t("provider_held_folder")}</span>
                <code>{move || folder.get().unwrap_or_else(|| i18n.t("provider_held_no_folder"))}</code>
            </div>
            <section class="provider-deps__section is-danger">{rows}</section>
        </Dialog>
    }
}

/// A build installed without some held files (`skip_held`): the files to add by hand.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub provider: ProviderInfo,
    pub key: String,
    pub build: String,
    pub files: Vec<HeldFile>,
}

/// The app's one note of files left out, shown over any page (the one that installed is gone).
#[derive(Clone, Copy)]
pub struct SkippedHeld(pub RwSignal<Option<Skipped>>);

pub fn provide_skipped_held() {
    provide_context(SkippedHeld(RwSignal::new(None)));
}

pub fn use_skipped_held() -> SkippedHeld {
    expect_context::<SkippedHeld>()
}

impl SkippedHeld {
    /// Says which files `build` was installed without, when there are any.
    pub fn note(&self, provider: ProviderInfo, key: String, build: String, files: Vec<HeldFile>) {
        if !files.is_empty() {
            let _ = self.0.try_set(Some(Skipped { provider, key, build, files }));
        }
    }
}

/// The files a build was installed without: each with its page and the build's folder it goes in.
#[component]
pub fn SkippedHeldDialog() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let actions = use_build_actions();
    let skipped = use_skipped_held().0;
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(skipped.with(Option::is_some)));
    Effect::new(move |previous: Option<bool>| {
        let now = open.get();
        if previous == Some(true) && !now {
            skipped.set(None);
        }
        now
    });
    let lead = move || {
        skipped.with(|s| {
            s.as_ref()
                .map(|s| {
                    i18n.tp(
                        "provider_skipped_lead",
                        &[("build", s.build.clone()), ("provider", s.provider.name.clone())],
                    )
                })
                .unwrap_or_default()
        })
    };
    let rows = move || {
        skipped.get().map(|s| {
            let provider = s.provider.clone();
            s.files
                .into_iter()
                .map(|file| {
                    let line = format!("{} — {}", file.title, file.file_name);
                    let folder = (!file.folder.is_empty())
                        .then(|| i18n.tp("provider_skipped_folder", &[("folder", file.folder.clone())]));
                    let provider = provider.clone();
                    view! {
                        <div class="provider-held__row">
                            <div class="provider-held__text">
                                <span class="provider-held__name">{line}</span>
                                {folder.map(|f| view! { <span class="provider-held__elsewhere">{f}</span> })}
                            </div>
                            {file.url.map(|url| view! {
                                <IconAction
                                    icon="open_in_new"
                                    tone=ActionTone::Info
                                    title=Signal::derive(move || i18n.t("modrinth_dependency_open"))
                                    on_click=Callback::new(move |()| open_page(i18n, toasts, &provider, url.clone()))
                                />
                            })}
                        </div>
                    }
                })
                .collect_view()
        })
    };
    let open_folder = move || {
        if let Some(key) = skipped.with_untracked(|s| s.as_ref().map(|s| s.key.clone())) {
            actions.open_dir(key);
        }
    };
    view! {
        <Dialog open=open title=Signal::derive(move || i18n.t("provider_skipped_title")) icon="warning_amber" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost icon="folder_open" on_click=move |_| open_folder()>
                    {move || i18n.t("provider_skipped_open")}
                </Button>
                <Button variant=Variant::Primary on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <p class="provider-deps__lead">{lead}</p>
            <section class="provider-deps__section is-danger">{rows}</section>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(sha1: &str) -> HeldFile {
        HeldFile {
            title: "Held".into(),
            file_name: "held.jar".into(),
            url: None,
            size: 1,
            sha1: sha1.into(),
            alternative: None,
            folder: "mods".into(),
        }
    }

    #[test]
    fn the_install_goes_on_once_every_file_is_there_or_taken_from_elsewhere() {
        let files = [file("a"), file("b")];
        assert!(!all_settled(&files, &[true, false], &[]));
        assert!(all_settled(&files, &[true, false], &["b".into()]), "b comes from elsewhere");
        assert!(all_settled(&files, &[true, true], &[]));
        assert!(!all_settled(&files, &[], &["a".into(), "b".into()]), "not looked at yet");
        assert!(!all_settled(&[], &[], &[]), "nothing waits");
    }
}
