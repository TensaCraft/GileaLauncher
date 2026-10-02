//! Settings → Java and performance: the default memory limit and GPU mode for
//! new builds, and the user's own Java list. Java auto-discovered on this computer is not shown
//! here (it still backs the per-build Java picker); "Scan Java" imports it into the user's list.

use launcher_shared::{JavaEntry, JavaList, Level, MemoryInfo, SettingUpdate};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    ActionTone, Button, Field, IconAction, InFlight, PathField, Section, SegOption, Segmented, SettingRow,
    TextInput, Variant, ipc, use_toasts,
};

use crate::builds::ram::RamSetting;
use crate::builds::ram_slider;
use crate::store::{use_settings_writer, use_store};

/// What the GPU switch shows: `auto`, `igpu`, or `dgpu` (the default).
pub fn gpu_choice(raw: &str) -> &'static str {
    match raw {
        "auto" => "auto",
        "igpu" => "igpu",
        _ => "dgpu",
    }
}

/// Where the memory slider starts: the saved limit within range, else the recommended amount.
pub fn ram_start(saved: Option<u64>, info: &MemoryInfo) -> f64 {
    let (min, max, recommended) = ram_slider(info);
    saved.map_or(recommended, |gb| (gb as f64).clamp(min, max))
}

#[derive(serde::Serialize)]
struct AddArgs {
    label: String,
    path: String,
}

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

#[derive(serde::Serialize)]
struct StartArgs {
    start: Option<String>,
}

#[component]
pub fn JavaSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let writer = use_settings_writer();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let memory = RwSignal::new(None::<MemoryInfo>);
    let java = RwSignal::new(JavaList::default());
    spawn_local(async move {
        if let Ok(info) = ipc::call::<MemoryInfo>("memory_info").await {
            let _ = memory.try_set(Some(info));
        }
        if let Ok(list) = ipc::call::<JavaList>("java_list").await {
            let _ = java.try_set(list);
        }
    });

    let auto = RwSignal::new(store.settings.get_untracked().default_max_ram_gb.is_none());
    let ram = RwSignal::new(1.0);
    Effect::new(move |_| {
        if let Some(info) = memory.get() {
            ram.set(ram_start(store.settings.get_untracked().default_max_ram_gb, &info));
        }
    });
    let on_auto = Callback::new(move |on: bool| {
        let value = if on { None } else { Some(ram.get_untracked().round().max(1.0) as u64) };
        writer.apply(SettingUpdate::DefaultMaxRamGb(value));
    });
    let on_ram = Callback::new(move |gb: f64| {
        writer.apply(SettingUpdate::DefaultMaxRamGb(Some(gb.round().max(1.0) as u64)))
    });
    let gpu = RwSignal::new(gpu_choice(&store.settings.get_untracked().gpu_mode_default).to_string());
    let gpu_options = Signal::derive(move || {
        vec![
            SegOption::new("auto", i18n.t("gpu_mode_auto")),
            SegOption::new("igpu", i18n.t("gpu_mode_integrated")),
            SegOption::new("dgpu", i18n.t("gpu_mode_discrete")),
        ]
    });
    let on_gpu = Callback::new(move |mode: String| writer.apply(SettingUpdate::GpuModeDefault(mode)));

    let name = RwSignal::new(String::new());
    let path = RwSignal::new(String::new());
    let path_error = RwSignal::new(None::<String>);
    // One of each at a time; the section may be gone (another one chosen) when an answer comes.
    let (scanning, adding, removing) = (InFlight::new(), InFlight::new(), InFlight::new());
    let browse = Callback::new(move |()| {
        let start = Some(path.get_untracked()).filter(|p| !p.trim().is_empty());
        spawn_local(async move {
            if let Ok(Some(picked)) =
                ipc::invoke::<_, Option<String>>("pick_java_file", &StartArgs { start }).await
            {
                let _ = path.try_set(picked);
                let _ = path_error.try_set(None);
            }
        });
    });
    let add = move || {
        if !adding.start() {
            return;
        }
        let args = AddArgs { label: name.get_untracked(), path: path.get_untracked() };
        spawn_local(async move {
            match ipc::invoke::<_, JavaList>("java_add", &args).await {
                Ok(list) => {
                    let _ = java.try_set(list);
                    let _ = name.try_set(String::new());
                    let _ = path.try_set(String::new());
                    let _ = path_error.try_set(None);
                }
                Err(e) => {
                    let _ = path_error.try_set(Some(i18n.error(&e)));
                }
            }
            adding.done();
        });
    };
    let scan = move || {
        if !scanning.start() {
            return;
        }
        spawn_local(async move {
            match ipc::call::<JavaList>("java_scan").await {
                Ok(list) => {
                    let _ = java.try_set(list);
                }
                Err(e) => {
                    let error = i18n.error(&e);
                    toasts.show(Level::Error, i18n.tp("custom_java_scan_failed", &[("error", error)]), None);
                }
            }
            scanning.done();
        });
    };
    let remove = move |target: String| {
        if !removing.start() {
            return;
        }
        spawn_local(async move {
            if let Ok(list) = ipc::invoke::<_, JavaList>("java_remove", &PathArgs { path: target }).await {
                let _ = java.try_set(list);
            }
            removing.done();
        });
    };
    let rows = move |entries: Vec<JavaEntry>| {
        entries
            .into_iter()
            .map(|entry| {
                let target = entry.path.clone();
                let full_path = entry.path.clone();
                view! {
                    <div class="java-row">
                        <div class="java-row__text">
                            <div class="java-row__label">{entry.label}</div>
                            <div class="java-row__path" data-tip=full_path data-tip-side="top">{entry.path}</div>
                        </div>
                        <IconAction
                            icon="delete_outline"
                            tone=ActionTone::Danger
                            title=t("delete")
                            loading=removing.running()
                            on_click=Callback::new(move |()| remove(target.clone()))
                        />
                    </div>
                }
            })
            .collect_view()
    };
    let auto_label = move || {
        let value = memory.get().map(|m| m.recommended_heap_gb.to_string()).unwrap_or_default();
        i18n.tp("default_max_ram_auto", &[("value", value)])
    };

    view! {
        <Section icon="memory" title=t("java_performance_section") desc=t("java_performance_desc")>
            <RamSetting
                title=t("default_max_ram_label")
                auto_label=Signal::derive(auto_label)
                auto=auto
                ram=ram
                memory=memory
                on_auto=on_auto
                on_ram=on_ram
            />
            <SettingRow title=t("gpu_mode_default_label")>
                <Segmented options=gpu_options value=gpu on_change=on_gpu />
            </SettingRow>
        </Section>
        <Section icon="coffee" title=t("custom_java_section") desc=t("custom_java_section_desc")>
            <div class="grid2">
                <Field label=t("custom_java_name_label")>
                    <TextInput value=name />
                </Field>
                <Field label=t("custom_java_path_label") error=Signal::derive(move || path_error.get())>
                    <PathField
                        value=path
                        browse_label=t("browse_file")
                        on_browse=browse
                        invalid=Signal::derive(move || path_error.get().is_some())
                    />
                </Field>
            </div>
            <div class="wrap">
                <Button variant=Variant::Primary icon="add" loading=adding.running() on_click=move |_| add()>
                    {move || i18n.t("custom_java_add")}
                </Button>
                <Button icon="search" loading=scanning.running() on_click=move |_| scan()>
                    {move || i18n.t("custom_java_scan")}
                </Button>
            </div>
            <div class="java-list">
                {move || {
                    let custom = java.with(|l| l.custom.clone());
                    if custom.is_empty() {
                        view! { <div class="hint">{move || i18n.t("custom_java_empty")}</div> }.into_any()
                    } else {
                        rows(custom).into_any()
                    }
                }}
            </div>
        </Section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_and_memory_start_from_the_settings() {
        assert_eq!(gpu_choice("auto"), "auto");
        assert_eq!(gpu_choice("igpu"), "igpu");
        assert_eq!(gpu_choice(""), "dgpu");
        assert_eq!(gpu_choice("rtx"), "dgpu");
        let info = MemoryInfo { total_gb: 16, min_heap_gb: 1, max_heap_gb: 14, recommended_heap_gb: 6 };
        assert_eq!(ram_start(None, &info), 6.0);
        assert_eq!(ram_start(Some(8), &info), 8.0);
        assert_eq!(ram_start(Some(64), &info), 14.0, "clamped to what this computer allows");
    }
}
