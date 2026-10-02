//! The content providers' UI, one for every provider (`ContentProvider`): a source on the
//! "Build content" tabs, its parts of the Installed list, the dependency dialog, the update
//! confirmation and the modpacks with their install dialog. What a provider offers
//! (`ProviderInfo`) decides where it shows.

pub mod api;
pub mod cards;
pub mod dependencies;
pub mod flow;
pub mod held;
pub mod installed;
pub mod modpacks;
pub mod pack_updates;
pub mod panel;
pub mod store;

use launcher_shared::provider::{Need, ProviderInfo, provider_error_key};
use launcher_shared::{AppError, AppInfo, ContentKind, Level, mods_supported};
use leptos::task::spawn_local;
use serde::Serialize;
use ui_kit::i18n::I18nCtx;
use ui_kit::{Toasts, ipc};

/// The providers this build has (`app_info`).
pub fn providers(info: Option<&AppInfo>) -> Vec<ProviderInfo> {
    info.map(|i| i.providers.clone()).unwrap_or_default()
}

/// Who offers `kind` for a build of `client` — mods and shader packs need a mod loader.
pub fn content_providers(all: &[ProviderInfo], kind: ContentKind, client: Option<&str>) -> Vec<ProviderInfo> {
    if kind != ContentKind::ResourcePacks && !mods_supported(client) {
        return Vec::new();
    }
    all.iter().filter(|p| p.offers(Need::Content(kind))).cloned().collect()
}

/// Who offers modpacks.
pub fn modpack_providers(all: &[ProviderInfo]) -> Vec<ProviderInfo> {
    all.iter().filter(|p| p.offers(Need::Modpacks)).cloned().collect()
}

/// Who updates the builds it installed from modpacks.
pub fn modpack_update_providers(all: &[ProviderInfo]) -> Vec<ProviderInfo> {
    all.iter().filter(|p| p.offers(Need::ModpackUpdates)).cloned().collect()
}

/// A provider's error as the user reads it: the provider-named text when there is one.
pub fn describe(i18n: I18nCtx, provider: &ProviderInfo, e: &AppError) -> String {
    match provider_error_key(e) {
        Some(key) => {
            let mut params: Vec<(&str, String)> =
                e.params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            params.push(("provider", provider.name.clone()));
            i18n.tp(key, &params)
        }
        None => i18n.error(e),
    }
}

#[derive(Serialize)]
struct UrlArgs {
    url: String,
}

/// Opens a page of `provider`'s site in the system browser; a failure becomes a toast.
pub fn open_page(i18n: I18nCtx, toasts: Toasts, provider: &ProviderInfo, url: String) {
    let name = provider.name.clone();
    spawn_local(async move {
        if ipc::invoke::<_, ()>("open_url", &UrlArgs { url }).await.is_err() {
            toasts.show(Level::Error, i18n.tp("provider_open_failed", &[("provider", name)]), None);
        }
    });
}

#[cfg(test)]
mod wording_tests {
    use launcher_shared::ContentKind;
    use launcher_shared::provider::{installed_key, installing_key, not_found_key, search_key, update_texts};

    fn lang(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    /// Every text a resource-pack or shader tab can show.
    fn pack_tab_keys(kind: ContentKind) -> Vec<&'static str> {
        let u = update_texts(kind);
        vec![
            search_key(kind),
            not_found_key(kind),
            installing_key(kind),
            installed_key(kind),
            kind.toggled_key(true),
            kind.toggled_key(false),
            kind.deleted_key(),
            u.action,
            u.confirm,
            u.done,
            u.idle,
            u.checking,
            u.current,
            u.none,
            "content_filtered_by_version",
            "installed_updates_unchecked",
            "no_compatible_version",
            "no_file_found",
            "modrinth_dependency_open",
        ]
    }

    #[test]
    fn pack_tabs_never_say_mods() {
        let (uk, en) = (
            lang(include_str!("../../../../assets/langs/uk_UA.json")),
            lang(include_str!("../../../../assets/langs/en_US.json")),
        );
        for kind in [ContentKind::ResourcePacks, ContentKind::ShaderPacks] {
            for key in pack_tab_keys(kind) {
                let (u, e) = (uk[key].as_str().unwrap_or_default(), en[key].as_str().unwrap_or_default());
                assert!(!u.is_empty() && !e.is_empty(), "{key} is missing");
                assert!(!u.to_lowercase().contains("мод"), "{key}: {u}");
                let words: Vec<String> =
                    e.to_lowercase().split(|c: char| !c.is_alphabetic()).map(str::to_string).collect();
                assert!(!words.iter().any(|w| w == "mod" || w == "mods"), "{key}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::ContentKind;
    use launcher_shared::provider::ProviderInfo;

    use super::*;

    fn provider(id: &str, content: Vec<ContentKind>, modpacks: bool) -> ProviderInfo {
        ProviderInfo {
            id: id.into(),
            name: id.to_uppercase(),
            icon: "search".into(),
            content,
            updates: vec![ContentKind::Mods],
            modpacks,
            modpack_updates: modpacks,
        }
    }

    #[test]
    fn every_provider_of_a_kind_is_offered_in_module_order() {
        let all = [
            provider("modrinth", ContentKind::ALL.to_vec(), true),
            provider("curseforge", vec![ContentKind::Mods], false),
        ];
        let ids = |kind, client| {
            content_providers(&all, kind, client).into_iter().map(|p| p.id).collect::<Vec<_>>()
        };
        assert_eq!(ids(ContentKind::Mods, Some("Fabric")), ["modrinth", "curseforge"]);
        assert_eq!(ids(ContentKind::ShaderPacks, Some("Fabric")), ["modrinth"], "only who offers the kind");
        assert!(ids(ContentKind::Mods, Some("Minecraft")).is_empty(), "vanilla has no mods");
        assert_eq!(ids(ContentKind::ResourcePacks, Some("Minecraft")), ["modrinth"]);
        assert!(
            content_providers(&[], ContentKind::Mods, Some("Fabric")).is_empty(),
            "no backend, no source"
        );
        let packs: Vec<String> = modpack_providers(&all).into_iter().map(|p| p.id).collect();
        assert_eq!(packs, ["modrinth"]);
        assert!(modpack_providers(&[]).is_empty());
    }

    #[test]
    fn provider_texts_name_their_provider() {
        for raw in [
            include_str!("../../../../assets/langs/uk_UA.json"),
            include_str!("../../../../assets/langs/en_US.json"),
        ] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
            for key in [
                "provider_unreachable",
                "provider_file_mismatch",
                "provider_dependencies_title",
                "provider_dependencies_message",
                "provider_open_failed",
                "installed_updates_unchecked",
            ] {
                let text = map.get(key).and_then(|v| v.as_str()).unwrap_or_default();
                assert!(text.contains("{provider}"), "{key}: {text}");
            }
        }
    }
}
