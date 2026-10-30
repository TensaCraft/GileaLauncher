//! "Modpacks": the modpacks of the content providers that offer them —
//! Modrinth's now, CurseForge's and Tensa's later — with a switch between them when there are
//! several.

use launcher_shared::provider::ProviderInfo;
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{DropZone, EmptyState, SegOption, Segmented};

use crate::providers::modpacks::ProviderModpacks;
use crate::providers::{modpack_providers, providers};
use crate::shell::PageHeader;
use crate::store::use_store;

pub const MODPACKS_PATH: &str = "/modpacks";

/// The provider whose modpack file `path` is, by its ending (`ProviderInfo::pack_file`).
pub fn pack_owner<'a>(offered: &'a [ProviderInfo], path: &str) -> Option<&'a ProviderInfo> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    offered.iter().find(|p| p.pack_file.as_ref().is_some_and(|ending| ui_kit::drop::takes(name, &[ending])))
}

/// The endings of the modpack files the offered providers take.
pub fn pack_endings(offered: &[ProviderInfo]) -> Vec<String> {
    offered.iter().filter_map(|p| p.pack_file.clone()).collect()
}

/// A dropped modpack's name for its build: its file's, without the ending.
pub fn pack_title(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_string()
}

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
    // A pack file dropped on the page: the provider whose files end so shows its install dialog.
    let dropped = RwSignal::new(None::<(String, String)>);
    let on_dropped = Callback::new(move |paths: Vec<String>| {
        let Some(path) = paths.into_iter().next() else { return };
        if let Some(id) = offered.with_untracked(|ps| pack_owner(ps, &path).map(|p| p.id.clone())) {
            picked.set(id.clone());
            dropped.set(Some((id, path)));
        }
    });
    let endings = Signal::derive(move || offered.with(|ps| pack_endings(ps)));
    let body = move || {
        let id = active.get();
        let sources = sources.clone();
        match offered.with(|ps| ps.iter().find(|p| Some(&p.id) == id.as_ref()).cloned()) {
            Some(provider) => {
                view! { <ProviderModpacks provider=provider sources=sources dropped=dropped /> }.into_any()
            }
            None => view! {
                <EmptyState icon="webhook" title=Signal::derive(move || i18n.t("modpacks_no_providers")) />
            }
            .into_any(),
        }
    };
    view! {
        <PageHeader title_key="modpacks_title" />
        <DropZone accept=endings label=Signal::derive(move || i18n.t("drop_modpack")) on_drop=on_dropped class="create">
            {body}
        </DropZone>
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

    fn provider(id: &str, pack_file: Option<&str>) -> ProviderInfo {
        ProviderInfo {
            id: id.into(),
            name: id.into(),
            icon: String::new(),
            content: Vec::new(),
            updates: Vec::new(),
            modpacks: true,
            modpack_updates: false,
            content_clients: Vec::new(),
            content_label: None,
            pack_file: pack_file.map(str::to_string),
        }
    }

    #[test]
    fn a_dropped_pack_goes_to_the_provider_of_its_ending() {
        let offered = [
            provider("modrinth", Some("mrpack")),
            provider("curseforge", Some("zip")),
            provider("tensa", None),
        ];
        let owner = |path: &str| pack_owner(&offered, path).map(|p| p.id.as_str());
        assert_eq!(owner(r"C:\cache\drops\a\Fabulously Optimized.MRPACK"), Some("modrinth"));
        assert_eq!(owner("/tmp/drops/b/All the Mods 9.zip"), Some("curseforge"));
        assert_eq!(owner("/tmp/drops/c/notes.txt"), None);
        assert_eq!(owner("/tmp/drops/d/mrpack"), None, "an ending alone");
        assert_eq!(pack_endings(&offered), ["mrpack", "zip"]);
    }

    #[test]
    fn a_dropped_pack_is_named_after_its_file() {
        assert_eq!(pack_title("/tmp/drops/b/All the Mods 9 - 0.3.2.zip"), "All the Mods 9 - 0.3.2");
        assert_eq!(pack_title(r"C:\drops\Speedy.mrpack"), "Speedy");
        assert_eq!(pack_title("noext"), "noext");
    }
}
