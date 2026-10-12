//! The group editor: builds and mode, then what to sync in a tree of the builds' folders, then a
//! summary in plain words.

use std::collections::{BTreeSet, HashMap, HashSet};

use launcher_shared::{BuildDto, units};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, Checkbox, ChoiceCards, ChoiceOption, Dialog, DialogFooter, Field, Icon, InFlight, TextInput,
    Variant,
};

use super::card::HowItWorks;
use super::model::{mismatch, summary, tree_rows};
use super::{api, state};
use crate::dto::{Lock, SyncGroup, SyncMode, TreeEntry, error_text};

#[component]
pub fn EditorDialog(
    open: RwSignal<bool>,
    initial: RwSignal<SyncGroup>,
    #[prop(into)] builds: Signal<Vec<BuildDto>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let step = RwSignal::new(1u8);
    let name = RwSignal::new(String::new());
    let chosen = RwSignal::new(BTreeSet::<String>::new());
    let start = RwSignal::new(String::new());
    let mode = RwSignal::new("latest".to_string());
    let checked = RwSignal::new(BTreeSet::<String>::new());
    let loaded = RwSignal::new(HashMap::<String, Vec<TreeEntry>>::new());
    let expanded = RwSignal::new(HashSet::<String>::new());
    let filter = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saving = InFlight::new();

    // Each time it opens: the group it edits, or a new one.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let group = initial.get_untracked();
        step.set(1);
        name.set(group.name.clone());
        chosen.set(group.builds.iter().cloned().collect());
        start.set(group.start.clone());
        mode.set(if group.mode == SyncMode::Base { "base" } else { "latest" }.to_string());
        checked.set(group.paths.iter().cloned().collect());
        loaded.set(HashMap::new());
        expanded.set(HashSet::new());
        filter.set(String::new());
        error.set(None);
    });
    // The builds in the list's order.
    let picked = move || -> Vec<String> {
        builds.with(|all| {
            all.iter().filter(|b| chosen.with(|c| c.contains(&b.key))).map(|b| b.key.clone()).collect()
        })
    };
    let group_now = move || SyncGroup {
        id: initial.with_untracked(|g| g.id.clone()),
        name: name.get(),
        builds: picked(),
        paths: checked.get().into_iter().collect(),
        mode: if mode.get() == "base" { SyncMode::Base } else { SyncMode::Latest },
        start: start.get(),
    };
    let load = move |dir: String| {
        let (keys, editing) =
            (picked(), Some(initial.with_untracked(|g| g.id.clone())).filter(|id| !id.is_empty()));
        spawn_local(async move {
            match api::tree(keys, dir.clone(), editing).await {
                Ok(rows) => {
                    let _ = loaded.try_update(|l| {
                        l.insert(dir, rows);
                    });
                }
                Err(e) => {
                    let _ = error.try_set(Some(i18n.text(&error_text(&e))));
                }
            }
        });
    };
    let can_next = move || match step.get() {
        1 => {
            !name.get().trim().is_empty()
                && chosen.with(|c| c.len() >= 2)
                && chosen.with(|c| c.contains(&start.get()))
        }
        2 => checked.with(|c| !c.is_empty()),
        _ => true,
    };
    let next = move || {
        if step.get_untracked() == 1 {
            loaded.set(HashMap::new());
            expanded.set(HashSet::new());
            load(String::new());
        }
        step.update(|s| *s += 1);
    };
    let save = move || {
        if !saving.start() {
            return;
        }
        let group = group_now();
        spawn_local(async move {
            match api::save(group).await {
                Ok(_) => {
                    state::refresh();
                    let _ = open.try_set(false);
                }
                Err(e) => {
                    let _ = error.try_set(Some(i18n.text(&error_text(&e))));
                }
            }
            saving.done();
        });
    };

    let modes = Signal::derive(move || {
        vec![
            ChoiceOption {
                value: "latest".into(),
                label: i18n.t("sync_mode_latest"),
                icon: "update".into(),
                desc: Some(i18n.t("sync_mode_latest_desc")),
                image: None,
            },
            ChoiceOption {
                value: "base".into(),
                label: i18n.t("sync_mode_base"),
                icon: "star".into(),
                desc: Some(i18n.t("sync_mode_base_desc")),
                image: None,
            },
        ]
    });
    let build_rows = move || {
        builds
            .get()
            .into_iter()
            .map(|build| {
                let key = build.key.clone();
                // Follows the chosen set: a dialog opened again shows its own builds' ticks.
                let on = RwSignal::new(false);
                {
                    let key = key.clone();
                    Effect::new(move |_| on.set(chosen.with(|c| c.contains(&key))));
                }
                let toggled = {
                    let key = key.clone();
                    Callback::new(move |value: bool| {
                        chosen.update(|c| {
                            if value {
                                c.insert(key.clone());
                            } else {
                                c.remove(&key);
                            }
                        });
                        if value && start.with_untracked(String::is_empty) {
                            start.set(key.clone());
                        }
                        if !value && start.with_untracked(|s| *s == key) {
                            start.set(chosen.with_untracked(|c| c.iter().next().cloned().unwrap_or_default()));
                        }
                    })
                };
                let is_start = {
                    let key = key.clone();
                    move || start.get() == key
                };
                let is_chosen = {
                    let key = key.clone();
                    move || chosen.with(|c| c.contains(&key))
                };
                let label = Signal::derive(move || {
                    if mode.get() == "base" { i18n.t("sync_start_base") } else { i18n.t("sync_start_latest") }
                });
                let make_start = key.clone();
                view! {
                    <div class="sync-pick">
                        <Checkbox checked=on label=build.name.clone() on_change=toggled />
                        <button
                            type="button"
                            class="sync-pick__start"
                            class:is-on=is_start
                            class:is-hidden=move || !is_chosen()
                            on:click=move |_| start.set(make_start.clone())
                        >
                            <Icon name=if is_start() { "radio_button_checked" } else { "radio_button_unchecked" } />
                            {move || label.get()}
                        </button>
                    </div>
                }
            })
            .collect_view()
    };
    let tree = move || {
        let rows = tree_rows(&loaded.get(), &expanded.get(), &checked.get(), &filter.get());
        if rows.is_empty() {
            return view! { <div class="hint">{move || i18n.t("sync_tree_empty")}</div> }.into_any();
        }
        rows.into_iter()
            .map(|row| {
                let path = row.entry.path.clone();
                let indent = format!("padding-left:{}px", 8 + row.depth * 20);
                let toggle_open = {
                    let path = path.clone();
                    move |_| {
                        let opening = !expanded.with_untracked(|e| e.contains(&path));
                        expanded.update(|e| {
                            if !e.remove(&path) {
                                e.insert(path.clone());
                            }
                        });
                        if opening && !loaded.with_untracked(|l| l.contains_key(&path)) {
                            load(path.clone());
                        }
                    }
                };
                let toggle_check = {
                    let path = path.clone();
                    move |_| {
                        checked.update(|c| {
                            if !c.remove(&path) {
                                // A folder takes everything in it: the paths inside it go.
                                c.retain(|p| !p.starts_with(&format!("{path}/")));
                                c.insert(path.clone());
                            }
                        })
                    }
                };
                let tip = match &row.entry.lock {
                    Some(Lock::Managed) => Some(i18n.t("sync_lock_managed")),
                    Some(Lock::Taken { group }) => Some(i18n.tp("sync_lock_taken", &[("group", group.clone())])),
                    None if !row.entry.managed_in.is_empty() => {
                        Some(i18n.tp("sync_managed_in", &[("builds", row.entry.managed_in.join(", "))]))
                    }
                    None => None,
                };
                let opened = expanded.with_untracked(|e| e.contains(&path));
                let size = units::size(row.entry.size, &i18n.lang());
                view! {
                    <div class="sync-row" class:is-locked=row.locked style=indent>
                        {if row.entry.dir {
                            view! {
                                <button type="button" class="sync-row__open" on:click=toggle_open>
                                    <Icon name=if opened { "expand_more" } else { "chevron_right" } />
                                </button>
                            }
                            .into_any()
                        } else {
                            view! { <span class="sync-row__open"></span> }.into_any()
                        }}
                        <input
                            type="checkbox"
                            class="sync-row__check"
                            prop:checked=row.checked
                            disabled=row.locked || row.covered
                            on:change=toggle_check
                        />
                        <Icon name=if row.entry.dir { "folder" } else { "description" } class="sync-row__icon" />
                        <span class="sync-row__name">{row.entry.name.clone()}</span>
                        {row.present.map(|(present, total)| view! {
                            <span class="sync-row__present">
                                {i18n.tp("sync_present", &[("present", present.to_string()), ("total", total.to_string())])}
                            </span>
                        })}
                        {tip.map(|tip| view! {
                            <span class="sync-row__lock" data-tip=tip data-tip-side="top"><Icon name="lock" /></span>
                        })}
                        <span class="sync-row__size" class:is-large=row.large>{size}</span>
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };
    let summary_text = move || {
        let group = group_now();
        let parts = summary(&builds.get(), &group);
        let size: u64 = loaded
            .with(|l| l.values().flatten().filter(|e| group.paths.contains(&e.path)).map(|e| e.size).sum());
        i18n.tp(
            if group.mode == SyncMode::Base { "sync_summary_base" } else { "sync_summary" },
            &[
                ("builds", parts.builds.to_string()),
                ("paths", parts.paths.to_string()),
                ("shown", parts.shown),
                ("size", units::size(size, &i18n.lang())),
                ("start", parts.start),
            ],
        )
    };
    let warn = move || mismatch(&builds.get(), &group_now());
    let title = Signal::derive(move || {
        let key = if initial.with(|g| g.id.is_empty()) { "sync_editor_new" } else { "sync_editor_edit" };
        i18n.t(key)
    });
    let subtitle = Signal::derive(move || {
        let key = match step.get() {
            1 => "sync_step_builds",
            2 => "sync_step_paths",
            _ => "sync_step_summary",
        };
        i18n.tp("sync_step", &[("n", step.get().to_string()), ("what", i18n.t(key))])
    });

    view! {
        <Dialog open=open title=title subtitle=subtitle icon="sync" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                {move || (step.get() > 1).then(|| view! {
                    <Button icon="arrow_back" on_click=move |_| step.update(|s| *s -= 1)>{move || i18n.t("sync_back")}</Button>
                })}
                {move || if step.get() < 3 {
                    view! {
                        <Button variant=Variant::Primary icon="arrow_forward" disabled=Signal::derive(move || !can_next()) on_click=move |_| next()>
                            {move || i18n.t("sync_next")}
                        </Button>
                    }
                    .into_any()
                } else {
                    view! {
                        <Button variant=Variant::Primary icon="save" loading=saving.running() on_click=move |_| save()>
                            {move || i18n.t("save")}
                        </Button>
                    }
                    .into_any()
                }}
            </DialogFooter>
            <div class="sync-editor">
                {move || error.get().map(|e| view! { <div class="sync-editor__error">{e}</div> })}
                <div class:is-hidden=move || step.get() != 1>
                    <Field label=t("sync_name")>
                        <TextInput value=name placeholder=t("sync_name_placeholder") />
                    </Field>
                    <ChoiceCards options=modes value=mode label=t("sync_mode") columns=2 />
                    <div class="sync-picks__label">{move || i18n.t("sync_builds")}</div>
                    <div class="sync-picks">{build_rows}</div>
                </div>
                <div class:is-hidden=move || step.get() != 2>
                    <TextInput value=filter placeholder=t("sync_search") icon="search" />
                    <div class="sync-tree">{tree}</div>
                </div>
                <div class:is-hidden=move || step.get() != 3>
                    <p class="sync-summary">{summary_text}</p>
                    {move || warn().then(|| view! {
                        <div class="sync-warning"><Icon name="warning_amber" />{move || i18n.t("sync_mismatch")}</div>
                    })}
                    <HowItWorks />
                </div>
            </div>
        </Dialog>
    }
}
