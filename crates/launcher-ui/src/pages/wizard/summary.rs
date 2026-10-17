//! «Підсумок»: what «Створити й встановити» will do — the build (its loader's mark, name and
//! versions), its settings with every value told (the launcher's own named as such), and the
//! picked content by kind, each with its picture; picks a change of version or type dropped are
//! told here too.

use launcher_shared::{ContentKind, LoaderKind, PlannedPick};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Icon, Section};

use super::content::kind_icon;
use super::settings::file_name;
use super::{PicksDropped, Wizard};
use crate::builds::gpu::mode_label;
use crate::builds::loader_icon;
use crate::pages::content::tab_of;
use crate::providers::providers;
use crate::store::use_store;

/// The picks by kind, in the content tabs' order; a kind with none is left out.
fn content_groups(picks: &[PlannedPick]) -> Vec<(ContentKind, Vec<&PlannedPick>)> {
    ContentKind::ALL
        .into_iter()
        .map(|kind| (kind, picks.iter().filter(|p| p.kind == kind).collect::<Vec<_>>()))
        .filter(|(_, of_kind)| !of_kind.is_empty())
        .collect()
}

/// The account `key` names among `profiles` (key, name); `None` is the launcher's own.
fn account_name(key: Option<&str>, profiles: &[(String, String)]) -> Option<String> {
    let key = key?;
    Some(profiles.iter().find(|(k, _)| k == key).map_or_else(|| key.to_string(), |(_, name)| name.clone()))
}

fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mods_content_tab",
        ContentKind::ResourcePacks => "resourcepacks",
        ContentKind::ShaderPacks => "shaders_content_tab",
    }
}

#[component]
pub fn Summary(wizard: Wizard) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let state = wizard.state.get_untracked();
    let mark = loader_icon(Some(state.loader.display_name()));
    let versions = match state.loader {
        LoaderKind::Minecraft => format!("Minecraft {}", state.mc),
        loader => format!("Minecraft {} · {} {}", state.mc, loader.display_name(), state.loader_version),
    };
    let provider_names: Vec<(String, String)> =
        store.info.with_untracked(|i| providers(i.as_ref()).into_iter().map(|p| (p.id, p.name)).collect());
    let provider_name = move |id: &str| {
        provider_names.iter().find(|(p, _)| p == id).map_or_else(|| id.to_string(), |(_, name)| name.clone())
    };
    let profiles: Vec<(String, String)> =
        store.profiles.with_untracked(|list| list.iter().map(|p| (p.key.clone(), p.name.clone())).collect());

    // Every setting with its value; `None`: the launcher's own, told in its words.
    let s = &state.settings;
    let rows: Vec<(&'static str, &'static str, Option<String>, &'static str)> = vec![
        (
            "memory",
            "wizard_memory",
            s.max_ram_gb.map(|gb| i18n.tp("setup_memory_gb", &[("value", gb.to_string())])),
            "wizard_auto",
        ),
        (
            "developer_board",
            "gpu_mode_label",
            s.gpu_mode.as_deref().map(|m| i18n.t(mode_label(m))),
            "wizard_account_default",
        ),
        ("person", "wizard_account", account_name(s.profile.as_deref(), &profiles), "wizard_account_default"),
        ("image", "wizard_icon", s.image_path.as_deref().map(file_name), "wizard_no_icon"),
        (
            "notes",
            "build_description_label",
            Some(s.description.trim().to_string()).filter(|d| !d.is_empty()),
            "wizard_no_description",
        ),
    ];
    let total = state.picks.len();
    let groups: Vec<(ContentKind, Vec<PlannedPick>)> = content_groups(&state.picks)
        .into_iter()
        .map(|(kind, picks)| (kind, picks.into_iter().cloned().collect()))
        .collect();
    let content_title = Signal::derive(move || format!("{} · {total}", i18n.t("wizard_summary_content")));

    view! {
        <PicksDropped wizard=wizard />
        <div class="build-wizard__summary">
            <Section icon="inventory_2" title=t("wizard_summary_build")>
                <div class="build-wizard__build">
                    <div class="build-wizard__logo"><Icon name=mark /></div>
                    <div class="grow">
                        <div class="build-wizard__name">{state.name.trim().to_string()}</div>
                        <div class="hint">{versions}</div>
                    </div>
                </div>
            </Section>
            <Section icon="tune" title=t("wizard_summary_settings")>
                <div class="build-wizard__facts">
                    {rows.into_iter().map(|(icon, label, value, fallback)| {
                        let own = value.is_none();
                        view! {
                            <div class="build-wizard__fact">
                                <Icon name=icon outlined=true />
                                <span class="build-wizard__fact-label">{i18n.t(label)}</span>
                                <span class="build-wizard__fact-value" class:hint=own>
                                    {value.unwrap_or_else(|| i18n.t(fallback))}
                                </span>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </Section>
            <div class="build-wizard__picks-section">
                <Section icon="extension" title=content_title>
                    {(total == 0).then(|| view! { <p class="hint">{i18n.t("wizard_plan_empty")}</p> })}
                    {groups.into_iter().map(|(kind, picks)| {
                        let count = picks.len();
                        view! {
                            <div class=format!("build-wizard__kind build-wizard__kind--{}", tab_of(kind))>
                                <Icon name=kind_icon(kind) outlined=true />
                                <span>{i18n.t(kind_label(kind))}</span>
                                <span class="build-wizard__count">{count}</span>
                            </div>
                            <div class="build-wizard__picks">
                                {picks.into_iter().map(|pick| {
                                    let source = provider_name(&pick.provider);
                                    view! {
                                        <div class="build-wizard__item" title=pick.title.clone()>
                                            {match pick.icon_url.clone() {
                                                Some(url) => view! { <img class="build-wizard__item-pic" src=url loading="lazy" alt="" /> }.into_any(),
                                                None => view! { <span class="build-wizard__item-pic"><Icon name=kind_icon(kind) outlined=true /></span> }.into_any(),
                                            }}
                                            <div class="build-wizard__item-text">
                                                <div class="build-wizard__item-title">{pick.title.clone()}</div>
                                                <div class="build-wizard__item-source">{source}</div>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }
                    }).collect_view()}
                </Section>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::PlannedPick;

    use super::*;

    fn pick(kind: ContentKind, title: &str) -> PlannedPick {
        PlannedPick {
            provider: "modrinth".into(),
            kind,
            project_id: title.to_lowercase(),
            slug: title.to_lowercase(),
            title: title.into(),
            icon_url: None,
        }
    }

    #[test]
    fn the_picks_go_by_kind_in_the_tabs_order_and_empty_kinds_are_left_out() {
        let picks = [
            pick(ContentKind::ShaderPacks, "BSL"),
            pick(ContentKind::Mods, "Sodium"),
            pick(ContentKind::Mods, "Iris"),
        ];
        let groups = content_groups(&picks);
        let shown: Vec<(ContentKind, Vec<&str>)> =
            groups.iter().map(|(k, ps)| (*k, ps.iter().map(|p| p.title.as_str()).collect())).collect();
        assert_eq!(
            shown,
            [(ContentKind::Mods, vec!["Sodium", "Iris"]), (ContentKind::ShaderPacks, vec!["BSL"])]
        );
    }

    #[test]
    fn the_account_is_named_by_its_profile() {
        let profiles = [("steve".to_string(), "Steve".to_string()), ("alex".into(), "Alex".into())];
        assert_eq!(account_name(Some("alex"), &profiles).as_deref(), Some("Alex"));
        assert_eq!(account_name(None, &profiles), None, "the launcher's own");
        assert_eq!(
            account_name(Some("gone"), &profiles).as_deref(),
            Some("gone"),
            "a removed one by its key"
        );
    }
}
