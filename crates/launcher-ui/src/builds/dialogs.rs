//! Build dialogs and the build context menu: Copy asks for the new name,
//! Delete asks whether the folder goes too.

use launcher_shared::BuildDto;
use leptos::prelude::*;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::module::{BuildActionRun, ModuleBuildAction};
use ui_kit::{
    Button, Checkbox, ContextMenu, Dialog, DialogFooter, DialogTone, Field, InFlight, MenuEntry, TextInput,
    Variant, use_context_menu,
};

use super::actions::{BuildActions, use_build_actions};
use super::launch::{LaunchFlow, use_launch_flow};
use super::{build_subtitle, check_new_name, unique_name};
use crate::store::{AppStore, use_store};

/// `(id, translation key, icon, danger)`; `None` is a separator. A build of `client` without a mod
/// loader has no mods and shader packs.
pub fn build_menu_items(
    client: Option<&str>,
) -> Vec<Option<(&'static str, &'static str, &'static str, bool)>> {
    let loader = launcher_shared::mods_supported(client);
    let items = vec![
        Some(("play", "play", "play_arrow", false)),
        Some(("copy", "copy", "content_copy", false)),
        Some(("open", "open_directory", "folder_open", false)),
        Some(("shortcut", "create_desktop_shortcut", "add_to_home_screen", false)),
        None,
        Some(("mods", "mods_content_tab", "extension", false)),
        Some(("resourcepacks", "resourcepacks", "palette", false)),
        Some(("shaders", "shaders_content_tab", "wb_sunny", false)),
        Some(("screenshots", "version_screenshots_content_tab", "photo_library", false)),
        Some(("settings", "version_settings_content_tab", "tune", false)),
        None,
        Some(("delete", "delete", "delete", true)),
    ];
    items.into_iter().filter(|item| loader || !matches!(item, Some(("mods" | "shaders", ..)))).collect()
}

/// The menu of `build`: its own entries, and the modules' `actions` that apply to it before Delete.
/// What a module's menu entry does once picked.
pub enum MenuRun {
    /// Module `.0`'s command `.1` with `{"key"}`.
    Command(&'static str, &'static str),
    /// The module's own window for the build.
    Open(fn(BuildDto)),
}

pub fn menu_run(action: &ModuleBuildAction) -> MenuRun {
    match action.run {
        BuildActionRun::Command(command) => MenuRun::Command(action.module, command),
        BuildActionRun::Open(open) => MenuRun::Open(open),
    }
}

pub fn build_menu_items_with(
    build: &BuildDto,
    actions: &[ModuleBuildAction],
) -> Vec<Option<(&'static str, &'static str, &'static str, bool)>> {
    let mut items = build_menu_items(build.client.as_deref());
    let own: Vec<_> = actions
        .iter()
        .filter(|action| (action.applies)(build))
        .map(|action| Some((action.id, action.label, action.icon, false)))
        .collect();
    // Delete stays last, after its separator.
    let at = items.iter().rposition(Option::is_none).unwrap_or(items.len());
    items.splice(at..at, own);
    items
}

/// State of the Copy and Delete dialogs, shared by every page.
#[derive(Clone, Copy)]
pub struct BuildDialogs {
    store: AppStore,
    i18n: I18nCtx,
    copy_of: RwSignal<Option<BuildDto>>,
    copy_open: RwSignal<bool>,
    copy_name: RwSignal<String>,
    copy_error: RwSignal<Option<String>>,
    delete_of: RwSignal<Option<BuildDto>>,
    delete_open: RwSignal<bool>,
    delete_files: RwSignal<bool>,
}

impl BuildDialogs {
    /// Opens Copy with "{name} (Copy)", made unique.
    pub fn copy(&self, build: BuildDto) {
        let taken: Vec<String> =
            self.store.builds.with_untracked(|b| b.iter().map(|x| x.name.clone()).collect());
        let base = self.i18n.tp("version_copy_default_name", &[("name", build.name.clone())]);
        self.copy_name.set(unique_name(&base, &taken));
        self.copy_error.set(None);
        self.copy_of.set(Some(build));
        self.copy_open.set(true);
    }

    /// Opens Delete with "delete the folder" ticked.
    pub fn delete(&self, build: BuildDto) {
        self.delete_files.set(true);
        self.delete_of.set(Some(build));
        self.delete_open.set(true);
    }
}

pub fn provide_build_dialogs() -> BuildDialogs {
    let dialogs = BuildDialogs {
        store: use_store(),
        i18n: use_i18n(),
        copy_of: RwSignal::new(None),
        copy_open: RwSignal::new(false),
        copy_name: RwSignal::new(String::new()),
        copy_error: RwSignal::new(None),
        delete_of: RwSignal::new(None),
        delete_open: RwSignal::new(false),
        delete_files: RwSignal::new(true),
    };
    provide_context(dialogs);
    dialogs
}

pub fn use_build_dialogs() -> BuildDialogs {
    expect_context::<BuildDialogs>()
}

#[component]
pub fn BuildDialogsHost() -> impl IntoView {
    let d = use_build_dialogs();
    let actions = use_build_actions();
    let store = use_store();
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    // One copy at a time: a second Enter or click while it runs does nothing.
    let copying = InFlight::new();
    let copied = Callback::new(move |ok: bool| {
        copying.done();
        // The new build shows in the lists (the backend announces it); the page stays where the
        // player is by now.
        if ok {
            let _ = d.copy_open.try_set(false);
        }
    });
    let submit_copy = move || {
        let Some(build) = d.copy_of.get_untracked() else { return };
        let raw = d.copy_name.get_untracked();
        match store.builds.with_untracked(|builds| check_new_name(&raw, builds)) {
            Err(problem) => {
                d.copy_error.set(Some(i18n.tp(problem.key(), &[("name", raw.trim().to_string())])))
            }
            Ok(name) => {
                if copying.start() {
                    actions.copy(build.key, name, copied);
                }
            }
        }
    };
    let copy_info = move || {
        d.copy_of.with(|b| {
            b.as_ref().map(|b| {
                i18n.tp(
                    "copy_version_info",
                    &[
                        ("version", b.name.clone()),
                        ("client", b.client.clone().unwrap_or_default()),
                        ("mc_version", b.version.clone().unwrap_or_default()),
                    ],
                )
            })
        })
    };
    let delete_name = move || d.delete_of.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default());
    let confirm_delete = move || {
        if let Some(build) = d.delete_of.get_untracked() {
            d.delete_open.set(false);
            actions.delete(build.key, d.delete_files.get_untracked());
        }
    };
    view! {
        <Dialog open=d.copy_open title=t("copy_version_title") subtitle=Signal::derive(move || copy_info().unwrap_or_default()) icon="content_copy">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| d.copy_open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="content_copy"
                    loading=copying.running()
                    on_click=move |_| submit_copy()
                >
                    {move || i18n.t("copy")}
                </Button>
            </DialogFooter>
            <Field label=t("version_name_label") error=Signal::derive(move || d.copy_error.get())>
                <TextInput
                    value=d.copy_name
                    autofocus=true
                    invalid=Signal::derive(move || d.copy_error.get().is_some())
                    disabled=copying.running()
                    on_enter=Callback::new(move |()| submit_copy())
                />
            </Field>
        </Dialog>
        <Dialog
            open=d.delete_open
            title=Signal::derive(move || i18n.tp("version_delete_confirm_title", &[("version", delete_name())]))
            subtitle=Signal::derive(move || d.delete_of.with(|b| b.as_ref().map(build_subtitle).unwrap_or_default()))
            icon="delete_outline"
            tone=DialogTone::Danger
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| d.delete_open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Danger icon="delete" on_click=move |_| confirm_delete()>
                    {move || i18n.t("delete_version_action")}
                </Button>
            </DialogFooter>
            <p style="margin:0">{move || i18n.t("version_delete_warning_desc")}</p>
            <Checkbox checked=d.delete_files label=t("version_delete_directory") />
            <div class="hint">{move || i18n.t("version_delete_directory_desc")}</div>
        </Dialog>
    }
}

/// Opens the context menu of a build. Create during component setup (`use_build_menu`).
#[derive(Clone, Copy)]
pub struct BuildMenu {
    menu: ContextMenu,
    parts: Signal<crate::modules::ModuleParts>,
    toasts: ui_kit::Toasts,
    i18n: I18nCtx,
    flow: LaunchFlow,
    actions: BuildActions,
    dialogs: BuildDialogs,
    store: AppStore,
}

impl BuildMenu {
    /// The menu outlives the row that opened it (a game starting or stopping redraws the row), so
    /// everything the choice needs is taken now: nothing the row owns is read afterwards.
    pub fn open(&self, build: BuildDto, x: i32, y: i32) {
        let Some(actions) = self.parts.try_with_untracked(|p| p.build_actions.clone()) else { return };
        let entries = build_menu_items_with(&build, &actions)
            .into_iter()
            .map(|item| match item {
                Some((id, key, icon, danger)) => {
                    let entry = MenuEntry::item(id, self.i18n.t(key)).icon(icon);
                    if danger { entry.danger() } else { entry }
                }
                None => MenuEntry::Separator,
            })
            .collect();
        let this = *self;
        self.menu.open_with(x, y, entries, move |id| this.run(&id, build.clone(), &actions));
    }

    fn run(&self, id: &str, build: BuildDto, actions: &[ui_kit::module::ModuleBuildAction]) {
        match id {
            "play" => self.flow.start(build),
            "copy" => self.dialogs.copy(build),
            "open" => self.actions.open_dir(build.key),
            "shortcut" => self.actions.shortcut(build.key),
            tab @ ("mods" | "resourcepacks" | "shaders" | "screenshots" | "settings") => {
                self.store.go(&crate::pages::content::content_path(&build.key, tab))
            }
            "delete" => self.dialogs.delete(build),
            other => {
                let action = actions.iter().copied().find(|a| a.id == other);
                match action.as_ref().map(menu_run) {
                    Some(MenuRun::Open(open)) => open(build),
                    Some(MenuRun::Command(module, command)) => {
                        let (toasts, i18n) = (self.toasts, self.i18n);
                        leptos::task::spawn_local(async move {
                            let args = serde_json::json!({"key": build.key});
                            let done =
                                ui_kit::ipc::module_invoke::<_, serde_json::Value>(module, command, &args)
                                    .await;
                            if let Err(e) = done {
                                toasts.show(launcher_shared::Level::Error, i18n.error(&e), None);
                            }
                        });
                    }
                    None => {}
                }
            }
        }
    }
}

pub fn use_build_menu() -> BuildMenu {
    BuildMenu {
        menu: use_context_menu(),
        parts: crate::modules::use_module_parts(),
        toasts: ui_kit::use_toasts(),
        i18n: use_i18n(),
        flow: use_launch_flow(),
        actions: use_build_actions(),
        dialogs: use_build_dialogs(),
        store: use_store(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_menu_matches_the_original() {
        let items = build_menu_items(Some("Fabric"));
        let ids: Vec<&str> = items.iter().map(|i| i.map_or("-", |(id, ..)| id)).collect();
        assert_eq!(
            ids,
            [
                "play",
                "copy",
                "open",
                "shortcut",
                "-",
                "mods",
                "resourcepacks",
                "shaders",
                "screenshots",
                "settings",
                "-",
                "delete"
            ]
        );
        let keys: Vec<&str> = items.iter().flatten().map(|(_, key, ..)| *key).collect();
        assert_eq!(
            keys,
            [
                "play",
                "copy",
                "open_directory",
                "create_desktop_shortcut",
                "mods_content_tab",
                "resourcepacks",
                "shaders_content_tab",
                "version_screenshots_content_tab",
                "version_settings_content_tab",
                "delete"
            ]
        );
        assert!(items.iter().flatten().all(|(id, _, _, danger)| *danger == (*id == "delete")));
    }

    #[test]
    fn a_vanilla_build_menu_has_no_mods_or_shaders() {
        let ids: Vec<&str> =
            build_menu_items(Some("Minecraft")).into_iter().flatten().map(|(id, ..)| id).collect();
        assert!(!ids.contains(&"mods") && !ids.contains(&"shaders"), "{ids:?}");
        assert!(ids.contains(&"resourcepacks") && ids.contains(&"screenshots"));
    }

    #[test]
    fn module_actions_show_for_builds_they_apply_to() {
        let action = ModuleBuildAction {
            module: "tensa",
            id: "force_sync",
            icon: "sync",
            label: "tensacraft_force_sync",
            run: BuildActionRun::Command("force_sync"),
            applies: |b| b.client.as_deref() == Some("TensaCraft"),
        };
        let mut build = BuildDto {
            key: "aero".into(),
            version_id: "aero".into(),
            name: "Aero".into(),
            version: Some("26.3".into()),
            loader: None,
            client: None,
            loader_version: None,
            game_dir: String::new(),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
        };
        build.client = Some("TensaCraft".into());
        let ids: Vec<&str> =
            build_menu_items_with(&build, &[action]).iter().map(|i| i.map_or("-", |(id, ..)| id)).collect();
        let at = ids.iter().position(|id| *id == "force_sync").expect("the action is there");
        assert_eq!(
            &ids[at - 1..],
            ["settings", "force_sync", "-", "delete"],
            "after the build's own entries, before Delete"
        );
        build.client = Some("Fabric".into());
        let plain: Vec<&str> =
            build_menu_items_with(&build, &[action]).into_iter().flatten().map(|(id, ..)| id).collect();
        assert!(!plain.contains(&"force_sync"));
    }

    #[test]
    fn an_open_action_runs_in_the_ui() {
        fn open(_: BuildDto) {}
        let action = |run| ModuleBuildAction {
            module: "reports",
            id: "report",
            icon: "bug_report",
            label: "version_report_button",
            run,
            applies: |_| true,
        };
        assert!(matches!(menu_run(&action(BuildActionRun::Open(open))), MenuRun::Open(_)));
        assert!(matches!(
            menu_run(&action(BuildActionRun::Command("send"))),
            MenuRun::Command("reports", "send")
        ));
    }
}
