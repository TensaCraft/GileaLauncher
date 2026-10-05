//! The first-run wizard: a few steps, one question each, with pictures of what a choice looks like
//! (loaded from the repository, `AppInfo.media_url`, never packed into the launcher). A choice
//! applies at once and goes with `setup_apply` too, so a new data folder gets it as well.

mod steps;
mod storage;

use std::time::Duration;

use launcher_shared::{AppError, ErrorCode, Level, SettingUpdate, SetupPlan, SetupPreview};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Icon, Variant, ipc, use_toasts};

use crate::shell::PageHeader;
use crate::store::{SettingsWriter, use_settings_writer, use_store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Storage,
    Home,
    Cards,
    Sidebar,
    Game,
    Sound,
    Done,
}

impl Step {
    pub const ALL: [Step; 8] = [
        Step::Welcome,
        Step::Storage,
        Step::Home,
        Step::Cards,
        Step::Sidebar,
        Step::Game,
        Step::Sound,
        Step::Done,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<Step> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<Step> {
        self.index().checked_sub(1).and_then(|i| Self::ALL.get(i).copied())
    }

    pub fn icon(self) -> &'static str {
        match self {
            Step::Welcome => "waving_hand",
            Step::Storage => "folder",
            Step::Home => "home",
            Step::Cards => "play_circle",
            Step::Sidebar => "view_sidebar",
            Step::Game => "sports_esports",
            Step::Sound => "volume_up",
            Step::Done => "check_circle",
        }
    }

    /// Its name in the list of steps and its heading.
    pub fn key(self) -> &'static str {
        match self {
            Step::Welcome => "setup_step_welcome",
            Step::Storage => "setup_step_storage",
            Step::Home => "setup_step_home",
            Step::Cards => "setup_step_cards",
            Step::Sidebar => "setup_step_sidebar",
            Step::Game => "setup_step_game",
            Step::Sound => "setup_step_sound",
            Step::Done => "setup_step_done",
        }
    }

    /// The line under its heading.
    pub fn desc_key(self) -> &'static str {
        match self {
            Step::Welcome => "setup_step_welcome_desc",
            Step::Storage => "setup_step_storage_desc",
            Step::Home => "setup_step_home_desc",
            Step::Cards => "setup_step_cards_desc",
            Step::Sidebar => "setup_step_sidebar_desc",
            Step::Game => "setup_step_game_desc",
            Step::Sound => "setup_step_sound_desc",
            Step::Done => "setup_step_done_desc",
        }
    }
}

/// A picture of a choice, `<media>/<lang>/<name>.jpg` (English for a language without its own);
/// none without the folder: the cards show their icons.
pub fn picture(media: Option<&str>, lang: &str, name: &str) -> Option<String> {
    let base = media?.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let lang = if lang == "uk_UA" { "uk_UA" } else { "en_US" };
    Some(format!("{base}/{lang}/{name}.jpg"))
}

/// The wizard's choices: the latest one of each kind.
pub fn remember(chosen: &mut Vec<SettingUpdate>, update: SettingUpdate) {
    chosen.retain(|u| std::mem::discriminant(u) != std::mem::discriminant(&update));
    chosen.push(update);
}

/// Where the wizard is and what it was told; every step reads and changes it.
#[derive(Clone, Copy)]
pub struct Wizard {
    pub step: RwSignal<Step>,
    /// The furthest step seen: the list of steps goes back to any step up to it.
    reached: RwSignal<usize>,
    /// The last move went on (the next step slides in from the right).
    forward: RwSignal<bool>,
    /// The launcher-data folder typed or picked, and what it gives (from the backend): the folder
    /// last checked, its folders, or why it was not checked (a relative path…).
    pub dir: RwSignal<String>,
    pub checked: RwSignal<Option<String>>,
    pub preview: RwSignal<Option<SetupPreview>>,
    pub preview_error: RwSignal<Option<AppError>>,
    chosen: StoredValue<Vec<SettingUpdate>>,
    pub saving: RwSignal<bool>,
    pub error: RwSignal<Option<AppError>>,
    writer: SettingsWriter,
}

impl Wizard {
    /// Applies a choice now and keeps it for the folder the wizard ends with.
    pub fn choose(&self, update: SettingUpdate) {
        self.chosen.update_value(|chosen| remember(chosen, update.clone()));
        self.writer.apply(update);
    }

    pub fn go(&self, to: Step) {
        self.forward.set(to.index() >= self.step.get_untracked().index());
        self.reached.update(|r| *r = (*r).max(to.index()));
        self.step.set(to);
    }

    /// The folder typed is the one checked, and nothing is in its way.
    pub fn folder_ready(&self) -> bool {
        let typed = self.dir.get();
        !typed.trim().is_empty()
            && self.checked.with(|c| c.as_deref() == Some(typed.as_str()))
            && self.preview_error.with(Option::is_none)
            && self.preview.with(|p| p.as_ref().is_some_and(|p| p.issue.is_none()))
    }
}

pub fn use_wizard() -> Wizard {
    expect_context::<Wizard>()
}

#[component]
pub fn SetupPage() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let toasts = use_toasts();
    let wizard = Wizard {
        step: RwSignal::new(Step::Welcome),
        reached: RwSignal::new(0),
        forward: RwSignal::new(true),
        dir: RwSignal::new(String::new()),
        checked: RwSignal::new(None),
        preview: RwSignal::new(None),
        preview_error: RwSignal::new(None),
        chosen: StoredValue::new(Vec::new()),
        saving: RwSignal::new(false),
        error: RwSignal::new(None),
        writer: use_settings_writer(),
    };
    provide_context(wizard);
    storage::watch_folder(wizard);
    // A storage problem found at start: the data folder comes first.
    let issues = move || store.setup.with(|s| s.as_ref().is_some_and(|s| !s.issues.is_empty()));
    // Opened again from the settings: it can be left without finishing.
    let again = move || store.setup.with(|s| s.as_ref().is_some_and(|s| !s.should_open));

    // The folder it ends with, the chosen settings with it; a new folder needs a restart.
    let finish = move |then: &'static str| {
        if wizard.saving.get_untracked() {
            return;
        }
        wizard.saving.set(true);
        wizard.error.set(None);
        let plan = SetupPlan {
            lang: store.settings.get_untracked().lang,
            app_state_dir: wizard.dir.get_untracked(),
            updates: wizard.chosen.get_value(),
        };
        #[derive(serde::Serialize)]
        struct PlanArgs {
            plan: SetupPlan,
        }
        spawn_local(async move {
            match ipc::invoke::<_, bool>("setup_apply", &PlanArgs { plan }).await {
                // Saving stays on until the launcher restarts: one more click would save again.
                Ok(true) => {
                    toasts.show(
                        Level::Success,
                        i18n.t("setup_wizard_saved"),
                        Some(i18n.t("storage_dir_saved_restart")),
                    );
                    set_timeout(
                        || {
                            spawn_local(async {
                                let _ = ipc::call::<()>("app_restart").await;
                            })
                        },
                        Duration::from_millis(1200),
                    );
                    return;
                }
                Ok(false) => {
                    store.setup.update(|s| {
                        if let Some(s) = s {
                            s.should_open = false;
                            s.issues.clear();
                        }
                    });
                    store.go(then);
                }
                // About the folder: shown at the folder's step. Anything else: a toast.
                Err(e)
                    if matches!(
                        e.code,
                        ErrorCode::InvalidDirectoryPath | ErrorCode::DirectoryCreateFailed
                    ) =>
                {
                    let _ = wizard.error.try_set(Some(e));
                    if wizard.step.try_get_untracked().is_some_and(|s| s != Step::Storage) {
                        wizard.go(Step::Storage);
                    }
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
            let _ = wizard.saving.try_set(false);
        });
    };
    let skip = move |_: ()| if issues() { wizard.go(Step::Storage) } else { finish("/") };

    let rail = move || {
        let now = wizard.step.get();
        let reached = wizard.reached.get();
        Step::ALL
            .iter()
            .map(|&step| {
                let i = step.index();
                let done = i < now.index();
                // Steps not reached yet open only through «Далі».
                let ahead = i > reached;
                view! {
                    <button
                        type="button"
                        class="wizard__item"
                        class:is-on=step == now
                        class:is-done=done
                        aria-current=(step == now).then_some("step")
                        disabled=ahead
                        on:click=move |_| wizard.go(step)
                    >
                        <span class="wizard__mark">
                            {if done {
                                view! { <Icon name="check" /> }.into_any()
                            } else {
                                view! { <Icon name=step.icon() outlined=true /> }.into_any()
                            }}
                        </span>
                        <span class="wizard__label">{i18n.t(step.key())}</span>
                    </button>
                }
            })
            .collect_view()
    };
    let total = Step::ALL.len();
    let progress = move || format!("width: {}%", (wizard.step.get().index() * 100) / (total - 1));
    let next_blocked = move || wizard.step.get() == Step::Storage && !wizard.folder_ready();

    view! {
        <PageHeader title_key="setup_wizard_title" />
        <div class="wizard">
            <aside class="wizard__rail">
                <div class="wizard__rail-head">
                    <img src="/img/app-icon.png" alt="" />
                    <span>{move || i18n.t("app_title")}</span>
                </div>
                <nav class="wizard__steps">{rail}</nav>
                <Show when=again>
                    <Button variant=Variant::Ghost icon="close" on_click=move |_| store.go("/settings")>
                        {move || i18n.t("close")}
                    </Button>
                </Show>
            </aside>
            <section class="wizard__panel">
                <div class="wizard__progress"><span style=progress></span></div>
                <header class="wizard__head">
                    <span class="wizard__count">
                        {move || i18n.tp(
                            "setup_step_of",
                            &[("n", (wizard.step.get().index() + 1).to_string()), ("total", total.to_string())],
                        )}
                    </span>
                    <h2>{move || i18n.t(wizard.step.get().key())}</h2>
                    <p>{move || i18n.t(wizard.step.get().desc_key())}</p>
                </header>
                <div class="wizard__body" class:is-back=move || !wizard.forward.get()>
                    {move || steps::view(wizard.step.get())}
                </div>
                <footer class="wizard__foot">
                    {move || match wizard.step.get().prev() {
                        Some(prev) => view! {
                            <Button variant=Variant::Ghost icon="arrow_back" on_click=move |_| wizard.go(prev)>
                                {move || i18n.t("setup_back")}
                            </Button>
                        }.into_any(),
                        // Opened again, it is left with «Закрити» instead.
                        None if again() => ().into_any(),
                        None => view! {
                            <Button variant=Variant::Ghost icon="fast_forward" on_click=move |_| skip(())>
                                {move || i18n.t("setup_skip")}
                            </Button>
                        }.into_any(),
                    }}
                    <span class="wizard__gap"></span>
                    {move || match wizard.step.get().next() {
                        Some(next) => view! {
                            <Button
                                variant=Variant::Primary
                                icon="arrow_forward"
                                disabled=Signal::derive(next_blocked)
                                on_click=move |_| wizard.go(next)
                            >
                                {move || i18n.t("setup_next")}
                            </Button>
                        }.into_any(),
                        None => view! {
                            <Button variant=Variant::Ghost icon="person_add" on_click=move |_| finish("/profiles")>
                                {move || i18n.t("setup_add_account")}
                            </Button>
                            <Button
                                variant=Variant::Primary
                                icon="rocket_launch"
                                loading=Signal::derive(move || wizard.saving.get())
                                on_click=move |_| finish("/")
                            >
                                {move || i18n.t("setup_start")}
                            </Button>
                        }.into_any(),
                    }}
                </footer>
            </section>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{CardPlay, SettingUpdate};

    use super::*;

    #[test]
    fn the_steps_run_from_the_welcome_to_the_end() {
        assert_eq!(Step::ALL.first(), Some(&Step::Welcome));
        assert_eq!(Step::ALL.last(), Some(&Step::Done));
        assert_eq!(Step::Storage.index(), 1);
        assert_eq!(Step::Welcome.next(), Some(Step::Storage));
        assert_eq!(Step::Done.next(), None);
        assert_eq!(Step::Welcome.prev(), None);
        assert_eq!(Step::Done.prev(), Some(Step::Sound));
        let keys: Vec<_> = Step::ALL.iter().map(|s| s.key()).collect();
        assert!(keys.iter().all(|k| k.starts_with("setup_step_")), "{keys:?}");
    }

    #[test]
    fn a_picture_is_taken_from_the_media_folder_in_the_shown_language() {
        let base = Some("https://example.net/setup/");
        assert_eq!(
            picture(base, "uk_UA", "cards-bar").as_deref(),
            Some("https://example.net/setup/uk_UA/cards-bar.jpg")
        );
        assert_eq!(
            picture(base, "pl_PL", "cards-bar").as_deref(),
            Some("https://example.net/setup/en_US/cards-bar.jpg"),
            "a language without pictures of its own shows the English ones"
        );
        assert_eq!(picture(None, "uk_UA", "cards-bar"), None, "no folder: the icons");
        assert_eq!(picture(Some("  "), "uk_UA", "cards-bar"), None);
    }

    #[test]
    fn the_wizard_keeps_the_latest_choice_of_each_kind() {
        let mut chosen = Vec::new();
        remember(&mut chosen, SettingUpdate::CardPlay(CardPlay::Bar));
        remember(&mut chosen, SettingUpdate::CompactSidebar(false));
        remember(&mut chosen, SettingUpdate::CardPlay(CardPlay::Corner));
        assert_eq!(
            chosen,
            vec![SettingUpdate::CompactSidebar(false), SettingUpdate::CardPlay(CardPlay::Corner)]
        );
    }
}
