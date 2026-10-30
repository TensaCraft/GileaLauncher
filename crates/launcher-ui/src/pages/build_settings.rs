//! Build settings, a tab of the build's content page: General (name, loader,
//! icon, quick-join server), Runtime (Java, GPU, memory) and JVM arguments, saved together.

use launcher_shared::args;
use launcher_shared::{
    AccountKind, AppError, BuildDto, BuildSettingsDto, BuildSettingsUpdate, CatalogVersion, ErrorCode,
    JavaList, Level, LoaderKind, MemoryInfo, ProfileDto,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Checkbox, CodeEditor, ConfirmDialog, Dialog, DialogFooter, Field, Icon, NavEntry, Section,
    SegOption, Segmented, Select, SelectOption, SettingRow, SettingsNav, Size, TextInput, Variant,
    card_image, ipc, use_toasts,
};

use crate::builds::description::{DescriptionField, description_problem};
use crate::builds::gpu::GpuChoice;
use crate::builds::loader_pick::{
    LoaderBuildField, LoaderCards, LoaderPick, change_warning, loader_mark, loader_value,
};
use crate::builds::ram::RamSetting;
use crate::builds::{build_loader, build_minecraft, ram_slider};
use crate::pages::settings::java::gpu_choice;
use crate::pages::wizard::state::loader_available;
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
    pub description: RwSignal<String>,
    /// `SCREEN_LAUNCHER`, `SCREEN_FULL` or `SCREEN_WINDOW`.
    pub fullscreen: RwSignal<String>,
}

/// The game's screen as the launcher's settings say.
pub const SCREEN_LAUNCHER: &str = "launcher";
/// The game full screen, whatever the launcher's settings say.
pub const SCREEN_FULL: &str = "full";
/// The game in a window, whatever the launcher's settings say.
pub const SCREEN_WINDOW: &str = "window";

impl Default for SettingsForm {
    fn default() -> SettingsForm {
        SettingsForm::new()
    }
}

impl SettingsForm {
    pub fn new() -> SettingsForm {
        SettingsForm {
            name: RwSignal::new(String::new()),
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
            description: RwSignal::new(String::new()),
            fullscreen: RwSignal::new(SCREEN_LAUNCHER.to_string()),
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
        let _ = self.description.try_set(s.description.clone());
        let screen = match s.fullscreen {
            None => SCREEN_LAUNCHER,
            Some(true) => SCREEN_FULL,
            Some(false) => SCREEN_WINDOW,
        };
        let _ = self.fullscreen.try_set(screen.to_string());
    }

    /// After a change of loader: the Java the install gave the build; everything else keeps what
    /// the user typed.
    pub fn take_component(&self, s: &BuildSettingsDto) {
        let _ = self.java.try_set(s.java_path.clone().unwrap_or_else(|| AUTO_JAVA.to_string()));
    }

    /// What "Save" sends; automatic memory sends no limit. The loader is not among it: it changes
    /// by its own operation only, so a page opened before never puts the old one back.
    pub fn update(&self) -> BuildSettingsUpdate {
        BuildSettingsUpdate {
            name: self.name.get_untracked(),
            component: None,
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
            description: Some(self.description.get_untracked()),
            fullscreen: match self.fullscreen.get_untracked().as_str() {
                SCREEN_FULL => Some(true),
                SCREEN_WINDOW => Some(false),
                _ => None,
            },
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

/// The settings' line of a build's loader: "Fabric 0.16.9" (`vanilla`, the language's words, for
/// plain Minecraft) and "Minecraft 1.21.1" under it.
pub fn loader_line(build: &BuildDto, vanilla: &str) -> (String, String) {
    let lv = build.loader_version.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let name = match (build_loader(build), lv) {
        (LoaderKind::Minecraft, _) => vanilla.to_string(),
        (kind, Some(lv)) => format!("{} {lv}", kind.display_name()),
        (kind, None) => kind.display_name().to_string(),
    };
    let mc = build_minecraft(build);
    (name, if mc.is_empty() { mc } else { format!("Minecraft {mc}") })
}

/// Minecraft's versions for the dialog (`snapshot`, the language's word, marks snapshots), the
/// build's own first when the list does not have it.
pub fn minecraft_options(versions: &[CatalogVersion], current: &str, snapshot: &str) -> Vec<SelectOption> {
    let mut options: Vec<SelectOption> = versions
        .iter()
        .map(|v| {
            let label = if v.kind == "snapshot" { format!("{} ({snapshot})", v.id) } else { v.id.clone() };
            SelectOption::new(v.id.clone(), label)
        })
        .collect();
    if !current.is_empty() && !options.iter().any(|o| o.value == current) {
        options.insert(0, SelectOption::new(current, current));
    }
    options
}

/// The pick is what the build has: installing it would change nothing.
pub fn same_pick(build: &BuildDto, kind: LoaderKind, mc: &str, loader_version: &str) -> bool {
    build_loader(build) == kind
        && build_minecraft(build) == mc
        && (kind == LoaderKind::Minecraft || build.loader_version.as_deref() == Some(loader_version))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeArgs {
    key: String,
    loader: LoaderKind,
    mc: String,
    loader_version: Option<String>,
}

#[derive(serde::Serialize)]
struct CatalogArgs {
    snapshots: bool,
}

/// "Change the loader": the wizard's cards, the loader's build and the Minecraft version, starting
/// from the build's own. A change that leaves the build's mods behind asks first; "Install" runs
/// as an operation and `on_done` gets the new settings.
#[component]
fn LoaderDialog(
    open: RwSignal<bool>,
    build_key: String,
    on_done: Callback<BuildSettingsDto>,
) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let current = {
        let key = build_key.clone();
        Memo::new(move |_| store.builds.with(|b| b.iter().find(|b| b.key == key).cloned()))
    };
    let pick = LoaderPick::new(LoaderKind::Minecraft);
    let mc = RwSignal::new(String::new());
    let snapshots = RwSignal::new(false);
    let versions = RwSignal::new(None::<Result<Vec<CatalogVersion>, String>>);
    let confirm = RwSignal::new(false);
    let busy = Memo::new(move |_| store.ops.with(|o| o.busy));
    let load_versions = move || {
        let _ = versions.try_set(None);
        let snapshots = snapshots.get_untracked();
        spawn_local(async move {
            let answer =
                ipc::invoke::<_, Vec<CatalogVersion>>("catalog_minecraft", &CatalogArgs { snapshots }).await;
            let _ = versions.try_set(Some(answer.map_err(|e| i18n.error(&e))));
        });
    };
    // Opened: the build's own loader, its build and its Minecraft; the lists asked afresh.
    Effect::new(move |was_open: Option<bool>| {
        let is_open = open.get();
        if is_open && was_open != Some(true) {
            if let Some(build) = current.get_untracked() {
                pick.loader.set(loader_value(build_loader(&build)));
                pick.build.set(build.loader_version.clone().unwrap_or_default());
                mc.set(build_minecraft(&build));
            }
            pick.load();
            load_versions();
        }
        is_open
    });
    let mc_options = Signal::derive(move || {
        let (chosen, snapshot) = (mc.get(), i18n.t("version_create_snapshot_badge"));
        versions.with(|v| match v {
            Some(Ok(list)) => minecraft_options(list, &chosen, &snapshot),
            _ => minecraft_options(&[], &chosen, &snapshot),
        })
    });
    // What the change risks for the build's mods, in the language.
    let warning = Memo::new(move |_| {
        let from = current.with(|b| b.as_ref().map(build_loader))?;
        change_warning(from, pick.kind()).map(|(key, words)| i18n.tp(key, &words))
    });
    let ready = move || {
        let (kind, chosen, build) = (pick.kind(), mc.get(), pick.build.get());
        let available = pick.catalogs.with(|all| loader_available(kind, &chosen, all));
        let changes = current.with(|b| b.as_ref().is_some_and(|b| !same_pick(b, kind, &chosen, &build)));
        !chosen.is_empty() && available && (kind == LoaderKind::Minecraft || !build.is_empty()) && changes
    };
    let submit = Callback::new(move |()| {
        let kind = pick.kind();
        let args = ChangeArgs {
            key: build_key.clone(),
            loader: kind,
            mc: mc.get_untracked(),
            loader_version: (kind != LoaderKind::Minecraft).then(|| pick.build.get_untracked()),
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
    });
    let install = move || {
        if warning.with_untracked(Option::is_some) {
            confirm.set(true);
        } else {
            submit.run(());
        }
    };

    view! {
        <Dialog
            open=open
            title=t("build_loader_dialog_title")
            subtitle=t("build_loader_dialog_desc")
            icon="swap_horiz"
            class="dialog--loader"
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="download"
                    disabled=Signal::derive(move || busy.get() || !ready())
                    on_click=move |_| install()
                >
                    {move || i18n.t("minecraft_components_install_action")}
                </Button>
            </DialogFooter>
            <LoaderCards pick=pick mc=mc vanilla=true />
            <div class="build-loader__fields">
                <Field label=t("version_component_minecraft_label")>
                    <div class="wrap">
                        <div class="grow"><Select options=mc_options value=mc /></div>
                        <Checkbox
                            checked=snapshots
                            label=t("wizard_snapshots")
                            on_change=Callback::new(move |_: bool| load_versions())
                        />
                    </div>
                </Field>
                <LoaderBuildField pick=pick mc=mc />
            </div>
            {move || versions.with(|v| match v {
                Some(Err(e)) => Some(view! {
                    <div class="wizard__status is-error">
                        <Icon name="error_outline" />
                        <span class="grow">{e.clone()}</span>
                    </div>
                }),
                _ => None,
            })}
            {move || warning.get().map(|text| view! {
                <div class="wizard__status is-warn">
                    <Icon name="warning_amber" />
                    <span class="grow">{text}</span>
                </div>
            })}
        </Dialog>
        <ConfirmDialog
            open=confirm
            title=t("build_loader_confirm_title")
            message=Signal::derive(move || warning.get().unwrap_or_default())
            confirm_label=t("build_loader_confirm")
            cancel_label=t("cancel")
            on_confirm=submit
        />
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
        description,
        fullscreen,
    } = form;
    let screen_options = Signal::derive(move || {
        vec![
            SegOption::new(SCREEN_LAUNCHER, i18n.t("screen_mode_launcher")),
            SegOption::new(SCREEN_FULL, i18n.t("screen_mode_full")),
            SegOption::new(SCREEN_WINDOW, i18n.t("screen_mode_window")),
        ]
    });
    let name_error = RwSignal::new(None::<String>);
    let port_error = RwSignal::new(None::<String>);
    let icon_error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let change_open = RwSignal::new(false);
    let memory = RwSignal::new(None::<MemoryInfo>);
    let javas = RwSignal::new(JavaList::default());
    let build = Memo::new(move |_| store.builds.with(|b| b.iter().find(|b| b.key == key()).cloned()));

    // Saved settings on the page. An answer after the page is gone changes nothing: the header is
    // the app's, and another page shows it by then.
    let fill = move |s: &BuildSettingsDto| {
        if loaded.try_set(Some(Ok(s.clone()))).is_some() {
            return;
        }
        form.fill(s);
        let _ = icon_error.try_set(None);
        let _ = name_error.try_set(None);
        let _ = port_error.try_set(None);
        let _ = header.subtitle.try_set(Some(s.name.clone()));
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
    });
    // A new loader (from this page's dialog, the build's mods, a modpack's update): the page takes
    // the Java its install gave, and keeps the rest.
    let take_loader = move |settings: BuildSettingsDto| {
        form.take_component(&settings);
        let _ = loaded.try_set(Some(Ok(settings)));
    };
    Effect::new(move |seen: Option<Option<String>>| {
        let component = build.with(|b| b.as_ref().and_then(|b| b.loader.clone()));
        if seen.is_some_and(|before| before != component) {
            let key = key();
            spawn_local(async move {
                if let Ok(settings) =
                    ipc::invoke::<_, BuildSettingsDto>("build_settings_get", &KeyArgs { key }).await
                {
                    take_loader(settings);
                }
            });
        }
        component
    });

    let save = move || {
        if !port_ok(&port.get_untracked()) {
            port_error.set(Some(i18n.t("invalid_port")));
            tab.set("general");
            return;
        }
        // The backend would cut it; its field says why it is not saved.
        if description.with_untracked(|text| description_problem(text)).is_some() {
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
    let effective_java = move || {
        let chosen = java.get();
        if chosen != AUTO_JAVA {
            return chosen;
        }
        loaded
            .with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).and_then(|s| s.auto_java.clone()))
            .unwrap_or_else(|| i18n.t("java_path_auto_pending"))
    };
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
                <DescriptionField value=description />
                <Field label=t("build_loader_label")>
                    {move || build.get().map(|b| {
                        let (name, sub) = loader_line(&b, &i18n.t("build_loader_vanilla"));
                        view! {
                            <div class="list-row build-row build-loader">
                                <div class="build-row__icon"><Icon name=loader_mark(build_loader(&b)) /></div>
                                <div class="build-row__text">
                                    <div class="build-row__name">{name}</div>
                                    <div class="build-row__sub">{sub}</div>
                                </div>
                                <Button icon="swap_horiz" on_click=move |_| change_open.set(true)>
                                    {move || i18n.t("build_loader_change")}
                                </Button>
                            </div>
                        }
                    })}
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
                    <GpuChoice value=gpu />
                </SettingRow>
                <SettingRow title=t("screen_mode_label") desc=t("screen_mode_desc")>
                    <Segmented options=screen_options value=fullscreen />
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
        <LoaderDialog open=change_open build_key=key() on_done=Callback::new(take_loader) />
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::{CatalogVersion, JavaEntry};

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

    fn release(id: &str, kind: &str) -> CatalogVersion {
        CatalogVersion { id: id.into(), kind: kind.into(), release_time: None }
    }

    #[test]
    fn the_dialog_lists_minecraft_with_the_build_s_own() {
        let versions = [release("1.21.1", "release"), release("24w33a", "snapshot")];
        let shown = |current: &str| -> Vec<(String, String)> {
            minecraft_options(&versions, current, "Снапшот").into_iter().map(|o| (o.value, o.label)).collect()
        };
        let pair = |v: &str, l: &str| (v.to_string(), l.to_string());
        assert_eq!(shown("1.21.1"), [pair("1.21.1", "1.21.1"), pair("24w33a", "24w33a (Снапшот)")]);
        assert_eq!(shown("1.12.2")[0], pair("1.12.2", "1.12.2"), "the build's own, not listed");
        assert_eq!(shown("1.12.2").len(), 3);
        assert!(minecraft_options(&[], "", "Снапшот").is_empty());
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

    fn build_of(client: &str, component: &str, lv: Option<&str>) -> BuildDto {
        BuildDto {
            key: "aero".into(),
            version_id: "aero".into(),
            name: "Aero".into(),
            version: Some("1.21.1".into()),
            loader: Some(component.into()),
            client: Some(client.into()),
            loader_version: lv.map(str::to_string),
            game_dir: String::new(),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
        }
    }

    #[test]
    fn the_loader_row_names_the_loader_its_version_and_minecraft() {
        let line = |b: &BuildDto| loader_line(b, "Ваніль");
        let pair = |name: &str, sub: &str| (name.to_string(), sub.to_string());
        let fabric = build_of("Fabric", "fabric-loader-0.16.9-1.21.1", Some("0.16.9"));
        assert_eq!(line(&fabric), pair("Fabric 0.16.9", "Minecraft 1.21.1"));
        assert_eq!(line(&build_of("Minecraft", "1.21.1", None)), pair("Ваніль", "Minecraft 1.21.1"));
        assert_eq!(
            line(&build_of("TensaCraft", "neoforge-21.1.77", Some("21.1.77"))).0,
            "NeoForge 21.1.77",
            "a server's build names the loader it runs"
        );
        assert_eq!(line(&build_of("Forge", "1.20.1-forge-47.4.10", None)).0, "Forge");
    }

    #[test]
    fn installing_what_the_build_has_changes_nothing() {
        let fabric = build_of("Fabric", "fabric-loader-0.16.9-1.21.1", Some("0.16.9"));
        assert!(same_pick(&fabric, LoaderKind::Fabric, "1.21.1", "0.16.9"));
        assert!(!same_pick(&fabric, LoaderKind::Fabric, "1.21.1", "0.16.10"));
        assert!(!same_pick(&fabric, LoaderKind::Fabric, "1.21.4", "0.16.9"));
        assert!(!same_pick(&fabric, LoaderKind::Minecraft, "1.21.1", ""));
        let vanilla = build_of("Minecraft", "1.21.1", None);
        assert!(same_pick(&vanilla, LoaderKind::Minecraft, "1.21.1", "0.16.9"), "vanilla has no build");
        assert!(!same_pick(&vanilla, LoaderKind::Quilt, "1.21.1", "0.27.1"));
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
            description: String::new(),
            fullscreen: None,
        }
    }

    #[test]
    fn the_screen_mode_follows_the_launcher_unless_the_build_chose() {
        let owner = Owner::new();
        owner.with(|| {
            let form = SettingsForm::new();
            form.fill(&saved("1.21.1", None));
            assert_eq!(form.fullscreen.get_untracked(), SCREEN_LAUNCHER);
            assert_eq!(form.update().fullscreen, None);
            form.fullscreen.set(SCREEN_FULL.into());
            assert_eq!(form.update().fullscreen, Some(true));
            form.fullscreen.set(SCREEN_WINDOW.into());
            assert_eq!(form.update().fullscreen, Some(false));
            form.fill(&BuildSettingsDto { fullscreen: Some(true), ..saved("1.21.1", None) });
            assert_eq!(form.fullscreen.get_untracked(), SCREEN_FULL);
        });
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
            assert_eq!(update.component, None, "the loader changes by its own operation, never by Save");
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
    fn a_picked_icon_is_checked_before_saving() {
        assert_eq!(icon_pick(Ok(Some("C:/icon.png".into()))), IconPick::Use("C:/icon.png".into()));
        assert_eq!(icon_pick(Ok(None)), IconPick::Cancelled);
        let bad = AppError::new(ErrorCode::InvalidInput, "not an image").with_param("error", "not an image");
        assert_eq!(icon_pick(Err(bad)), IconPick::Invalid);
    }
}
