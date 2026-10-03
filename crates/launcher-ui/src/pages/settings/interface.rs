use launcher_shared::recent::RECENT_MOST;
use launcher_shared::{ClickSound, SettingUpdate, WINDOW_MAX, WINDOW_MIN, WINDOW_PRESETS, WindowSize};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Field, Section, Select, SelectOption, SettingRow, Switch, TextInput, Variant, sound};

use super::mirror_bool;
use crate::store::{use_settings_writer, use_store};

/// The list entry for a stored window size: `fullscreen`, `maximized`, a preset, or `custom`.
pub fn window_choice(stored: &str) -> String {
    match WindowSize::parse(stored).unwrap_or_default() {
        WindowSize::Size { width, height } if !WINDOW_PRESETS.contains(&(width, height)) => {
            "custom".to_string()
        }
        size => size.as_config_str(),
    }
}

#[component]
pub fn InterfaceSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let writer = use_settings_writer();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let compact = mirror_bool(|s| s.compact_sidebar);
    let recent = RwSignal::new(store.settings.get_untracked().home_recent_builds.to_string());
    Effect::new(move |_| recent.set(store.settings.get().home_recent_builds.to_string()));
    let recent_options = Signal::derive(move || {
        (0..=RECENT_MOST)
            .map(|n| {
                let label = if n == 0 { i18n.t("home_recent_builds_none") } else { n.to_string() };
                SelectOption::new(n.to_string(), label)
            })
            .collect::<Vec<_>>()
    });
    let sounds_on = mirror_bool(|s| s.click_sound_enabled);
    let variant = RwSignal::new(store.settings.get_untracked().click_sound.as_config_str().to_string());
    Effect::new(move |_| variant.set(store.settings.get().click_sound.as_config_str().to_string()));
    let variants = Signal::derive(move || {
        ClickSound::ALL
            .iter()
            .map(|s| SelectOption::new(s.as_config_str(), i18n.t(s.label_key())))
            .collect::<Vec<_>>()
    });

    // Follows the saved size only when it changes: another setting saved meanwhile keeps "Custom"
    // open while its fields are filled in.
    let stored = Memo::new(move |_| store.settings.with(|s| s.window_size.clone()));
    let window = RwSignal::new(window_choice(&store.settings.get_untracked().window_size));
    Effect::new(move |_| window.set(window_choice(&stored.get())));
    let initial = WindowSize::parse(&store.settings.get_untracked().window_size).unwrap_or_default();
    let (start_w, start_h) = match initial {
        WindowSize::Size { width, height } => (width, height),
        _ => WINDOW_PRESETS[0],
    };
    let custom_w = RwSignal::new(start_w.to_string());
    let custom_h = RwSignal::new(start_h.to_string());
    let custom_error = RwSignal::new(None::<String>);
    let window_options = Signal::derive(move || {
        let mut options = vec![
            SelectOption::new("fullscreen", i18n.t("window_size_fullscreen")),
            SelectOption::new("maximized", i18n.t("window_size_maximized")),
        ];
        options.extend(
            WINDOW_PRESETS.iter().map(|(w, h)| SelectOption::new(format!("{w}x{h}"), format!("{w} × {h}"))),
        );
        options.push(SelectOption::new("custom", i18n.t("window_size_custom")));
        options
    });
    let apply_custom = move || {
        let size =
            custom_w.get_untracked().trim().parse().ok().zip(custom_h.get_untracked().trim().parse().ok());
        match size.and_then(|(w, h)| WindowSize::sized(w, h)) {
            Some(size) => {
                custom_error.set(None);
                writer.apply(SettingUpdate::WindowSize(size.as_config_str()));
            }
            None => custom_error.set(Some(i18n.tp(
                "window_size_invalid",
                &[
                    ("min", format!("{} × {}", WINDOW_MIN.0, WINDOW_MIN.1)),
                    ("max", format!("{} × {}", WINDOW_MAX.0, WINDOW_MAX.1)),
                ],
            ))),
        }
    };

    view! {
        <Section icon="dashboard" title=t("interface") desc=t("interface_desc")>
            <SettingRow title=t("compact_sidebar") desc=t("compact_sidebar_desc")>
                <Switch checked=compact on_change=Callback::new(move |v| writer.apply(SettingUpdate::CompactSidebar(v))) />
            </SettingRow>
            <SettingRow title=t("home_recent_builds") desc=t("home_recent_builds_desc")>
                <div style="width:220px">
                    <Select
                        options=recent_options
                        value=recent
                        icon="history"
                        on_change=Callback::new(move |v: String| {
                            if let Ok(count) = v.parse() {
                                writer.apply(SettingUpdate::HomeRecentBuilds(count));
                            }
                        })
                    />
                </div>
            </SettingRow>
            <SettingRow title=t("ui_click_sound_enabled") desc=t("ui_click_sound_desc")>
                <Switch checked=sounds_on on_change=Callback::new(move |v| writer.apply(SettingUpdate::ClickSoundEnabled(v))) />
            </SettingRow>
            <SettingRow title=t("ui_click_sound_variant")>
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
                            writer.apply(SettingUpdate::ClickSound(chosen));
                        })
                    />
                </div>
            </SettingRow>
            <SettingRow title=t("window_size_label") desc=t("window_size_desc")>
                <div style="width:240px">
                    <Select
                        options=window_options
                        value=window
                        icon="aspect_ratio"
                        on_change=Callback::new(move |v: String| {
                            if v != "custom" {
                                writer.apply(SettingUpdate::WindowSize(v));
                            }
                        })
                    />
                </div>
            </SettingRow>
            <Show when=move || window.get() == "custom">
                <div class="window-custom">
                    <Field label=t("window_size_width") error=Signal::derive(move || custom_error.get())>
                        <TextInput value=custom_w on_enter=Callback::new(move |()| apply_custom()) />
                    </Field>
                    <Field label=t("window_size_height")>
                        <TextInput value=custom_h on_enter=Callback::new(move |()| apply_custom()) />
                    </Field>
                    <Button variant=Variant::Primary icon="check" on_click=move |_| apply_custom()>
                        {move || i18n.t("window_size_apply")}
                    </Button>
                </div>
            </Show>
        </Section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_choices_map_presets_and_custom_sizes() {
        assert_eq!(window_choice("fullscreen"), "fullscreen");
        assert_eq!(window_choice("maximized"), "maximized");
        assert_eq!(window_choice("1366x800"), "1366x800");
        assert_eq!(window_choice("1500x900"), "custom");
        assert_eq!(window_choice(""), "1366x800", "nothing stored: the default size");
    }
}
