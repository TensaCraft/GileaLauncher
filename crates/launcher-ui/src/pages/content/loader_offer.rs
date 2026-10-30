//! A build without a mod loader, in its mods and shader packs: the way to one. The loader goes
//! into the build for the build's own Minecraft version, and the game folder stays as it is.

use launcher_shared::{BuildDto, BuildSettingsDto, ContentKind, Level, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Icon, Size, Variant, ipc, use_toasts};

use super::{tab_icon, tab_of};
use crate::builds::build_minecraft;
use crate::builds::loader_pick::{LoaderBuildField, LoaderCards, LoaderPick};
use crate::pages::wizard::state::loader_available;
use crate::store::use_store;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SetLoaderArgs {
    key: String,
    loader: LoaderKind,
    loader_version: Option<String>,
}

/// The offer's title and line in a tab of `kind`.
pub fn offer_texts(kind: ContentKind) -> (&'static str, &'static str) {
    match kind {
        ContentKind::ShaderPacks => ("loader_offer_shaders_title", "loader_offer_shaders_desc"),
        _ => ("loader_offer_mods_title", "loader_offer_mods_desc"),
    }
}

#[component]
pub fn LoaderOffer(key: String, kind: ContentKind, current: Signal<Option<BuildDto>>) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let toasts = use_toasts();
    let pick = LoaderPick::new(LoaderKind::Fabric);
    pick.load();
    let mc = Signal::derive(move || current.with(|b| b.as_ref().map(build_minecraft).unwrap_or_default()));
    let installing = RwSignal::new(false);
    let busy = Memo::new(move |_| store.ops.with(|o| o.busy));
    // A loader with a build for the version, and its build picked.
    let ready = move || {
        let (kind, mc) = (pick.kind(), mc.get());
        kind != LoaderKind::Minecraft
            && !pick.build.with(String::is_empty)
            && pick.catalogs.with(|all| loader_available(kind, &mc, all))
    };
    let install = StoredValue::new(move || {
        let args = SetLoaderArgs {
            key: key.clone(),
            loader: pick.kind(),
            loader_version: Some(pick.build.get_untracked()),
        };
        installing.set(true);
        // Once installed, the build has a loader and its list shows here instead.
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, BuildSettingsDto>("build_set_loader", &args).await {
                toasts.show(
                    Level::Error,
                    i18n.tp("version_component_install_failed", &[("error", i18n.error(&e))]),
                    None,
                );
            }
            let _ = installing.try_set(false);
        });
    });
    let (title, desc) = offer_texts(kind);
    let label =
        move || i18n.tp("loader_offer_install", &[("loader", pick.kind().display_name().to_string())]);

    view! {
        <div class="loader-offer">
            <div class="empty loader-offer__head">
                <div class="empty__icon"><Icon name=tab_icon(tab_of(kind)) outlined=true /></div>
                <div class="empty__title">{move || i18n.t(title)}</div>
                <div class="empty__desc">{move || i18n.tp(desc, &[("mc", mc.get())])}</div>
            </div>
            <LoaderCards pick=pick mc=mc vanilla=false />
            <div class="loader-offer__foot">
                <div class="grow"><LoaderBuildField pick=pick mc=mc /></div>
                <Button
                    size=Size::Md
                    variant=Variant::Primary
                    icon="download"
                    loading=Signal::derive(move || installing.get())
                    disabled=Signal::derive(move || busy.get() || !ready())
                    on_click=move |_| install.with_value(|run| run())
                >
                    {label}
                </Button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_packs_say_why_they_need_a_loader() {
        assert_eq!(offer_texts(ContentKind::Mods), ("loader_offer_mods_title", "loader_offer_mods_desc"));
        assert_eq!(
            offer_texts(ContentKind::ShaderPacks),
            ("loader_offer_shaders_title", "loader_offer_shaders_desc")
        );
    }
}
