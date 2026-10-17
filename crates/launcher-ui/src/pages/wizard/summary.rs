//! «Підсумок»: what «Створити й встановити» will do — the game and its loader, the picked
//! content by kind, and the settings that differ from the launcher's defaults; picks a change of
//! version or type dropped are told here too.

use launcher_shared::{ContentKind, LoaderKind};
use leptos::prelude::*;
use ui_kit::Icon;
use ui_kit::i18n::use_i18n;

use super::{PicksDropped, Wizard};
use crate::builds::gpu::mode_label;

#[component]
pub fn Summary(wizard: Wizard) -> impl IntoView {
    let i18n = use_i18n();
    let state = wizard.state.get_untracked();
    let game = format!("Minecraft {}", state.mc);
    let loader = (state.loader != LoaderKind::Minecraft)
        .then(|| format!("{} {} · {}", state.loader.display_name(), state.loader_version, state.mc));
    let groups: Vec<(ContentKind, Vec<String>)> = ContentKind::ALL
        .into_iter()
        .map(|kind| (kind, state.picks.iter().filter(|p| p.kind == kind).map(|p| p.title.clone()).collect()))
        .filter(|(_, titles): &(ContentKind, Vec<String>)| !titles.is_empty())
        .collect();
    let label = move |kind: ContentKind| match kind {
        ContentKind::Mods => i18n.t("mods_content_tab"),
        ContentKind::ResourcePacks => i18n.t("resourcepacks"),
        ContentKind::ShaderPacks => i18n.t("shaders_content_tab"),
    };
    let mut settings: Vec<String> = Vec::new();
    if let Some(gb) = state.settings.max_ram_gb {
        settings.push(format!(
            "{}: {}",
            i18n.t("max_ram_label"),
            i18n.tp("setup_memory_gb", &[("value", gb.to_string())])
        ));
    }
    if let Some(gpu) = &state.settings.gpu_mode {
        settings.push(format!("{}: {}", i18n.t("gpu_mode_label"), i18n.t(mode_label(gpu))));
    }
    if state.settings.profile.is_some() {
        settings.push(i18n.t("wizard_account"));
    }
    if state.settings.image_path.is_some() {
        settings.push(i18n.t("wizard_icon"));
    }
    if !state.settings.description.trim().is_empty() {
        settings.push(format!(
            "{}: {}",
            i18n.t("build_description_label"),
            state.settings.description.trim()
        ));
    }
    let no_content = groups.is_empty();
    let no_settings = settings.is_empty();

    view! {
        <PicksDropped wizard=wizard />
        <dl class="sync-facts build-wizard__summary">
            <dt>{i18n.t("version_name_label")}</dt>
            <dd>{state.name.trim().to_string()}</dd>
            <dt>{i18n.t("wizard_summary_game")}</dt>
            <dd>{game}{loader.map(|l| view! { <br /><span class="hint">{l}</span> })}</dd>
            <dt>{i18n.t("wizard_summary_content")}</dt>
            <dd>
                {no_content.then(|| view! { <span class="hint">{i18n.t("wizard_plan_empty")}</span> })}
                {groups.into_iter().map(|(kind, titles)| view! {
                    <div class="build-wizard__group"><Icon name="check" /><b>{format!("{}:", label(kind))}</b>{format!(" {}", titles.join(", "))}</div>
                }).collect_view()}
            </dd>
            <dt>{i18n.t("wizard_summary_settings")}</dt>
            <dd>
                {no_settings.then(|| view! { <span class="hint">{i18n.t("wizard_settings_default")}</span> })}
                {settings.into_iter().map(|line| view! { <div>{line}</div> }).collect_view()}
            </dd>
        </dl>
    }
}
