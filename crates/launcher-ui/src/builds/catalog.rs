//! The install catalog shared by "Create build" and the Components page's Install mode:
//! loader chips, a version search and the unstable filter over
//! Minecraft / Fabric / Quilt / Forge / NeoForge rows, 80 at a time.

use launcher_shared::{AppError, CatalogVersion, LoaderKind, LoaderOption};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ChipDef, ChipTabs, EmptyState, Icon, SelectOption, Skeleton, Switch, Tag, TagTone, TextInput,
    Variant, ipc,
};

use super::{LatestRequest, catalog_shown, loader_icon, release_date};
use crate::store::use_store;

/// One row of the catalog: Minecraft `mc`, and for loaders the builds on offer.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateRow {
    pub kind: LoaderKind,
    pub mc: String,
    pub snapshot: bool,
    /// The default loader build is a prerelease.
    pub beta: bool,
    pub release_time: Option<String>,
    pub builds: Vec<launcher_shared::LoaderBuild>,
    pub default_version: Option<String>,
}

pub fn rows_from_minecraft(list: Vec<CatalogVersion>) -> Vec<CreateRow> {
    list.into_iter()
        .map(|v| CreateRow {
            kind: LoaderKind::Minecraft,
            snapshot: v.kind == "snapshot",
            mc: v.id,
            beta: false,
            release_time: v.release_time,
            builds: Vec::new(),
            default_version: None,
        })
        .collect()
}

pub fn rows_from_loader(kind: LoaderKind, list: Vec<LoaderOption>) -> Vec<CreateRow> {
    list.into_iter()
        .map(|o| CreateRow {
            kind,
            beta: o.builds.iter().any(|b| b.version == o.default_version && !b.stable),
            snapshot: o.snapshot,
            mc: o.mc,
            release_time: None,
            builds: o.builds,
            default_version: Some(o.default_version),
        })
        .collect()
}

/// "Minecraft 1.21.1", "Fabric 1.21.1"…
pub fn row_title(row: &CreateRow) -> String {
    format!("{} {}", row.kind.display_name(), row.mc)
}

/// Forge has no unstable builds and no snapshot versions, so its tab has no filter.
pub fn offers_unstable(tab: &str) -> bool {
    tab != "forge"
}

/// The row's Minecraft version contains `query` (case and surrounding spaces ignored).
pub fn matches_query(row: &CreateRow, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || row.mc.to_lowercase().contains(&query)
}

/// The component a row installs by default (`1.21.1`, `fabric-loader-0.16.9-1.21.1`).
pub fn row_component_id(row: &CreateRow) -> String {
    row.kind.component_id(&row.mc, row.default_version.as_deref().unwrap_or_default())
}

/// The loader builds of `row` for a select; `beta` (the language's word) marks prereleases.
pub fn build_choices(row: Option<&CreateRow>, beta: &str) -> Vec<SelectOption> {
    row.map(|r| {
        r.builds
            .iter()
            .map(|b| {
                let label = if b.stable { b.version.clone() } else { format!("{} ({beta})", b.version) };
                SelectOption::new(b.version.clone(), label)
            })
            .collect()
    })
    .unwrap_or_default()
}

#[derive(serde::Serialize)]
struct LoaderCatalogArgs {
    loader: LoaderKind,
    unstable: bool,
}

#[derive(serde::Serialize)]
struct CatalogArgs {
    snapshots: bool,
}

/// The catalog rows of `kind`: Minecraft from Mojang's list, loaders from their own.
pub async fn load_rows(kind: LoaderKind, unstable: bool) -> Result<Vec<CreateRow>, AppError> {
    match kind {
        LoaderKind::Minecraft => {
            ipc::invoke::<_, Vec<CatalogVersion>>("catalog_minecraft", &CatalogArgs { snapshots: unstable })
                .await
                .map(rows_from_minecraft)
        }
        kind => ipc::invoke::<_, Vec<LoaderOption>>(
            "catalog_loader",
            &LoaderCatalogArgs { loader: kind, unstable },
        )
        .await
        .map(|list| rows_from_loader(kind, list)),
    }
}

/// The catalog toolbar and rows. `on_pick` gets the row whose "Install" was pressed. With
/// `installed` (component ids), an installed Minecraft version shows a disabled "Installed", and a
/// loader row whose default build is installed gets an "Installed" tag (other builds can still be
/// picked).
#[component]
pub fn Catalog(
    on_pick: Callback<CreateRow>,
    #[prop(optional, into)] installed: Option<Signal<Vec<String>>>,
    /// The page's switch, placed right after the search.
    #[prop(optional)]
    sources: Option<ViewFn>,
) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let tab = RwSignal::new("minecraft");
    let snapshots = RwSignal::new(false);
    // `None` while loading; the error is already translated.
    let versions = RwSignal::new(None::<Result<Vec<CreateRow>, String>>);
    let pages = RwSignal::new(1usize);
    let latest = StoredValue::new_local(LatestRequest::new());
    let tab_kind = move || match tab.get_untracked() {
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "forge" => LoaderKind::Forge,
        "neoforge" => LoaderKind::NeoForge,
        _ => LoaderKind::Minecraft,
    };
    let load = move || {
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        versions.set(None);
        pages.set(1);
        let (kind, unstable) =
            (tab_kind(), snapshots.get_untracked() && offers_unstable(tab.get_untracked()));
        spawn_local(async move {
            let result = load_rows(kind, unstable).await;
            if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                let _ = versions.try_set(Some(result.map_err(|e| i18n.error(&e))));
            }
        });
    };
    load();
    Effect::new(move |previous: Option<&'static str>| {
        let current = tab.get();
        if previous.is_some_and(|p| p != current) {
            load();
        }
        current
    });

    // The search keeps across tabs; a new query starts from the first page.
    let query = RwSignal::new(String::new());
    Effect::new(move |previous: Option<String>| {
        let current = query.get();
        if previous.is_some_and(|p| p != current) {
            pages.set(1);
        }
        current
    });
    let busy = move || store.ops.with(|o| o.busy);

    let row = move |row: CreateRow| {
        let title = row_title(&row);
        let date = release_date(row.release_time.as_deref());
        let (kind, mc, default) = (row.kind, row.mc.clone(), row.default_version.clone());
        let sub = move || match &default {
            None => date
                .clone()
                .map(|d| i18n.tp("version_create_release_date", &[("date", d)]))
                .unwrap_or_default(),
            Some(version) => format!(
                "{} • Minecraft {} • {}",
                kind.display_name(),
                mc,
                i18n.tp("version_create_loader_build", &[("version", version.clone())])
            ),
        };
        let (snapshot, beta, loader_row) = (row.snapshot, row.beta, kind != LoaderKind::Minecraft);
        let id = row_component_id(&row);
        let done = move || installed.is_some_and(|ids| ids.with(|ids| ids.contains(&id)));
        let tag_done = done.clone();
        view! {
            <div class="list-row build-row">
                <div class="build-row__icon"><Icon name=loader_icon(Some(kind.display_name())) /></div>
                <div class="build-row__text">
                    <div class="build-row__name">{title}</div>
                    <div class="build-row__sub">{sub}</div>
                </div>
                {snapshot.then(|| view! { <Tag tone=TagTone::Snapshot>{move || i18n.t("version_create_snapshot_badge")}</Tag> })}
                {beta.then(|| view! { <Tag tone=TagTone::Beta>{move || i18n.t("version_create_unstable_loader_badge")}</Tag> })}
                {move || (loader_row && tag_done()).then(|| view! {
                    <Tag tone=TagTone::Neutral>{move || i18n.t("minecraft_components_installed")}</Tag>
                })}
                {move || {
                    if !loader_row && done() {
                        view! {
                            <Button variant=Variant::Secondary icon="check" disabled=true>
                                {move || i18n.t("minecraft_components_installed")}
                            </Button>
                        }
                        .into_any()
                    } else {
                        let picked = row.clone();
                        view! {
                            <Button
                                variant=Variant::Primary
                                icon="download"
                                disabled=Signal::derive(busy)
                                on_click=move |_| on_pick.run(picked.clone())
                            >
                                {move || i18n.t("minecraft_components_install_action")}
                            </Button>
                        }
                        .into_any()
                    }
                }}
            </div>
        }
    };
    let body = move || {
        match versions.get() {
        None => view! {
            <div class="builds__list">
                {(0..6)
                    .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=48 /><Skeleton width=260 /></div> })
                    .collect_view()}
            </div>
        }
        .into_any(),
        Some(Err(message)) => view! {
            <EmptyState icon="cloud_off" title=t("version_create_error") desc=message>
                <Button icon="refresh" on_click=move |_| load()>{move || i18n.t("version_create_retry")}</Button>
            </EmptyState>
        }
        .into_any(),
        Some(Ok(list)) => {
            let q = query.get();
            let list: Vec<CreateRow> = list.into_iter().filter(|row| matches_query(row, &q)).collect();
            if list.is_empty() {
                return view! { <EmptyState icon="search_off" title=t("version_create_empty") /> }.into_any();
            }
            let total = list.len();
            let shown = catalog_shown(total, pages.get());
            view! {
                <div class="builds__list">{list.into_iter().take(shown).map(row).collect_view()}</div>
                {(shown < total).then(|| view! {
                    <div class="create__more">
                        <Button on_click=move |_| pages.update(|p| *p += 1)>{move || i18n.t("load_more")}</Button>
                    </div>
                })}
            }
            .into_any()
        }
    }
    };
    let filter_label = move || {
        i18n.t(if tab.get() == "minecraft" {
            "version_create_filter_snapshots"
        } else {
            "version_create_filter_unstable_versions"
        })
    };

    view! {
        <div class="create__bar">
            <ChipTabs
                chips=vec![
                    ChipDef { id: "minecraft", icon: "layers", label: Signal::derive(|| "Minecraft".to_string()), tone: TagTone::Vanilla },
                    ChipDef { id: "fabric", icon: "extension", label: Signal::derive(|| "Fabric".to_string()), tone: TagTone::Fabric },
                    ChipDef { id: "quilt", icon: "grid_view", label: Signal::derive(|| "Quilt".to_string()), tone: TagTone::Quilt },
                    ChipDef { id: "forge", icon: "build", label: Signal::derive(|| "Forge".to_string()), tone: TagTone::Forge },
                    ChipDef { id: "neoforge", icon: "construction", label: Signal::derive(|| "NeoForge".to_string()), tone: TagTone::NeoForge },
                ]
                active=tab
            />
            <div class="create__tools">
                // As in every list's bar: the search first, then the page's switch, then the rest.
                <div class="create__search">
                    <TextInput value=query icon="search" placeholder=t("version_create_search") />
                </div>
                {sources.map(|s| s.run())}
                <Show when=move || offers_unstable(tab.get())>
                    // Just the switch; its words are its tooltip.
                    <div class="create__filter" data-tip=filter_label data-tip-side="top">
                        <Switch checked=snapshots label=Signal::derive(filter_label) on_change=Callback::new(move |_: bool| load()) />
                    </div>
                </Show>
            </div>
        </div>
        {body}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::LoaderBuild;

    #[test]
    fn catalog_rows_cover_minecraft_and_loaders() {
        let vanilla = rows_from_minecraft(vec![CatalogVersion {
            id: "24w33a".into(),
            kind: "snapshot".into(),
            release_time: None,
        }]);
        assert_eq!(row_title(&vanilla[0]), "Minecraft 24w33a");
        assert!(vanilla[0].snapshot && vanilla[0].builds.is_empty());
        let options = vec![LoaderOption {
            mc: "1.21.1".into(),
            snapshot: false,
            builds: vec![
                LoaderBuild { version: "0.17.0-beta.1".into(), stable: false },
                LoaderBuild { version: "0.16.9".into(), stable: true },
            ],
            default_version: "0.17.0-beta.1".into(),
        }];
        let rows = rows_from_loader(LoaderKind::Fabric, options);
        assert_eq!(row_title(&rows[0]), "Fabric 1.21.1");
        assert_eq!(rows[0].default_version.as_deref(), Some("0.17.0-beta.1"));
        assert!(rows[0].beta, "the default build is a prerelease");
    }

    #[test]
    fn forge_has_no_unstable_switch() {
        assert!(!offers_unstable("forge"));
        for tab in ["minecraft", "fabric", "quilt", "neoforge"] {
            assert!(offers_unstable(tab), "{tab}");
        }
        let options = vec![LoaderOption {
            mc: "1.21.1".into(),
            snapshot: false,
            builds: vec![LoaderBuild { version: "21.1.77".into(), stable: true }],
            default_version: "21.1.77".into(),
        }];
        assert_eq!(row_title(&rows_from_loader(LoaderKind::NeoForge, options)[0]), "NeoForge 1.21.1");
    }

    #[test]
    fn search_matches_minecraft_versions() {
        let row = |mc: &str| {
            rows_from_minecraft(vec![CatalogVersion {
                id: mc.into(),
                kind: "release".into(),
                release_time: None,
            }])
            .remove(0)
        };
        assert!(matches_query(&row("1.20.1"), ""));
        assert!(matches_query(&row("1.20.1"), " 1.20 "));
        assert!(!matches_query(&row("1.21.1"), "1.20"));
        assert!(matches_query(&row("24w33a"), "24W"), "case is ignored");
    }

    #[test]
    fn rows_name_the_component_they_install() {
        let vanilla = rows_from_minecraft(vec![CatalogVersion {
            id: "1.21.1".into(),
            kind: "release".into(),
            release_time: None,
        }]);
        assert_eq!(row_component_id(&vanilla[0]), "1.21.1");
        let fabric = rows_from_loader(
            LoaderKind::Fabric,
            vec![LoaderOption {
                mc: "1.21.1".into(),
                snapshot: false,
                builds: vec![
                    LoaderBuild { version: "0.16.9".into(), stable: true },
                    LoaderBuild { version: "0.17.0-beta.1".into(), stable: false },
                ],
                default_version: "0.16.9".into(),
            }],
        );
        assert_eq!(row_component_id(&fabric[0]), "fabric-loader-0.16.9-1.21.1");
        let labels: Vec<String> =
            build_choices(fabric.first(), "Бета").into_iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            ["0.16.9", "0.17.0-beta.1 (Бета)"],
            "unstable versions are marked in the language"
        );
        assert!(build_choices(None, "Beta").is_empty());
    }
}
