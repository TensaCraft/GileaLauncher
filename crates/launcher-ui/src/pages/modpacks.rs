//! "Modpacks": the modpacks of the content providers that offer them —
//! Modrinth's now, CurseForge's and Tensa's later — with a switch between them when there are
//! several.

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{EmptyState, SegOption, Segmented};

use crate::providers::modpacks::ProviderModpacks;
use crate::providers::{modpack_providers, providers};
use crate::shell::PageHeader;
use crate::store::use_store;

pub const MODPACKS_PATH: &str = "/modpacks";

/// The picked source when it is offered, otherwise the first one offered.
pub fn active_source<'a>(offered: &[&'a str], picked: &str) -> Option<&'a str> {
    offered.iter().find(|id| **id == picked).or(offered.first()).copied()
}

#[component]
pub fn ModpacksPage() -> impl IntoView {
    let (store, i18n) = (use_store(), use_i18n());
    let offered = Memo::new(move |_| modpack_providers(&store.info.with(|i| providers(i.as_ref()))));
    let picked = RwSignal::new(String::new());
    // The switch shows the source the page shows, from the start.
    Effect::new(move |_| {
        if let Some(first) = offered.with(|o| o.first().map(|p| p.id.clone()))
            && picked.with_untracked(String::is_empty)
        {
            picked.set(first);
        }
    });
    let active = Memo::new(move |_| {
        offered.with(|ps| {
            let ids: Vec<&str> = ps.iter().map(|p| p.id.as_str()).collect();
            picked.with(|p| active_source(&ids, p).map(str::to_string))
        })
    });
    let options = Signal::derive(move || {
        offered.get().into_iter().map(|p| SegOption::new(p.id, p.name).with_icon(p.icon)).collect::<Vec<_>>()
    });
    // The switch between sources sits in the list's own bar, right after its search.
    let sources = ViewFn::from(move || {
        view! {
            <Show when=move || offered.with(|ps| ps.len() > 1)>
                <Segmented options=options value=picked />
            </Show>
        }
    });
    let body = move || {
        let id = active.get();
        let sources = sources.clone();
        match offered.with(|ps| ps.iter().find(|p| Some(&p.id) == id.as_ref()).cloned()) {
            Some(provider) => view! { <ProviderModpacks provider=provider sources=sources /> }.into_any(),
            None => view! {
                <EmptyState icon="webhook" title=Signal::derive(move || i18n.t("modpacks_no_providers")) />
            }
            .into_any(),
        }
    };
    view! {
        <PageHeader title_key="modpacks_title" />
        <div class="create">{body}</div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_gone_falls_back_to_the_first() {
        assert_eq!(active_source(&["modrinth", "curseforge"], "curseforge"), Some("curseforge"));
        assert_eq!(active_source(&["modrinth"], "curseforge"), Some("modrinth"), "a source gone falls back");
        assert_eq!(active_source(&["modrinth"], ""), Some("modrinth"));
        assert_eq!(active_source(&[], ""), None);
    }
}
