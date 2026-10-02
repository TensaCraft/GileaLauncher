use launcher_shared::{GameStartAction, SettingUpdate, UpdateState};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{ActionTone, IconAction, Section, Select, SelectOption, SettingRow, Switch};

use super::mirror_bool;
use crate::store::{use_settings_writer, use_store};
use crate::update::{status_line, time_of_day, use_update_actions};

#[component]
pub fn LauncherSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let writer = use_settings_writer();
    let updates = use_update_actions();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));

    let lang = RwSignal::new(store.settings.get_untracked().lang);
    Effect::new(move |_| lang.set(store.settings.get().lang));
    let on_game_start =
        RwSignal::new(store.settings.get_untracked().on_game_start.as_config_str().to_string());
    Effect::new(move |_| on_game_start.set(store.settings.get().on_game_start.as_config_str().to_string()));
    let ask_profile = mirror_bool(|s| s.ask_profile_on_launch);
    let auto_update = mirror_bool(|s| s.auto_update);
    let beta = mirror_bool(|s| s.include_beta_updates);

    let update = move || store.update.get();
    let configured = move || update().is_some_and(|s| s.configured);
    let busy = move || {
        update().is_some_and(|s| matches!(s.state, UpdateState::Checking | UpdateState::Downloading { .. }))
    };
    let available = move || update().is_some_and(|s| matches!(s.state, UpdateState::Available { .. }));
    let update_desc = move || match update() {
        None => i18n.t("launcher_update_status_unknown"),
        Some(status) => {
            let line = i18n.text(&status_line(&status, status.last_checked_ms.map(time_of_day)));
            match &status.state {
                UpdateState::Failed { error } => format!("{line}: {}", i18n.error(error)),
                _ => line,
            }
        }
    };

    let game_start_options = Signal::derive(move || {
        vec![
            SelectOption::new("nothing", i18n.t("on_game_start_nothing")),
            SelectOption::new("close", i18n.t("on_game_start_close")),
            SelectOption::new("tray", i18n.t("on_game_start_tray")),
        ]
    });
    let languages = Signal::derive(move || {
        vec![
            SelectOption::new("uk_UA", i18n.t("language_uk")),
            SelectOption::new("en_US", i18n.t("language_en")),
        ]
    });

    view! {
        <Section icon="tune" title=t("launcher_behavior") desc=t("launcher_behavior_desc")>
            <SettingRow title=t("language") desc=t("setting_language_desc")>
                <div style="width:220px">
                    <Select options=languages value=lang icon="translate" on_change=Callback::new(move |l: String| writer.apply(SettingUpdate::Lang(l))) />
                </div>
            </SettingRow>
            <SettingRow title=t("on_game_start") desc=t("on_game_start_desc")>
                <div style="width:220px">
                    <Select
                        options=game_start_options
                        value=on_game_start
                        icon="sports_esports"
                        on_change=Callback::new(move |raw: String| {
                            if let Some(action) = GameStartAction::from_config_str(&raw) {
                                writer.apply(SettingUpdate::OnGameStart(action));
                            }
                        })
                    />
                </div>
            </SettingRow>
            <SettingRow title=t("ask_profile_on_launch") desc=t("ask_profile_on_launch_desc")>
                <Switch checked=ask_profile on_change=Callback::new(move |v| writer.apply(SettingUpdate::AskProfileOnLaunch(v))) />
            </SettingRow>
            <SettingRow title=t("autoupdate") desc=Signal::derive(update_desc)>
                // Just an icon with a tooltip, as tall as the switch beside it.
                {move || if available() {
                    view! {
                        <IconAction
                            icon="download"
                            tone=ActionTone::Ok
                            compact=true
                            title=t("update_install")
                            on_click=Callback::new(move |()| updates.download())
                        />
                    }.into_any()
                } else {
                    view! {
                        <IconAction
                            icon="update"
                            compact=true
                            title=t("check_updates_now")
                            loading=Signal::derive(busy)
                            disabled=Signal::derive(move || !configured())
                            on_click=Callback::new(move |()| updates.check())
                        />
                    }.into_any()
                }}
                // The switch goes last, in the column of the other rows' switches.
                <Switch checked=auto_update on_change=Callback::new(move |v| writer.apply(SettingUpdate::AutoUpdate(v))) />
            </SettingRow>
            <SettingRow title=t("beta_updates") desc=t("include_beta_updates_desc")>
                <Switch checked=beta on_change=Callback::new(move |v| writer.apply(SettingUpdate::IncludeBetaUpdates(v))) />
            </SettingRow>
        </Section>
    }
}
