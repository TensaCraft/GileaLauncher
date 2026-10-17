//! «Налаштування»: memory, graphics card, account, icon and description — the build settings
//! page's own controls; what the player leaves as it is stays the launcher's default.

use launcher_shared::MemoryInfo;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Field, Icon, Section, Select, SelectOption, TextArea, Variant, ipc};

use super::Wizard;
use crate::builds::gpu::GpuChoice;
use crate::builds::ram::RamSetting;
use crate::store::use_store;

/// The memory the system reports; the slider shows the advice while the limit is automatic. A
/// step already closed lets it go.
fn take_memory(
    info: MemoryInfo,
    memory: RwSignal<Option<MemoryInfo>>,
    auto_ram: RwSignal<bool>,
    ram: RwSignal<f64>,
) {
    let _ = memory.try_set(Some(info));
    if auto_ram.try_get_untracked() == Some(true) {
        let _ = ram.try_set(info.recommended_heap_gb as f64);
    }
}

/// A picked file's name, for the line under the button.
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

#[component]
pub fn Settings(wizard: Wizard) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let start = wizard.state.get_untracked().settings;
    let auto_ram = RwSignal::new(start.max_ram_gb.is_none());
    let ram = RwSignal::new(start.max_ram_gb.unwrap_or(4) as f64);
    let memory = RwSignal::new(None::<MemoryInfo>);
    spawn_local(async move {
        if let Ok(info) = ipc::call::<MemoryInfo>("memory_info").await {
            take_memory(info, memory, auto_ram, ram);
        }
    });
    Effect::new(move |_| {
        let limit = (!auto_ram.get()).then(|| ram.get().round().max(1.0) as u64);
        wizard.state.update(|s| s.settings.max_ram_gb = limit);
    });
    let auto_label = move || {
        let value = store
            .settings
            .with(|s| s.default_max_ram_gb)
            .or_else(|| memory.get().map(|m| m.recommended_heap_gb))
            .map(|gb| gb.to_string())
            .unwrap_or_default();
        i18n.tp("default_max_ram_auto", &[("value", value)])
    };
    let gpu = RwSignal::new(
        start
            .gpu_mode
            .clone()
            .unwrap_or_else(|| store.settings.with_untracked(|s| s.gpu_mode_default.clone())),
    );
    let on_gpu = Callback::new(move |mode: String| wizard.state.update(|s| s.settings.gpu_mode = Some(mode)));
    let profile = RwSignal::new(start.profile.clone().unwrap_or_default());
    let profiles = Signal::derive(move || {
        let mut options = vec![SelectOption::new(String::new(), i18n.t("wizard_account_default"))];
        options.extend(store.profiles.with(|list| {
            list.iter().map(|p| SelectOption::new(p.key.clone(), p.name.clone())).collect::<Vec<_>>()
        }));
        options
    });
    let on_profile = Callback::new(move |key: String| {
        wizard.state.update(|s| s.settings.profile = Some(key).filter(|k| !k.is_empty()));
    });
    let description = RwSignal::new(start.description.clone());
    Effect::new(move |_| {
        let text = description.get();
        wizard.state.update(|s| s.settings.description = text);
    });
    let icon = Signal::derive(move || wizard.state.with(|s| s.settings.image_path.clone()));
    let pick_icon = move || {
        spawn_local(async move {
            if let Ok(Some(path)) = ipc::call::<Option<String>>("pick_image_file").await {
                let _ = wizard.state.try_update(|s| s.settings.image_path = Some(path));
            }
        });
    };

    view! {
        <div class="build-wizard__settings">
            <Section icon="memory" title=t("wizard_memory")>
                <RamSetting title=t("max_ram_label") auto_label=Signal::derive(auto_label) auto=auto_ram ram=ram memory=memory />
            </Section>
            <Section icon="developer_board" title=t("gpu_mode_label")>
                <GpuChoice value=gpu on_change=on_gpu />
            </Section>
            <Section icon="person" title=t("wizard_account")>
                <Select options=profiles value=profile on_change=on_profile />
            </Section>
            <Section icon="image" title=t("wizard_icon_and_description")>
                <Field label=t("wizard_icon")>
                    <div class="wrap">
                        <Button variant=Variant::Ghost icon="image" on_click=move |_| pick_icon()>
                            {move || i18n.t("select_icon")}
                        </Button>
                        {move || icon.get().map(|path| view! {
                            <span class="hint"><Icon name="check" />{file_name(&path)}</span>
                            <Button
                                variant=Variant::Ghost
                                icon="close"
                                on_click=move |_| wizard.state.update(|s| s.settings.image_path = None)
                            >
                                {move || i18n.t("remove_icon")}
                            </Button>
                        })}
                    </div>
                </Field>
                <Field label=t("build_description_label")>
                    <TextArea value=description rows=2 placeholder=t("build_description_hint") />
                </Field>
            </Section>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_that_comes_after_the_step_closed_is_let_go() {
        let step = Owner::new();
        let (memory, auto_ram, ram) =
            step.with(|| (RwSignal::new(None::<MemoryInfo>), RwSignal::new(true), RwSignal::new(4.0)));
        step.cleanup();
        let info = MemoryInfo { total_gb: 16, min_heap_gb: 1, max_heap_gb: 14, recommended_heap_gb: 6 };
        take_memory(info, memory, auto_ram, ram);
    }

    #[test]
    fn a_picked_icon_is_named_by_its_file() {
        assert_eq!(super::file_name(r"C:\Users\Player\Pictures\cube.png"), "cube.png");
        assert_eq!(super::file_name("/home/player/cube.png"), "cube.png");
    }
}
