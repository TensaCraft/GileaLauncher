//! Settings → «Сховище»: what the launcher's own files take, per build and for the launcher. Each
//! line has a button to clear it, «Очистити непотрібне» clears them all at once, and the rules the
//! launcher cleans by once a day have their switches here.

use std::collections::HashSet;

use launcher_shared::{
    AppError, ComponentsSnapshot, Level, LineSize, RuleState, StorageLine, StorageRule, StorageSnapshot,
    units,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ConfirmDialog, Dialog, DialogFooter, DialogTone, Icon, IconAction, Section, SettingRow, Skeleton,
    Switch, Variant, card_image, ipc, use_toasts,
};

use super::SettingsSection;
use crate::modules::use_module_parts;
use crate::store::use_store;

/// A build's row: its lines (a vanilla build's without the mods' ones), its total, and its share
/// of the largest build's total, which the bar shows.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildRow {
    pub key: String,
    pub name: String,
    pub total: u64,
    pub share: f64,
    pub running: bool,
    pub lines: Vec<LineSize>,
}

/// The builds' rows, the largest first.
pub fn rows(snapshot: &StorageSnapshot) -> Vec<BuildRow> {
    let largest = snapshot.builds.iter().map(|b| b.total()).max().unwrap_or(0).max(1);
    let mut rows: Vec<BuildRow> = snapshot
        .builds
        .iter()
        .map(|b| BuildRow {
            key: b.key.clone(),
            name: b.name.clone(),
            total: b.total(),
            share: b.total() as f64 / largest as f64,
            running: b.running,
            lines: b.lines.iter().filter(|l| !(b.vanilla && l.line.mods_only())).cloned().collect(),
        })
        .collect();
    rows.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// What «Очистити непотрібне» asks about: the lines it frees, summed over the builds.
pub fn confirm_lines(snapshot: &StorageSnapshot) -> Vec<LineSize> {
    snapshot.unneeded()
}

/// What the components no build runs or builds on take (the «Компоненти» page removes them).
pub fn unused_components(list: &ComponentsSnapshot) -> u64 {
    list.components.iter().filter(|c| c.used_by.is_empty() && c.base_for.is_empty()).map(|c| c.size).sum()
}

fn line_icon(line: StorageLine) -> &'static str {
    match line {
        StorageLine::WorldBackups => "public",
        StorageLine::ModBackups => "extension",
        StorageLine::ModpackBackups => "inventory_2",
        StorageLine::SyncHistory => "history",
        StorageLine::ServiceFolder => "folder_special",
        StorageLine::CrashReports => "bug_report",
        StorageLine::GameLogs => "description",
        StorageLine::Thumbnails => "image",
        StorageLine::UpdateDownloads => "system_update_alt",
        StorageLine::TempFiles => "hourglass_empty",
    }
}

#[derive(serde::Serialize)]
struct RefreshArgs {
    refresh: bool,
}

#[derive(serde::Serialize)]
struct ClearArgs {
    key: Option<String>,
    line: StorageLine,
}

#[derive(serde::Serialize)]
struct RuleArgs {
    rule: StorageRule,
    on: bool,
}

/// A world-backups clear waiting for its confirmation: the build's key and name, and the size.
#[derive(Clone)]
struct WorldClear {
    key: String,
    name: String,
    bytes: u64,
}

#[component]
pub fn UsageSection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let parts = use_module_parts();
    let active = use_context::<SettingsSection>();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let size = move |bytes: u64| units::size(bytes, &i18n.lang());

    let snapshot = RwSignal::new(None::<StorageSnapshot>);
    let unused = RwSignal::new(0u64);
    let keep = RwSignal::new(None::<u64>);
    let open = RwSignal::new(HashSet::<String>::new());
    // What is being cleared: `<key>/<line>`, `unneeded`, or `refresh`.
    let working = RwSignal::new(None::<String>);
    let confirm_all = RwSignal::new(false);
    let confirm_world = RwSignal::new(false);
    let world_clear = RwSignal::new(None::<WorldClear>);

    let has_backups = Signal::derive(move || parts.with(|p| p.sections.iter().any(|s| s.id == "backups")));
    let load = move |refresh: bool| {
        if refresh {
            working.set(Some("refresh".into()));
        }
        spawn_local(async move {
            match ipc::invoke::<_, StorageSnapshot>("storage_snapshot", &RefreshArgs { refresh }).await {
                Ok(fresh) => {
                    let _ = snapshot.try_set(Some(fresh));
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
            if let Ok(list) = ipc::call::<ComponentsSnapshot>("components_list").await {
                let _ = unused.try_set(unused_components(&list));
            }
            let _ = working.try_set(None);
        });
    };
    load(false);
    Effect::new(move |_| {
        if has_backups.get() {
            spawn_local(async move {
                if let Ok(settings) =
                    ipc::module_invoke::<_, Value>("backups", "settings", &Value::Null).await
                {
                    let _ = keep.try_set(settings["keep"].as_u64());
                }
            });
        }
    });

    // After a clear: the new sizes, and what it freed.
    let cleared = move |before: u64, result: Result<StorageSnapshot, AppError>| match result {
        Ok(fresh) => {
            let freed = before.saturating_sub(fresh.total());
            let _ = snapshot.try_set(Some(fresh));
            toasts.show(Level::Success, i18n.tp("storage_cleaned", &[("size", size(freed))]), None);
        }
        Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
    };
    let clear = move |key: Option<String>, line: StorageLine| {
        if working.get_untracked().is_some() {
            return;
        }
        let before = snapshot.with_untracked(|s| s.as_ref().map_or(0, StorageSnapshot::total));
        working.set(Some(format!("{}/{}", key.clone().unwrap_or_default(), line.label_key())));
        spawn_local(async move {
            let result = ipc::invoke::<_, StorageSnapshot>("storage_clear", &ClearArgs { key, line }).await;
            cleared(before, result);
            let _ = working.try_set(None);
        });
    };
    let clear_all = move || {
        let before = snapshot.with_untracked(|s| s.as_ref().map_or(0, StorageSnapshot::total));
        working.set(Some("unneeded".into()));
        spawn_local(async move {
            cleared(before, ipc::call::<StorageSnapshot>("storage_clear_unneeded").await);
            let _ = working.try_set(None);
        });
    };
    let set_rule = move |rule: StorageRule, on: bool| {
        spawn_local(async move {
            match ipc::invoke::<_, StorageSnapshot>("storage_set_rule", &RuleArgs { rule, on }).await {
                Ok(fresh) => {
                    let _ = snapshot.try_set(Some(fresh));
                }
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    };

    let busy = move |id: String| Signal::derive(move || working.get().as_deref() == Some(id.as_str()));
    // One line: its icon, label, size, and its «Очистити» when it holds something.
    let line_row = move |key: Option<String>, name: String, running: bool, item: LineSize| {
        let LineSize { line, bytes } = item;
        let id = format!("{}/{}", key.clone().unwrap_or_default(), line.label_key());
        let button = (bytes > 0).then(|| {
            let key = key.clone();
            let on_click = Callback::new(move |()| {
                if line == StorageLine::WorldBackups {
                    world_clear.set(Some(WorldClear {
                        key: key.clone().unwrap_or_default(),
                        name: name.clone(),
                        bytes,
                    }));
                    confirm_world.set(true);
                } else {
                    clear(key.clone(), line);
                }
            });
            view! {
                <Button icon="cleaning_services" disabled=running loading=busy(id.clone()) on_click=on_click>
                    {move || i18n.t("storage_clear")}
                </Button>
            }
        });
        // The column stays when there is no button: the sizes line up.
        let tip = (running && bytes > 0).then(|| i18n.t("storage_running"));
        view! {
            <div class="usage-line">
                <Icon name=line_icon(line) class="usage-line__icon" />
                <span class="usage-line__label">{move || i18n.t(line.label_key())}</span>
                <span class="usage-line__size" class:is-empty=bytes == 0>
                    {move || if bytes == 0 { i18n.t("storage_empty") } else { size(bytes) }}
                </span>
                <span class="usage-line__action" data-tip=tip data-tip-side="top">{button}</span>
            </div>
        }
    };

    let build_row = move |row: BuildRow| {
        let key = row.key.clone();
        let cover = store
            .builds
            .with_untracked(|builds| builds.iter().find(|b| b.key == row.key).and_then(|b| b.image.clone()));
        let unfolded = {
            let key = key.clone();
            Signal::derive(move || open.with(|o| o.contains(&key)))
        };
        let toggle = {
            let key = key.clone();
            move |_| {
                ui_kit::sound::play_click();
                open.update(|o| {
                    if !o.remove(&key) {
                        o.insert(key.clone());
                    }
                })
            }
        };
        let lines = {
            let (key, name, running, items) = (key.clone(), row.name.clone(), row.running, row.lines.clone());
            move || {
                items
                    .iter()
                    .cloned()
                    .map(|item| line_row(Some(key.clone()), name.clone(), running, item))
                    .collect_view()
            }
        };
        let width = format!("width:{:.1}%", (row.share * 100.0).max(1.0));
        view! {
            <div class="usage-build" class:is-open=unfolded>
                <button type="button" class="usage-build__head" aria-expanded=move || unfolded.get().to_string() on:click=toggle>
                    <img class="usage-build__cover" src=card_image(cover.as_deref()) alt="" />
                    <span class="usage-build__name">{row.name.clone()}</span>
                    {row.running.then(|| view! {
                        <span class="usage-build__running" data-tip=move || i18n.t("storage_running") data-tip-side="top">
                            <Icon name="play_circle" />
                        </span>
                    })}
                    <span class="usage-build__bar"><span class="usage-build__fill" style=width></span></span>
                    <span class="usage-build__total">{size(row.total)}</span>
                    <Icon name="expand_more" class="usage-build__chevron" />
                </button>
                {move || unfolded.get().then(|| view! { <div class="usage-build__lines">{lines()}</div> })}
            </div>
        }
    };

    let summary = move || {
        snapshot.get().map(|s| {
            let total = i18n.tp("storage_total", &[("size", size(s.total()))]);
            let free = s.free.map(|f| i18n.tp("storage_free", &[("size", size(f))]));
            view! {
                <div class="usage-summary">
                    <div class="usage-summary__text">
                        <div class="usage-summary__total">{total}</div>
                        {free.map(|f| view! { <div class="usage-summary__free">{f}</div> })}
                    </div>
                    <IconAction
                        icon="refresh"
                        title=t("refresh")
                        loading=busy("refresh".into())
                        on_click=Callback::new(move |()| load(true))
                    />
                    <Button
                        variant=Variant::Primary
                        icon="cleaning_services"
                        disabled=s.unneeded().is_empty()
                        loading=busy("unneeded".into())
                        on_click=move |_| confirm_all.set(true)
                    >
                        {move || i18n.t("storage_clear_unneeded")}
                    </Button>
                </div>
            }
        })
    };
    let skeleton = || {
        (0..3)
            .map(|_| view! { <div class="usage-build usage-build--skeleton"><Skeleton /></div> })
            .collect_view()
    };
    let builds = move || match snapshot.get() {
        None => skeleton().into_any(),
        Some(s) => rows(&s).into_iter().map(build_row).collect_view().into_any(),
    };
    let launcher = move || {
        snapshot.get().map(|s| {
            let lines =
                s.launcher.into_iter().map(|item| line_row(None, String::new(), false, item)).collect_view();
            let components = unused.get();
            view! {
                {lines}
                <div class="usage-line">
                    <Icon name="construction" class="usage-line__icon" />
                    <span class="usage-line__label">{move || i18n.t("storage_unused_components")}</span>
                    <span class="usage-line__size" class:is-empty=components == 0>
                        {move || if components == 0 { i18n.t("storage_empty") } else { size(components) }}
                    </span>
                    <span class="usage-line__action">
                        <Button icon="construction" on_click=move |_| store.go("/builds/components")>
                            {move || i18n.t("minecraft_components_nav")}
                        </Button>
                    </span>
                </div>
            }
        })
    };
    let rule_rows = move || {
        let rules: Vec<RuleState> =
            snapshot.with(|s| s.as_ref().map(|s| s.rules.clone()).unwrap_or_default());
        rules
            .into_iter()
            .map(|state| {
                let checked = RwSignal::new(state.on);
                let rule = state.rule;
                view! {
                    <SettingRow title=t(rule.label_key()) desc=t(rule.desc_key())>
                        <Switch checked=checked on_change=Callback::new(move |on| set_rule(rule, on)) />
                    </SettingRow>
                }
            })
            .collect_view()
    };
    let world_rule = move || {
        keep.get().filter(|_| has_backups.get()).map(|count| {
            let desc = i18n.tp("storage_rule_world_backups_desc", &[("keep", count.to_string())]);
            view! {
                <SettingRow title=t("storage_rule_world_backups") desc=desc>
                    <IconAction
                        icon="tune"
                        compact=true
                        title=t("storage_configure")
                        on_click=Callback::new(move |()| {
                            if let Some(SettingsSection(active)) = active {
                                active.set("backups");
                            }
                        })
                    />
                </SettingRow>
            }
        })
    };
    let all_lines = move || {
        snapshot
            .with(|s| s.as_ref().map(confirm_lines).unwrap_or_default())
            .into_iter()
            .map(|l| {
                view! {
                    <li class="usage-confirm__line">
                        <span>{i18n.t(l.line.label_key())}</span>
                        <span class="usage-confirm__size">{size(l.bytes)}</span>
                    </li>
                }
            })
            .collect_view()
    };
    let world_message = Signal::derive(move || {
        world_clear.get().map_or(String::new(), |w| {
            i18n.tp("storage_world_backups_confirm", &[("name", w.name), ("size", size(w.bytes))])
        })
    });

    view! {
        <Section icon="data_usage" title=t("storage_title") desc=t("storage_desc")>
            {summary}
            <div class="usage-builds">{builds}</div>
        </Section>
        <Section icon="rocket_launch" title=t("storage_launcher")>
            <div class="usage-builds">{launcher}</div>
        </Section>
        <Section icon="auto_delete" title=t("storage_rules_title") desc=t("storage_rules_desc")>
            {world_rule}
            {rule_rows}
        </Section>
        <Dialog open=confirm_all title=t("storage_clear_unneeded_title") icon="cleaning_services" tone=DialogTone::Warning>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| confirm_all.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    on_click=move |_| {
                        confirm_all.set(false);
                        clear_all();
                    }
                >
                    {move || i18n.t("storage_clear")}
                </Button>
            </DialogFooter>
            <ul class="usage-confirm">{all_lines}</ul>
            <p class="hint">{move || i18n.t("storage_clear_unneeded_keep")}</p>
        </Dialog>
        <ConfirmDialog
            open=confirm_world
            title=t("confirmation")
            message=world_message
            confirm_label=t("delete")
            cancel_label=t("cancel")
            danger=true
            on_confirm=Callback::new(move |()| {
                if let Some(w) = world_clear.get_untracked() {
                    clear(Some(w.key), StorageLine::WorldBackups);
                }
            })
        />
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{
        BuildStorage, ComponentDto, ComponentsSnapshot, LineSize, StorageLine, StorageSnapshot,
    };

    use super::*;

    fn line(line: StorageLine, bytes: u64) -> LineSize {
        LineSize { line, bytes }
    }

    fn build(key: &str, vanilla: bool, lines: Vec<LineSize>) -> BuildStorage {
        BuildStorage { key: key.into(), name: key.to_uppercase(), vanilla, running: false, lines }
    }

    fn snapshot(builds: Vec<BuildStorage>) -> StorageSnapshot {
        StorageSnapshot {
            builds,
            launcher: vec![line(StorageLine::Thumbnails, 5)],
            free: None,
            rules: Vec::new(),
        }
    }

    #[test]
    fn rows_go_largest_first_with_their_share() {
        let rows = rows(&snapshot(vec![
            build("small", false, vec![line(StorageLine::GameLogs, 25)]),
            build("big", false, vec![line(StorageLine::ModBackups, 60), line(StorageLine::GameLogs, 40)]),
        ]));
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["big", "small"]);
        assert_eq!((rows[0].total, rows[0].share), (100, 1.0));
        assert_eq!((rows[1].total, rows[1].share), (25, 0.25));
    }

    #[test]
    fn a_vanilla_build_shows_no_mod_lines() {
        let lines = vec![line(StorageLine::WorldBackups, 1), line(StorageLine::ModBackups, 0)];
        let rows = rows(&snapshot(vec![build("plain", true, lines.clone()), build("modded", false, lines)]));
        let shown = |key: &str| -> Vec<StorageLine> {
            rows.iter().find(|r| r.key == key).unwrap().lines.iter().map(|l| l.line).collect()
        };
        assert_eq!(shown("plain"), vec![StorageLine::WorldBackups]);
        assert_eq!(shown("modded"), vec![StorageLine::WorldBackups, StorageLine::ModBackups]);
    }

    #[test]
    fn the_confirmation_lists_only_what_goes() {
        let lines = confirm_lines(&snapshot(vec![build(
            "a",
            false,
            vec![
                line(StorageLine::WorldBackups, 9),
                line(StorageLine::GameLogs, 3),
                line(StorageLine::CrashReports, 0),
            ],
        )]));
        assert_eq!(lines, vec![line(StorageLine::GameLogs, 3), line(StorageLine::Thumbnails, 5)]);
    }

    #[test]
    fn unused_components_are_those_no_build_uses() {
        let component = |size: u64, used_by: &[&str], base_for: &[&str]| ComponentDto {
            id: "x".into(),
            loader: None,
            minecraft: None,
            loader_version: None,
            size,
            modified_ms: None,
            used_by: used_by.iter().map(|s| s.to_string()).collect(),
            base_for: base_for.iter().map(|s| s.to_string()).collect(),
        };
        let list = ComponentsSnapshot {
            components: vec![
                component(10, &[], &[]),
                component(20, &["Aero"], &[]),
                component(40, &[], &["fabric-loader-0.16.9-1.21.1"]),
                component(5, &[], &[]),
            ],
        };
        assert_eq!(unused_components(&list), 15);
    }
}
