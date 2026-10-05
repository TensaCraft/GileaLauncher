//! The wizard's steps after the folder: one question each, the looks shown with pictures.

use launcher_shared::{CardPlay, ClickSound, GameStartAction, MemoryInfo, SettingUpdate};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{ChoiceCards, ChoiceOption, Icon, Select, SelectOption, SettingRow, Switch, ipc, sound};

use super::storage::StorageStep;
use super::{Step, picture, use_wizard};
use crate::builds::ram::RamSetting;
use crate::pages::settings::java::ram_start;
use crate::store::{AppStore, use_store};

/// The step's own view.
pub fn view(step: Step) -> AnyView {
    match step {
        Step::Welcome => view! { <WelcomeStep /> }.into_any(),
        Step::Storage => view! { <StorageStep /> }.into_any(),
        Step::Home => view! { <HomeStep /> }.into_any(),
        Step::Cards => view! { <CardsStep /> }.into_any(),
        Step::Sidebar => view! { <SidebarStep /> }.into_any(),
        Step::Game => view! { <GameStep /> }.into_any(),
        Step::Sound => view! { <SoundStep /> }.into_any(),
        Step::Done => view! { <DoneStep /> }.into_any(),
    }
}

/// A picture of a choice in the language shown now.
fn shot(store: AppStore, name: &str) -> Option<String> {
    let media = store.info.with(|i| i.as_ref().and_then(|i| i.media_url.clone()));
    store.settings.with(|s| picture(media.as_deref(), &s.lang, name))
}

/// A setting as the choice cards hold it: kept in step with the saved settings.
fn mirror(store: AppStore, read: fn(&launcher_shared::SettingsSnapshot) -> String) -> RwSignal<String> {
    let value = RwSignal::new(store.settings.with_untracked(read));
    Effect::new(move |_| value.set(store.settings.with(read)));
    value
}

fn mirror_flag(store: AppStore, read: fn(&launcher_shared::SettingsSnapshot) -> bool) -> RwSignal<bool> {
    let value = RwSignal::new(store.settings.with_untracked(read));
    Effect::new(move |_| value.set(store.settings.with(read)));
    value
}

#[component]
fn WelcomeStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let lang = mirror(store, |s| s.lang.clone());
    // Built again only when the language changes (a choice saved keeps the pictures shown).
    let options = Memo::new(move |_| {
        vec![
            ChoiceOption::new("uk_UA", i18n.t("setup_lang_uk"), "translate")
                .with_desc(i18n.t("setup_lang_uk_desc")),
            ChoiceOption::new("en_US", i18n.t("setup_lang_en"), "translate")
                .with_desc(i18n.t("setup_lang_en_desc")),
        ]
    });
    view! {
        <div class="wizard-welcome">
            <div class="wizard-welcome__hero">
                <img src="/img/app-icon.png" alt="" />
                <div>
                    <b>{move || i18n.t("setup_welcome_title")}</b>
                    <span>{move || i18n.t("setup_welcome_lead")}</span>
                </div>
            </div>
            <h3 class="wizard__ask">{move || i18n.t("language")}</h3>
            <ChoiceCards
                options=options
                label=Signal::derive(move || i18n.t("language"))
                value=lang
                columns=2
                on_change=Callback::new(move |l: String| wizard.choose(SettingUpdate::Lang(l)))
            />
        </div>
    }
}

#[component]
fn HomeStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let shown = mirror(store, |s| if s.home_recent_builds > 0 { "on" } else { "off" }.to_string());
    // Built again only when the language changes (a choice saved keeps the pictures shown).
    let options = Memo::new(move |_| {
        vec![
            ChoiceOption::new("on", i18n.t("setup_home_recent"), "history")
                .with_desc(i18n.t("setup_home_recent_desc"))
                .with_image(shot(store, "home-recent")),
            ChoiceOption::new("off", i18n.t("setup_home_builds"), "grid_view")
                .with_desc(i18n.t("setup_home_builds_desc"))
                .with_image(shot(store, "home-builds")),
        ]
    });
    let on_change = Callback::new(move |v: String| {
        // Shown again: as many as before, else the usual five.
        let before = store.settings.with_untracked(|s| s.home_recent_builds);
        let count = if v == "on" { if before > 0 { before } else { 5 } } else { 0 };
        wizard.choose(SettingUpdate::HomeRecentBuilds(count));
    });
    view! { <ChoiceCards options=options label=Signal::derive(move || i18n.t("setup_step_home")) value=shown on_change=on_change /> }
}

#[component]
fn CardsStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let play = mirror(store, |s| s.card_play.as_config_str().to_string());
    // Built again only when the language changes (a choice saved keeps the pictures shown).
    let options = Memo::new(move |_| {
        CardPlay::ALL
            .iter()
            .map(|p| {
                let (icon, desc, name) = match p {
                    CardPlay::Center => ("play_circle", "setup_cards_center_desc", "cards-center"),
                    CardPlay::Bar => ("smart_display", "setup_cards_bar_desc", "cards-bar"),
                    CardPlay::Corner => ("picture_in_picture_alt", "setup_cards_corner_desc", "cards-corner"),
                };
                ChoiceOption::new(p.as_config_str(), i18n.t(p.label_key()), icon)
                    .with_desc(i18n.t(desc))
                    .with_image(shot(store, name))
            })
            .collect::<Vec<_>>()
    });
    let on_change =
        Callback::new(move |v: String| wizard.choose(SettingUpdate::CardPlay(CardPlay::from_config_str(&v))));
    view! { <ChoiceCards options=options label=Signal::derive(move || i18n.t("setup_step_cards")) value=play on_change=on_change /> }
}

#[component]
fn SidebarStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let compact = mirror(store, |s| if s.compact_sidebar { "compact" } else { "full" }.to_string());
    // Built again only when the language changes (a choice saved keeps the pictures shown).
    let options = Memo::new(move |_| {
        vec![
            ChoiceOption::new("compact", i18n.t("setup_sidebar_compact"), "view_sidebar")
                .with_desc(i18n.t("setup_sidebar_compact_desc"))
                .with_image(shot(store, "sidebar-compact")),
            ChoiceOption::new("full", i18n.t("setup_sidebar_full"), "menu_open")
                .with_desc(i18n.t("setup_sidebar_full_desc"))
                .with_image(shot(store, "sidebar-full")),
        ]
    });
    let on_change =
        Callback::new(move |v: String| wizard.choose(SettingUpdate::CompactSidebar(v == "compact")));
    view! { <ChoiceCards options=options label=Signal::derive(move || i18n.t("setup_step_sidebar")) value=compact on_change=on_change /> }
}

#[component]
fn GameStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    let memory = RwSignal::new(None::<MemoryInfo>);
    spawn_local(async move {
        if let Ok(info) = ipc::call::<MemoryInfo>("memory_info").await {
            let _ = memory.try_set(Some(info));
        }
    });
    let auto = RwSignal::new(store.settings.get_untracked().default_max_ram_gb.is_none());
    let ram = RwSignal::new(1.0);
    Effect::new(move |_| {
        if let Some(info) = memory.get() {
            ram.set(ram_start(store.settings.get_untracked().default_max_ram_gb, &info));
        }
    });
    let auto_label = move || {
        let value = memory.get().map(|m| m.recommended_heap_gb.to_string()).unwrap_or_default();
        i18n.tp("default_max_ram_auto", &[("value", value)])
    };
    let on_auto = Callback::new(move |on: bool| {
        let value = if on { None } else { Some(ram.get_untracked().round().max(1.0) as u64) };
        wizard.choose(SettingUpdate::DefaultMaxRamGb(value));
    });
    let on_ram = Callback::new(move |gb: f64| {
        wizard.choose(SettingUpdate::DefaultMaxRamGb(Some(gb.round().max(1.0) as u64)))
    });

    let start = mirror(store, |s| s.on_game_start.as_config_str().to_string());
    let starts = Memo::new(move |_| {
        [
            (GameStartAction::Nothing, "desktop_windows", "on_game_start_nothing", "setup_game_nothing_desc"),
            (GameStartAction::Tray, "south_east", "on_game_start_tray", "setup_game_tray_desc"),
            (GameStartAction::Close, "close", "on_game_start_close", "setup_game_close_desc"),
        ]
        .into_iter()
        .map(|(action, icon, label, desc)| {
            ChoiceOption::new(action.as_config_str(), i18n.t(label), icon).with_desc(i18n.t(desc))
        })
        .collect::<Vec<_>>()
    });
    let on_start = Callback::new(move |v: String| {
        if let Some(action) = GameStartAction::from_config_str(&v) {
            wizard.choose(SettingUpdate::OnGameStart(action));
        }
    });
    let ask = mirror_flag(store, |s| s.ask_profile_on_launch);

    view! {
        <div class="wizard-rows">
            <RamSetting
                title=t("default_max_ram_label")
                auto_label=Signal::derive(auto_label)
                auto=auto
                ram=ram
                memory=memory
                on_auto=on_auto
                on_ram=on_ram
            />
        </div>
        <h3 class="wizard__ask">{move || i18n.t("on_game_start")}</h3>
        <ChoiceCards options=starts label=Signal::derive(move || i18n.t("on_game_start")) value=start on_change=on_start />
        <div class="wizard-rows">
            <SettingRow title=t("ask_profile_on_launch") desc=t("ask_profile_on_launch_desc")>
                <Switch
                    checked=ask
                    on_change=Callback::new(move |v| wizard.choose(SettingUpdate::AskProfileOnLaunch(v)))
                />
            </SettingRow>
        </div>
    }
}

#[component]
fn SoundStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let sounds_on = mirror_flag(store, |s| s.click_sound_enabled);
    let variant = mirror(store, |s| s.click_sound.as_config_str().to_string());
    let variants = Signal::derive(move || {
        ClickSound::ALL
            .iter()
            .map(|s| SelectOption::new(s.as_config_str(), i18n.t(s.label_key())))
            .collect::<Vec<_>>()
    });
    let auto_update = mirror_flag(store, |s| s.auto_update);
    let beta = mirror_flag(store, |s| s.include_beta_updates);
    view! {
        <div class="wizard-rows">
            <SettingRow title=t("ui_click_sound_enabled") desc=t("ui_click_sound_desc")>
                <Switch
                    checked=sounds_on
                    on_change=Callback::new(move |v| wizard.choose(SettingUpdate::ClickSoundEnabled(v)))
                />
            </SettingRow>
            <SettingRow title=t("ui_click_sound_variant") desc=t("setup_sound_variant_desc")>
                <div style="width:220px">
                    <Select
                        options=variants
                        value=variant
                        icon="volume_up"
                        disabled=Signal::derive(move || !sounds_on.get())
                        on_change=Callback::new(move |v: String| {
                            let chosen = ClickSound::from_config_str(&v);
                            sound::configure(true, chosen);
                            sound::play_click();
                            wizard.choose(SettingUpdate::ClickSound(chosen));
                        })
                    />
                </div>
            </SettingRow>
        </div>
        <h3 class="wizard__ask">{move || i18n.t("setup_updates")}</h3>
        <div class="wizard-rows">
            <SettingRow title=t("autoupdate") desc=t("setup_autoupdate_desc")>
                <Switch checked=auto_update on_change=Callback::new(move |v| wizard.choose(SettingUpdate::AutoUpdate(v))) />
            </SettingRow>
            <SettingRow title=t("beta_updates") desc=t("include_beta_updates_desc")>
                <Switch checked=beta on_change=Callback::new(move |v| wizard.choose(SettingUpdate::IncludeBetaUpdates(v))) />
            </SettingRow>
        </div>
    }
}

#[component]
fn DoneStep() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let wizard = use_wizard();
    let on_off = move |on: bool| i18n.t(if on { "setup_on" } else { "setup_off" });
    // What each step ended with; a line takes back to its step.
    let lines = move || {
        let s = store.settings.get();
        let lang = i18n.t(if s.lang == "uk_UA" { "setup_lang_uk" } else { "setup_lang_en" });
        let memory = match s.default_max_ram_gb {
            Some(gb) => i18n.tp("setup_memory_gb", &[("value", gb.to_string())]),
            None => i18n.t("setup_memory_auto"),
        };
        let start = match s.on_game_start {
            GameStartAction::Nothing => "on_game_start_nothing",
            GameStartAction::Tray => "on_game_start_tray",
            GameStartAction::Close => "on_game_start_close",
        };
        let folder = wizard
            .preview
            .with(|p| p.as_ref().map(|p| p.app_state_dir.clone()))
            .unwrap_or_else(|| wizard.dir.get());
        let sounds = if s.click_sound_enabled { i18n.t(s.click_sound.label_key()) } else { on_off(false) };
        vec![
            (Step::Welcome, "translate", i18n.t("language"), lang),
            // The folder it will really be (a foreign one gets a folder of the launcher's own).
            (Step::Storage, "folder", i18n.t("setup_wizard_launcher_data"), folder),
            (
                Step::Home,
                "history",
                i18n.t("setup_step_home"),
                i18n.t(if s.home_recent_builds > 0 { "setup_home_recent" } else { "setup_home_builds" }),
            ),
            (Step::Cards, "play_circle", i18n.t("setup_step_cards"), i18n.t(s.card_play.label_key())),
            (
                Step::Sidebar,
                "view_sidebar",
                i18n.t("setup_step_sidebar"),
                i18n.t(if s.compact_sidebar { "setup_sidebar_compact" } else { "setup_sidebar_full" }),
            ),
            (Step::Game, "memory", i18n.t("default_max_ram_label"), memory),
            (Step::Game, "sports_esports", i18n.t("on_game_start"), i18n.t(start)),
            (Step::Sound, "volume_up", i18n.t("ui_click_sound_enabled"), sounds),
            (Step::Sound, "update", i18n.t("autoupdate"), on_off(s.auto_update)),
        ]
    };
    view! {
        <div class="wizard-done">
            {move || lines().into_iter().map(|(step, icon, label, value)| {
                // The folder may not fit: its tip shows all of it.
                let tip = (step == Step::Storage).then(|| value.clone());
                view! {
                <button type="button" class="wizard-done__line" on:click=move |_| wizard.go(step)>
                    <Icon name=icon outlined=true />
                    <span class="wizard-done__label">{label}</span>
                    <span class="wizard-done__value" data-tip=tip data-tip-side="top">{value}</span>
                    <Icon name="edit" outlined=true class="wizard-done__edit" />
                </button>
            }}).collect_view()}
        </div>
        <p class="wizard__later">{move || i18n.t("setup_done_later")}</p>
    }
}
