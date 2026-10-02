//! Build settings, a tab of the build's content page: General (name, component,
//! icon, quick-join server), Runtime (Java, GPU, memory) and JVM arguments, saved together.

use launcher_shared::args;
use launcher_shared::{
    AccountKind, AppError, BuildSettingsDto, BuildSettingsUpdate, ComponentDto, ComponentsSnapshot,
    ErrorCode, JavaList, Level, LoaderKind, MemoryInfo, ProfileDto,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Checkbox, CodeEditor, Dialog, DialogFooter, Field, Icon, NavEntry, Section, SegOption, Segmented,
    Select, SelectOption, SettingRow, SettingsNav, Size, TextInput, Variant, card_image, ipc, use_toasts,
};

use crate::builds::catalog::{CreateRow, build_choices, load_rows};
use crate::builds::ram::RamSetting;
use crate::builds::{LatestRequest, ram_slider};
use crate::pages::settings::java::gpu_choice;
use crate::shell::use_header;
use crate::store::use_store;

/// "Automatic" in the Java list.
pub const AUTO_JAVA: &str = "__launcher_auto__";
/// The performance preset: the official launcher's G1, as a build without its own gets it anyway.
const PERFORMANCE_ARGUMENTS: [&str; 6] = launcher_shared::args::DEFAULT_GC_ARGUMENTS;

/// Automatic first, then the user's and the found Java (each path once, "label (file)"), and the
/// build's saved path when neither list has it.
pub fn java_choices(
    list: &JavaList,
    saved: Option<&str>,
    auto_label: &str,
    saved_label: &str,
) -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new(AUTO_JAVA, auto_label)];
    for entry in list.custom.iter().chain(&list.launcher) {
        if !options.iter().any(|o| o.value == entry.path) {
            let file = entry.path.rsplit(['\\', '/']).next().unwrap_or_default();
            options.push(SelectOption::new(entry.path.clone(), format!("{} ({file})", entry.label)));
        }
    }
    if let Some(path) = saved.filter(|p| !p.is_empty() && !options.iter().any(|o| o.value == *p)) {
        options.push(SelectOption::new(path, format!("{saved_label}: {path}")));
    }
    options
}

/// Installed components ("Minecraft 1.21.1", "Fabric 1.21.1 (0.16.9)", else the id), with the
/// build's own first when it is not installed (anymore).
pub fn component_choices(components: &[ComponentDto], current: Option<&str>) -> Vec<SelectOption> {
    let mut options: Vec<SelectOption> = components
        .iter()
        .map(|c| {
            let label = match (c.loader, &c.minecraft, &c.loader_version) {
                (Some(LoaderKind::Minecraft), Some(mc), _) => format!("Minecraft {mc}"),
                (Some(kind), Some(mc), Some(lv)) => format!("{} {mc} ({lv})", kind.display_name()),
                _ => c.id.clone(),
            };
            SelectOption::new(c.id.clone(), label)
        })
        .collect();
    if let Some(id) = current.filter(|id| !id.is_empty() && !options.iter().any(|o| o.value == *id)) {
        options.insert(0, SelectOption::new(id, id));
    }
    options
}

/// The arguments after a preset: `keep` leaves them, `none` clears them, `performance` puts G1GC.
pub fn apply_preset(preset: &str, current: &str) -> String {
    match preset {
        "none" => String::new(),
        "performance" => PERFORMANCE_ARGUMENTS.join("\n"),
        _ => current.to_string(),
    }
}

/// Empty, or a port 1–65535.
pub fn port_ok(raw: &str) -> bool {
    let raw = raw.trim();
    raw.is_empty() || raw.parse::<u16>().is_ok_and(|port| port > 0)
}

/// The lines the player typed, blank ones dropped; the backend splits each into words (quotes
/// keep blanks, `launcher_shared::args`).
pub fn arguments_of(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// The page's fields.
#[derive(Clone, Copy)]
pub struct SettingsForm {
    pub name: RwSignal<String>,
    pub component: RwSignal<String>,
    pub java: RwSignal<String>,
    pub gpu: RwSignal<String>,
    pub auto_ram: RwSignal<bool>,
    pub ram: RwSignal<f64>,
    pub arguments: RwSignal<String>,
    pub preset: RwSignal<String>,
    pub host: RwSignal<String>,
    pub port: RwSignal<String>,
    pub image_path: RwSignal<Option<String>>,
    pub remove_image: RwSignal<bool>,
    /// The build's own account (a profile key); empty follows the launcher's settings.
    pub profile: RwSignal<String>,
}

impl Default for SettingsForm {
    fn default() -> SettingsForm {
        SettingsForm::new()
    }
}

impl SettingsForm {
    pub fn new() -> SettingsForm {
        SettingsForm {
            name: RwSignal::new(String::new()),
            component: RwSignal::new(String::new()),
            java: RwSignal::new(AUTO_JAVA.to_string()),
            gpu: RwSignal::new("dgpu".to_string()),
            auto_ram: RwSignal::new(true),
            ram: RwSignal::new(1.0),
            arguments: RwSignal::new(String::new()),
            preset: RwSignal::new("keep".to_string()),
            host: RwSignal::new(String::new()),
            port: RwSignal::new(String::new()),
            image_path: RwSignal::new(None),
            remove_image: RwSignal::new(false),
            profile: RwSignal::new(String::new()),
        }
    }

    /// Every field from saved settings; `try_set` because answers may come after the page is gone.
    pub fn fill(&self, s: &BuildSettingsDto) {
        self.take_component(s);
        let _ = self.name.try_set(s.name.clone());
        let _ = self.gpu.try_set(gpu_choice(&s.gpu_mode).to_string());
        let _ = self.auto_ram.try_set(s.max_ram_gb.is_none());
        if let Some(gb) = s.max_ram_gb {
            let _ = self.ram.try_set(gb as f64);
        }
        // One word a line; a word with blanks in quotes, so saving reads it back whole.
        let lines: Vec<String> = s.jvm_arguments.iter().map(|word| args::shown(word)).collect();
        let _ = self.arguments.try_set(lines.join("\n"));
        let _ = self.preset.try_set("keep".to_string());
        let _ = self.host.try_set(s.server_host.clone());
        let _ = self.port.try_set(s.server_port.map(|p| p.to_string()).unwrap_or_default());
        let _ = self.image_path.try_set(None);
        let _ = self.remove_image.try_set(false);
        let _ = self.profile.try_set(s.profile.clone().unwrap_or_default());
    }

    /// After "Install and use": the new component and the Java the install gave it; everything
    /// else keeps what the user typed.
    pub fn take_component(&self, s: &BuildSettingsDto) {
        let _ = self.component.try_set(s.component.clone().unwrap_or_default());
        let _ = self.java.try_set(s.java_path.clone().unwrap_or_else(|| AUTO_JAVA.to_string()));
    }

    /// What "Save" sends; automatic memory sends no limit.
    pub fn update(&self) -> BuildSettingsUpdate {
        BuildSettingsUpdate {
            name: self.name.get_untracked(),
            component: Some(self.component.get_untracked()).filter(|c| !c.is_empty()),
            java_path: Some(self.java.get_untracked()).filter(|j| j != AUTO_JAVA),
            gpu_mode: self.gpu.get_untracked(),
            max_ram_gb: (!self.auto_ram.get_untracked())
                .then(|| self.ram.get_untracked().round().max(1.0) as u64),
            jvm_arguments: arguments_of(&self.arguments.get_untracked()),
            server_host: self.host.get_untracked(),
            server_port: self.port.get_untracked(),
            image_path: self.image_path.get_untracked(),
            remove_image: self.remove_image.get_untracked(),
            profile: Some(self.profile.get_untracked()).filter(|p| !p.is_empty()),
        }
    }
}

/// The accounts a build may start with: the launcher's choice first (empty value), then each
/// account; one the build names that is gone stays listed, so the page shows what it has.
pub fn account_choices(
    profiles: &[ProfileDto],
    current: &str,
    launcher: &str,
    kind: impl Fn(AccountKind) -> String,
    gone: &str,
) -> Vec<SelectOption> {
    let mut choices = vec![SelectOption::new("", launcher)];
    choices.extend(
        profiles.iter().map(|p| SelectOption::new(p.key.clone(), format!("{} ({})", p.name, kind(p.kind)))),
    );
    if !current.is_empty() && !profiles.iter().any(|p| p.key == current) {
        choices.push(SelectOption::new(current, format!("{current} ({gone})")));
    }
    choices
}

/// What picking an icon gave: a file to use, nothing (cancelled), or a file that is no usable
/// image (the backend checks it when it is picked).
#[derive(Debug, PartialEq, Eq)]
pub enum IconPick {
    Use(String),
    Cancelled,
    Invalid,
}

pub fn icon_pick(result: Result<Option<String>, AppError>) -> IconPick {
    match result {
        Ok(Some(path)) => IconPick::Use(path),
        Ok(None) => IconPick::Cancelled,
        Err(_) => IconPick::Invalid,
    }
}

/// The loader a build runs, from its client and component (the original's `_current_loader_id`).
pub fn current_loader(client: Option<&str>, component: Option<&str>) -> &'static str {
    let text = format!("{}{}", client.unwrap_or_default(), component.unwrap_or_default())
        .to_lowercase()
        .replace(' ', "");
    if text.contains("neoforge") {
        return "neoforge";
    }
    ["fabric", "forge", "quilt", "minecraft"].into_iter().find(|id| text.contains(id)).unwrap_or("minecraft")
}

/// The build's own Minecraft version when the list has it, else the first (newest).
pub fn preferred_mc(rows: &[CreateRow], current: Option<&str>) -> String {
    current
        .filter(|mc| rows.iter().any(|row| row.mc == *mc))
        .map(str::to_string)
        .or_else(|| rows.first().map(|row| row.mc.clone()))
        .unwrap_or_default()
}

/// The build's own loader build when `row` offers it, else the row's default.
pub fn preferred_build(row: Option<&CreateRow>, current: Option<&str>) -> String {
    let Some(row) = row else { return String::new() };
    current
        .filter(|lv| row.builds.iter().any(|b| b.version == *lv))
        .map(str::to_string)
        .or_else(|| row.default_version.clone())
        .or_else(|| row.builds.first().map(|b| b.version.clone()))
        .unwrap_or_default()
}

/// The loaders the change dialog offers.
pub fn loader_choices() -> Vec<SelectOption> {
    [LoaderKind::Minecraft, LoaderKind::Fabric, LoaderKind::Quilt, LoaderKind::Forge, LoaderKind::NeoForge]
        .into_iter()
        .map(|kind| SelectOption::new(kind_value(kind), kind.display_name()))
        .collect()
}

fn kind_value(kind: LoaderKind) -> &'static str {
    match kind {
        LoaderKind::Minecraft => "minecraft",
        LoaderKind::Fabric => "fabric",
        LoaderKind::Quilt => "quilt",
        LoaderKind::Forge => "forge",
        LoaderKind::NeoForge => "neoforge",
    }
}

fn kind_of(value: &str) -> LoaderKind {
    match value {
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "forge" => LoaderKind::Forge,
        "neoforge" => LoaderKind::NeoForge,
        _ => LoaderKind::Minecraft,
    }
}

/// The Minecraft versions of the loaded rows; `snapshot` (the language's word) marks snapshots.
pub fn minecraft_choices(rows: &[CreateRow], snapshot: &str) -> Vec<SelectOption> {
    rows.iter()
        .map(|row| {
            let label = if row.snapshot { format!("{} ({snapshot})", row.mc) } else { row.mc.clone() };
            SelectOption::new(row.mc.clone(), label)
        })
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeArgs {
    key: String,
    loader: LoaderKind,
    mc: String,
    loader_version: Option<String>,
}

/// "Change the game's build" (the original's `VersionComponentModal`): loader, Minecraft version and
/// loader build, starting from the build's own; "Install and use" runs as an operation and
/// `on_done` gets the new settings.
#[component]
fn ChangeComponentDialog(
    open: RwSignal<bool>,
    build_key: String,
    on_done: Callback<BuildSettingsDto>,
) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let loader = RwSignal::new("minecraft".to_string());
    // The build's Minecraft version and loader build, preferred wherever the lists have them.
    let wanted = StoredValue::new((None::<String>, None::<String>));
    let unstable = RwSignal::new(false);
    let rows = RwSignal::new(None::<Result<Vec<CreateRow>, String>>);
    let mc = RwSignal::new(String::new());
    let build = RwSignal::new(String::new());
    let latest = StoredValue::new_local(LatestRequest::new());
    let load = move || {
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        rows.set(None);
        let (kind, show_unstable) = (kind_of(&loader.get_untracked()), unstable.get_untracked());
        spawn_local(async move {
            let result = load_rows(kind, show_unstable).await;
            if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                if let Ok(list) = &result {
                    let (want_mc, want_lv) = wanted.get_value();
                    let chosen = preferred_mc(list, want_mc.as_deref());
                    let row = list.iter().find(|row| row.mc == chosen);
                    let _ = build.try_set(preferred_build(row, want_lv.as_deref()));
                    let _ = mc.try_set(chosen);
                }
                let _ = rows.try_set(Some(result.map_err(|e| i18n.error(&e))));
            }
        });
    };
    let start_key = build_key.clone();
    Effect::new(move |was_open: Option<bool>| {
        let is_open = open.get();
        if is_open && was_open != Some(true) {
            let current = store.builds.with_untracked(|b| b.iter().find(|b| b.key == start_key).cloned());
            if let Some(current) = current {
                wanted.set_value((current.version.clone(), current.loader_version.clone()));
                loader.set(current_loader(current.client.as_deref(), current.loader.as_deref()).to_string());
            }
        }
        is_open
    });
    Effect::new(move |_| {
        if open.get() {
            loader.track();
            unstable.track();
            load();
        }
    });
    let selected_row = move || {
        let chosen = mc.get();
        rows.with(|r| {
            r.as_ref()
                .and_then(|r| r.as_ref().ok())
                .and_then(|list| list.iter().find(|row| row.mc == chosen).cloned())
        })
    };
    let mc_options = Signal::derive(move || {
        rows.with(|r| {
            r.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|list| minecraft_choices(list, &i18n.t("version_create_snapshot_badge")))
                .unwrap_or_default()
        })
    });
    let build_options = Signal::derive(move || {
        build_choices(selected_row().as_ref(), &i18n.t("version_create_unstable_loader_badge"))
    });
    let on_mc = Callback::new(move |_: String| {
        build.set(preferred_build(selected_row().as_ref(), wanted.get_value().1.as_deref()));
    });
    let status = move || match rows.get() {
        None => i18n.t("version_component_loading"),
        Some(Err(message)) => i18n.tp("version_component_load_failed", &[("error", message)]),
        Some(Ok(list)) if list.is_empty() => i18n.t("version_component_no_versions"),
        Some(Ok(_)) => i18n.t("version_component_ready"),
    };
    let ready = move || rows.with(|r| matches!(r, Some(Ok(list)) if !list.is_empty()));
    let submit = move || {
        let kind = kind_of(&loader.get_untracked());
        let chosen = mc.get_untracked();
        if chosen.is_empty() {
            toasts.show(Level::Error, i18n.t("version_component_selection_required"), None);
            return;
        }
        let args = ChangeArgs {
            key: build_key.clone(),
            loader: kind,
            mc: chosen,
            loader_version: (kind != LoaderKind::Minecraft).then(|| build.get_untracked()),
        };
        open.set(false);
        spawn_local(async move {
            match ipc::invoke::<_, BuildSettingsDto>("build_change_component", &args).await {
                Ok(settings) => {
                    let _ = on_done.try_run(settings);
                }
                Err(e) => toasts.show(
                    Level::Error,
                    i18n.tp("version_component_install_failed", &[("error", i18n.error(&e))]),
                    None,
                ),
            }
        });
    };
    let submit = StoredValue::new(submit);

    view! {
        <Dialog open=open title=t("version_component_title") subtitle=t("version_component_description") icon="swap_horiz">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="download"
                    disabled=Signal::derive(move || !ready())
                    on_click=move |_| submit.with_value(|s| s())
                >
                    {move || i18n.t("version_component_install_action")}
                </Button>
            </DialogFooter>
            <Field label=t("version_component_loader_label")>
                <Select options=Signal::derive(loader_choices) value=loader />
            </Field>
            <Field label=t("version_component_minecraft_label")>
                <Select options=mc_options value=mc on_change=on_mc />
            </Field>
            <Show when=move || loader.get() != "minecraft">
                <Field label=t("minecraft_components_loader_build_label")>
                    <Select options=build_options value=build />
                </Field>
            </Show>
            <Checkbox checked=unstable label=t("version_component_include_unstable") />
            <div class="hint">{status}</div>
        </Dialog>
    }
}

#[derive(serde::Serialize)]
struct KeyArgs {
    key: String,
}

#[derive(serde::Serialize)]
struct SaveArgs {
    key: String,
    update: BuildSettingsUpdate,
}

/// The settings' sections: id, icon, label.
pub const SECTIONS: [(&str, &str, &str); 3] = [
    ("general", "tune", "version_section_general"),
    ("runtime", "coffee", "version_section_runtime"),
    ("arguments", "terminal", "version_section_arguments"),
];

#[component]
pub fn BuildSettingsPanel(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let header = use_header();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let key = StoredValue::new(key);
    let key = move || key.get_value();
    let tab = RwSignal::new("general");
    let loaded = RwSignal::new(None::<Result<BuildSettingsDto, String>>);
    let form = SettingsForm::new();
    let SettingsForm {
        name,
        component,
        java,
        gpu,
        auto_ram,
        ram,
        arguments,
        preset,
        host,
        port,
        image_path,
        remove_image,
        profile,
    } = form;
    let name_error = RwSignal::new(None::<String>);
    let port_error = RwSignal::new(None::<String>);
    let icon_error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let change_open = RwSignal::new(false);
    let memory = RwSignal::new(None::<MemoryInfo>);
    let javas = RwSignal::new(JavaList::default());
    let components = RwSignal::new(Vec::<ComponentDto>::new());

    // Saved settings on the page; `try_set` because answers may come after the page is gone.
    let fill = move |s: &BuildSettingsDto| {
        form.fill(s);
        let _ = icon_error.try_set(None);
        let _ = name_error.try_set(None);
        let _ = port_error.try_set(None);
        let _ = header.subtitle.try_set(Some(s.name.clone()));
        let _ = loaded.try_set(Some(Ok(s.clone())));
    };
    let load = move || {
        let key = key();
        spawn_local(async move {
            match ipc::invoke::<_, BuildSettingsDto>("build_settings_get", &KeyArgs { key }).await {
                Ok(settings) => fill(&settings),
                Err(e) => {
                    let _ = loaded.try_set(Some(Err(i18n.error(&e))));
                }
            }
        });
    };
    load();
    spawn_local(async move {
        if let Ok(info) = ipc::call::<MemoryInfo>("memory_info").await {
            let _ = memory.try_set(Some(info));
            if auto_ram.try_get_untracked().unwrap_or(false) {
                let _ = ram.try_set(ram_slider(&info).2);
            }
        }
        if let Ok(list) = ipc::call::<JavaList>("java_list").await {
            let _ = javas.try_set(list);
        }
        if let Ok(snapshot) = ipc::call::<ComponentsSnapshot>("components_list").await {
            let _ = components.try_set(snapshot.components);
        }
    });

    let save = move || {
        if !port_ok(&port.get_untracked()) {
            port_error.set(Some(i18n.t("invalid_port")));
            tab.set("general");
            return;
        }
        let update = form.update();
        saving.set(true);
        let key = key();
        spawn_local(async move {
            match ipc::invoke::<_, BuildSettingsDto>("build_settings_save", &SaveArgs { key, update }).await {
                Ok(settings) => fill(&settings),
                Err(e) if matches!(e.code, ErrorCode::VersionNameEmpty | ErrorCode::VersionExists) => {
                    let _ = name_error.try_set(Some(i18n.error(&e)));
                    let _ = tab.try_set("general");
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
            let _ = saving.try_set(false);
        });
    };
    let pick_icon = move || {
        spawn_local(async move {
            match icon_pick(ipc::call::<Option<String>>("pick_image_file").await) {
                IconPick::Use(path) => {
                    let _ = image_path.try_set(Some(path));
                    let _ = remove_image.try_set(false);
                    let _ = icon_error.try_set(None);
                }
                IconPick::Cancelled => {}
                IconPick::Invalid => {
                    let _ = image_path.try_set(None);
                    let _ = icon_error.try_set(Some(i18n.t("version_icon_invalid")));
                }
            }
        });
    };

    let java_options = Signal::derive(move || {
        let saved =
            loaded.with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).and_then(|s| s.java_path.clone()));
        javas.with(|list| {
            java_choices(
                list,
                saved.as_deref(),
                &i18n.t("java_launcher_default"),
                &i18n.t("java_saved_custom_path"),
            )
        })
    });
    let component_options = Signal::derive(move || {
        let current = component.get();
        components.with(|list| component_choices(list, Some(&current)))
    });
    let effective_java = move || {
        let chosen = java.get();
        if chosen != AUTO_JAVA {
            return chosen;
        }
        loaded
            .with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).and_then(|s| s.auto_java.clone()))
            .unwrap_or_else(|| i18n.t("java_path_auto_pending"))
    };
    let gpu_options = Signal::derive(move || {
        vec![
            SegOption::new("auto", i18n.t("gpu_mode_auto")),
            SegOption::new("igpu", i18n.t("gpu_mode_integrated")),
            SegOption::new("dgpu", i18n.t("gpu_mode_discrete")),
        ]
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
    let preset_options = Signal::derive(move || {
        vec![
            SelectOption::new("keep", i18n.t("jvm_preset_keep")),
            SelectOption::new("none", i18n.t("jvm_preset_none")),
            SelectOption::new("performance", i18n.t("jvm_preset_performance")),
        ]
    });
    let icon_src = move || {
        let current = loaded.with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).and_then(|s| s.image.clone()));
        card_image(if remove_image.get() { None } else { current.as_deref() })
    };
    let icon_label = move || {
        image_path
            .get()
            .map(|p| p.rsplit(['\\', '/']).next().unwrap_or_default().to_string())
            .unwrap_or_else(|| i18n.t("select_icon"))
    };

    let account_options = Signal::derive(move || {
        let kind = |kind: AccountKind| match kind {
            AccountKind::Microsoft => i18n.t("microsoft"),
            AccountKind::Offline => i18n.t("offline_account"),
        };
        store.profiles.with(|profiles| {
            profile.with(|current| {
                account_choices(
                    profiles,
                    current,
                    &i18n.t("build_account_launcher"),
                    kind,
                    &i18n.t("build_account_gone"),
                )
            })
        })
    });
    let general = move || {
        view! {
            <Section icon="tune" title=t("version_section_general") desc=t("version_section_general_desc")>
                <Field label=t("version_name_label") error=Signal::derive(move || name_error.get())>
                    <TextInput value=name invalid=Signal::derive(move || name_error.get().is_some()) />
                </Field>
                <Field label=t("loaders_label")>
                    <div class="wrap">
                        <div class="grow">
                            <Select options=component_options value=component />
                        </div>
                        <Button icon="swap_horiz" on_click=move |_| change_open.set(true)>
                            {move || i18n.t("version_component_change_action")}
                        </Button>
                    </div>
                </Field>
                <Field label=t("build_account_label") hint=t("build_account_hint")>
                    <Select options=account_options value=profile />
                </Field>
            </Section>
            <Section icon="image" title=t("version_section_appearance") desc=t("version_section_appearance_desc")>
                <div class="build-settings__icon">
                    <img src=icon_src alt="" />
                    <div class="wrap">
                        <Button icon="upload_file" on_click=move |_| pick_icon()>{icon_label}</Button>
                        <Button
                            variant=Variant::Ghost
                            icon="delete_outline"
                            on_click=move |_| {
                                remove_image.set(true);
                                image_path.set(None);
                                icon_error.set(None);
                            }
                        >
                            {move || i18n.t("remove_icon")}
                        </Button>
                    </div>
                </div>
                {move || match icon_error.get() {
                    Some(e) => view! { <div class="error"><Icon name="error_outline" />{e}</div> }.into_any(),
                    None => view! { <div class="hint">{i18n.t("version_icon_hint")}</div> }.into_any(),
                }}
            </Section>
            <Section icon="dns" title=t("version_server_section") desc=t("version_server_section_desc")>
                <div class="build-settings__server">
                    <Field label=t("server_host_label")>
                        <TextInput value=host />
                    </Field>
                    <Field label=t("server_port_label") error=Signal::derive(move || port_error.get())>
                        <TextInput value=port invalid=Signal::derive(move || port_error.get().is_some()) />
                    </Field>
                </div>
            </Section>
        }
    };
    let runtime = move || {
        view! {
            <Section icon="coffee" title=t("version_section_runtime") desc=t("version_section_runtime_desc")>
                <SettingRow title=t("java_label") desc=t("java_selection_hint") stacked=true>
                    <Select options=java_options value=java />
                    <div class="hint">{move || format!("{}: {}", i18n.t("java_path_label"), effective_java())}</div>
                </SettingRow>
                <SettingRow title=t("gpu_mode_label")>
                    <Segmented options=gpu_options value=gpu />
                </SettingRow>
                <RamSetting
                    title=t("max_ram_label")
                    auto_label=Signal::derive(auto_label)
                    auto=auto_ram
                    ram=ram
                    memory=memory
                />
            </Section>
        }
    };
    let jvm = move || {
        view! {
            <Section icon="terminal" title=t("version_section_arguments") desc=t("version_section_arguments_desc")>
                <Field label=t("recommended_parameters_label")>
                    <Select
                        options=preset_options
                        value=preset
                        on_change=Callback::new(move |p: String| arguments.set(apply_preset(&p, &arguments.get_untracked())))
                    />
                </Field>
                <Field label=t("custom_jvm_arguments_label")>
                    <CodeEditor value=arguments placeholder=t("custom_jvm_arguments_hint") />
                </Field>
            </Section>
        }
    };

    let nav: Vec<NavEntry> =
        SECTIONS.iter().map(|&(id, icon, label)| NavEntry::Item { id, icon, label: t(label) }).collect();

    view! {
        // Like the launcher's settings: the sections in a side menu, Save under it.
        <div class="settings build-settings">
            <div class="build-settings__side">
                <SettingsNav entries=nav active=tab />
                <Button
                    size=Size::Md
                    variant=Variant::Primary
                    icon="save"
                    class="build-settings__save"
                    loading=Signal::derive(move || saving.get())
                    on_click=move |_| save()
                >
                    {move || i18n.t("save")}
                </Button>
            </div>
            <div class="settings__body">
                {move || match loaded.get() {
                    None => view! { <div class="hint">{move || i18n.t("loading_more")}</div> }.into_any(),
                    Some(Err(message)) => view! { <div class="hint">{message}</div> }.into_any(),
                    Some(Ok(_)) => match tab.get() {
                        "runtime" => runtime().into_any(),
                        "arguments" => jvm().into_any(),
                        _ => general().into_any(),
                    },
                }}
            </div>
        </div>
        <ChangeComponentDialog
            open=change_open
            build_key=key()
            on_done=Callback::new(move |settings: BuildSettingsDto| {
                form.take_component(&settings);
                let _ = loaded.try_set(Some(Ok(settings)));
                spawn_local(async move {
                    if let Ok(snapshot) = ipc::call::<ComponentsSnapshot>("components_list").await {
                        let _ = components.try_set(snapshot.components);
                    }
                });
            })
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::JavaEntry;

    #[test]
    fn build_settings_sections_follow_the_original() {
        let ids: Vec<&str> = SECTIONS.iter().map(|(id, ..)| *id).collect();
        assert_eq!(ids, ["general", "runtime", "arguments"]);
        assert_eq!(SECTIONS[1].2, "version_section_runtime");
    }

    #[test]
    fn java_choices_start_automatic_and_keep_a_saved_path() {
        let list = JavaList {
            launcher: vec![JavaEntry { label: "Java 21".into(), path: "C:/jdk21/bin/javaw.exe".into() }],
            custom: vec![
                JavaEntry { label: "Mine".into(), path: "C:/mine/bin/java.exe".into() },
                JavaEntry { label: "Java 21".into(), path: "C:/jdk21/bin/javaw.exe".into() },
            ],
        };
        let options = java_choices(&list, Some("D:/old/java.exe"), "Auto", "Saved");
        let values: Vec<&str> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, [AUTO_JAVA, "C:/mine/bin/java.exe", "C:/jdk21/bin/javaw.exe", "D:/old/java.exe"]);
        assert_eq!(options[1].label, "Mine (java.exe)");
        assert_eq!(options[3].label, "Saved: D:/old/java.exe");
        assert_eq!(
            java_choices(&list, Some("C:/mine/bin/java.exe"), "Auto", "Saved").len(),
            3,
            "a listed path is not repeated"
        );
    }

    #[test]
    fn components_are_named_like_the_catalog() {
        let component =
            |id: &str, loader: Option<LoaderKind>, mc: Option<&str>, lv: Option<&str>| ComponentDto {
                id: id.into(),
                loader,
                minecraft: mc.map(str::to_string),
                loader_version: lv.map(str::to_string),
                size: 0,
                modified_ms: None,
                used_by: Vec::new(),
                base_for: Vec::new(),
            };
        let list = [
            component("1.21.1", Some(LoaderKind::Minecraft), Some("1.21.1"), None),
            component(
                "fabric-loader-0.16.9-1.21.1",
                Some(LoaderKind::Fabric),
                Some("1.21.1"),
                Some("0.16.9"),
            ),
            component("custom", None, Some("1.21.1"), None),
        ];
        let labels: Vec<String> =
            component_choices(&list, Some("1.21.1")).into_iter().map(|o| o.label).collect();
        assert_eq!(labels, ["Minecraft 1.21.1", "Fabric 1.21.1 (0.16.9)", "custom"]);
        let gone = component_choices(&list, Some("neoforge-21.1.77"));
        assert_eq!(
            (gone[0].value.as_str(), gone.len()),
            ("neoforge-21.1.77", 4),
            "the build's own component stays listed"
        );
    }

    #[test]
    fn presets_ports_and_argument_lines() {
        assert_eq!(apply_preset("keep", "-Dx=1"), "-Dx=1");
        assert_eq!(apply_preset("none", "-Dx=1"), "");
        assert_eq!(
            apply_preset("performance", ""),
            launcher_shared::args::DEFAULT_GC_ARGUMENTS.join("\n"),
            "the official launcher's G1, as builds get it by default"
        );
        for ok in ["", " ", "25565", "1", "65535"] {
            assert!(port_ok(ok), "{ok:?}");
        }
        for bad in ["0", "65536", "abc", "-1"] {
            assert!(!port_ok(bad), "{bad:?}");
        }
        assert_eq!(arguments_of("-Dx=1\n\n  -XX:+UseG1GC  \n"), ["-Dx=1", "-XX:+UseG1GC"]);
    }

    #[test]
    fn the_change_dialog_offers_loaders_versions_and_builds() {
        let values: Vec<String> = loader_choices().into_iter().map(|o| o.value).collect();
        assert_eq!(values, ["minecraft", "fabric", "quilt", "forge", "neoforge"]);
        let rows = crate::builds::catalog::rows_from_minecraft(vec![
            launcher_shared::CatalogVersion {
                id: "1.21.1".into(),
                kind: "release".into(),
                release_time: None,
            },
            launcher_shared::CatalogVersion {
                id: "24w33a".into(),
                kind: "snapshot".into(),
                release_time: None,
            },
        ]);
        let labels: Vec<String> = minecraft_choices(&rows, "Снапшот").into_iter().map(|o| o.label).collect();
        assert_eq!(labels, ["1.21.1", "24w33a (Снапшот)"]);
        let args = ChangeArgs {
            key: "aero".into(),
            loader: LoaderKind::Fabric,
            mc: "1.21.1".into(),
            loader_version: Some("0.16.9".into()),
        };
        assert_eq!(
            serde_json::to_value(args).unwrap(),
            serde_json::json!({"key": "aero", "loader": "fabric", "mc": "1.21.1", "loaderVersion": "0.16.9"})
        );
    }

    #[test]
    fn a_build_may_take_any_account_and_shows_one_that_is_gone() {
        let account = |key: &str, kind| ProfileDto {
            key: key.into(),
            name: key.into(),
            id: String::new(),
            kind,
            is_default: false,
            reauth_required: false,
            reauth_reason: None,
        };
        let profiles = [account("Alex", AccountKind::Microsoft), account("Steve", AccountKind::Offline)];
        let kind = |k: AccountKind| format!("{k:?}");
        let rows = |current: &str| -> Vec<(String, String)> {
            account_choices(&profiles, current, "Launcher", kind, "gone")
                .into_iter()
                .map(|o| (o.value, o.label))
                .collect()
        };
        let pair = |v: &str, l: &str| (v.to_string(), l.to_string());
        let usual =
            vec![pair("", "Launcher"), pair("Alex", "Alex (Microsoft)"), pair("Steve", "Steve (Offline)")];
        assert_eq!(rows(""), usual);
        assert_eq!(rows("Alex"), usual);
        assert_eq!(rows("Bob").last(), Some(&pair("Bob", "Bob (gone)")));
        let form = SettingsForm::new();
        form.fill(&BuildSettingsDto { profile: Some("Alex".into()), ..saved("1.21.1", None) });
        assert_eq!(form.update().profile.as_deref(), Some("Alex"));
        form.profile.set(String::new());
        assert_eq!(form.update().profile, None, "the launcher's choice");
    }

    fn saved(component: &str, java: Option<&str>) -> BuildSettingsDto {
        BuildSettingsDto {
            key: "aero".into(),
            name: "Aero".into(),
            image: None,
            component: Some(component.into()),
            java_path: java.map(str::to_string),
            auto_java: None,
            gpu_mode: "dgpu".into(),
            max_ram_gb: None,
            jvm_arguments: Vec::new(),
            server_host: String::new(),
            server_port: None,
            profile: None,
        }
    }

    #[test]
    fn installing_a_component_keeps_unsaved_edits() {
        let owner = Owner::new();
        owner.with(|| {
            let form = SettingsForm::new();
            form.fill(&saved("1.21.1", Some("C:/jdk8/bin/java.exe")));
            assert_eq!(form.update().java_path.as_deref(), Some("C:/jdk8/bin/java.exe"));
            assert_eq!(form.update().max_ram_gb, None, "automatic memory sends no limit");
            form.name.set("Renamed".into());
            form.arguments.set("-Dx=1".into());
            form.auto_ram.set(false);
            form.ram.set(4.0);
            form.take_component(&saved("fabric-loader-0.16.9-1.21.1", None));
            let update = form.update();
            assert_eq!((update.name.as_str(), update.max_ram_gb), ("Renamed", Some(4)));
            assert_eq!(update.jvm_arguments, ["-Dx=1"]);
            assert_eq!(update.component.as_deref(), Some("fabric-loader-0.16.9-1.21.1"));
            assert_eq!(update.java_path, None, "the new component's own Java");
        });
    }

    #[test]
    fn a_saved_word_with_blanks_comes_back_as_it_was() {
        let owner = Owner::new();
        owner.with(|| {
            let form = SettingsForm::new();
            let mut settings = saved("1.21.1", None);
            settings.jvm_arguments =
                vec!["-Dpath=C:/My Games".into(), "--add-opens".into(), "a/b=ALL-UNNAMED".into()];
            form.fill(&settings);
            assert_eq!(
                form.arguments.get_untracked(),
                "\"-Dpath=C:/My Games\"
--add-opens
a/b=ALL-UNNAMED"
            );
        });
    }

    #[test]
    fn the_change_dialog_starts_from_the_build() {
        assert_eq!(current_loader(Some("Fabric"), Some("fabric-loader-0.16.9-1.21.1")), "fabric");
        assert_eq!(current_loader(Some("NeoForge"), Some("neoforge-21.1.77")), "neoforge");
        assert_eq!(current_loader(Some("Forge"), Some("1.20.1-forge-47.4.10")), "forge");
        assert_eq!(current_loader(Some("Quilt"), None), "quilt");
        assert_eq!(current_loader(Some("Minecraft"), Some("1.21.1")), "minecraft");
        assert_eq!(current_loader(None, None), "minecraft");
        let build =
            |version: &str, stable: bool| launcher_shared::LoaderBuild { version: version.into(), stable };
        let rows = crate::builds::catalog::rows_from_loader(
            LoaderKind::Fabric,
            vec![
                launcher_shared::LoaderOption {
                    mc: "1.21.4".into(),
                    snapshot: false,
                    builds: vec![build("0.16.10", true)],
                    default_version: "0.16.10".into(),
                },
                launcher_shared::LoaderOption {
                    mc: "1.20.1".into(),
                    snapshot: false,
                    builds: vec![build("0.16.10", true), build("0.16.9", true)],
                    default_version: "0.16.10".into(),
                },
            ],
        );
        assert_eq!(preferred_mc(&rows, Some("1.20.1")), "1.20.1", "the build's own Minecraft version");
        assert_eq!(preferred_mc(&rows, Some("1.12.2")), "1.21.4", "else the newest");
        assert_eq!(preferred_mc(&[], Some("1.20.1")), "");
        assert_eq!(preferred_build(rows.get(1), Some("0.16.9")), "0.16.9", "the build's own loader build");
        assert_eq!(preferred_build(rows.get(1), Some("9.9.9")), "0.16.10", "else the default");
        assert_eq!(preferred_build(None, Some("0.16.9")), "");
    }

    #[test]
    fn a_picked_icon_is_checked_before_saving() {
        assert_eq!(icon_pick(Ok(Some("C:/icon.png".into()))), IconPick::Use("C:/icon.png".into()));
        assert_eq!(icon_pick(Ok(None)), IconPick::Cancelled);
        let bad = AppError::new(ErrorCode::InvalidInput, "not an image").with_param("error", "not an image");
        assert_eq!(icon_pick(Err(bad)), IconPick::Invalid);
    }
}
