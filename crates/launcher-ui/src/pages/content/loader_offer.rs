//! A build without a mod loader, in its mods and shader packs: the way to one, the loader it had
//! before first. The loader goes into the build for the build's own Minecraft version, and the
//! game folder stays as it is: mods left in it from before are counted, and asked about.

use launcher_shared::{BuildDto, BuildSettingsDto, ContentKind, Level, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, ConfirmDialog, Icon, Size, Variant, ipc, use_toasts};

use super::{tab_icon, tab_of};
use crate::builds::actions::use_build_actions;
use crate::builds::build_minecraft;
use crate::builds::loader_pick::{LoaderBuildField, LoaderCards, LoaderPick, leftover_warning};
use crate::pages::wizard::state::loader_available;
use crate::store::use_store;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SetLoaderArgs {
    key: String,
    loader: LoaderKind,
    loader_version: Option<String>,
}

/// The loader offered first: the one the build had before it went back to vanilla, else Fabric.
pub fn offered_first(last: Option<LoaderKind>) -> LoaderKind {
    last.filter(|kind| *kind != LoaderKind::Minecraft).unwrap_or(LoaderKind::Fabric)
}

/// The note above the mods' offer: how many mod files the mods folder still holds, if any.
pub fn leftover_note(kind: ContentKind, count: u32) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    (kind == ContentKind::Mods && count > 0)
        .then(|| ("loader_offer_leftover_mods", vec![("count", count.to_string())]))
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
    let pick =
        LoaderPick::new(offered_first(current.with_untracked(|b| b.as_ref().and_then(|b| b.last_loader))));
    pick.load();
    // The build again: the mods its folder holds now.
    use_build_actions().refresh();
    let mc = Signal::derive(move || current.with(|b| b.as_ref().map(build_minecraft).unwrap_or_default()));
    let leftover = Memo::new(move |_| current.with(|b| b.as_ref().map_or(0, |b| b.leftover_mods)));
    let installing = RwSignal::new(false);
    let confirm = RwSignal::new(false);
    let busy = Memo::new(move |_| store.ops.with(|o| o.busy));
    // A loader with a build for the version, and its build picked.
    let ready = move || {
        let (kind, mc) = (pick.kind(), mc.get());
        kind != LoaderKind::Minecraft
            && !pick.build.with(String::is_empty)
            && pick.catalogs.with(|all| loader_available(kind, &mc, all))
    };
    let install = Callback::new(move |()| {
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
    // What the mods left in the folder risk under the picked loader, in the language.
    let warning = Memo::new(move |_| {
        leftover_warning(leftover.get(), pick.kind()).map(|(key, words)| i18n.tp(key, &words))
    });
    let ask = move || {
        if warning.with_untracked(Option::is_some) {
            confirm.set(true);
        } else {
            install.run(());
        }
    };
    let note = move || leftover_note(kind, leftover.get()).map(|(key, words)| i18n.tp(key, &words));
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
            {move || note().map(|text| view! {
                <div class="wizard__status is-warn">
                    <Icon name="warning_amber" />
                    <span class="grow">{text}</span>
                </div>
            })}
            <LoaderCards pick=pick mc=mc vanilla=false />
            <div class="loader-offer__foot">
                <div class="grow"><LoaderBuildField pick=pick mc=mc /></div>
                <Button
                    size=Size::Md
                    variant=Variant::Primary
                    icon="download"
                    loading=Signal::derive(move || installing.get())
                    disabled=Signal::derive(move || busy.get() || !ready())
                    on_click=move |_| ask()
                >
                    {label}
                </Button>
            </div>
        </div>
        <ConfirmDialog
            open=confirm
            title=Signal::derive(move || i18n.t("build_loader_confirm_title"))
            message=Signal::derive(move || warning.get().unwrap_or_default())
            confirm_label=Signal::derive(label)
            cancel_label=Signal::derive(move || i18n.t("cancel"))
            on_confirm=install
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loader_a_build_had_is_offered_first() {
        assert_eq!(offered_first(Some(LoaderKind::NeoForge)), LoaderKind::NeoForge);
        assert_eq!(offered_first(None), LoaderKind::Fabric);
        assert_eq!(offered_first(Some(LoaderKind::Minecraft)), LoaderKind::Fabric);
    }

    #[test]
    fn the_mods_offer_counts_the_mods_left_from_before() {
        assert_eq!(
            leftover_note(ContentKind::Mods, 4),
            Some(("loader_offer_leftover_mods", vec![("count", "4".to_string())]))
        );
        assert_eq!(leftover_note(ContentKind::Mods, 0), None);
        assert_eq!(
            leftover_note(ContentKind::ShaderPacks, 4),
            None,
            "the shaders' offer says nothing of mods"
        );
    }

    #[test]
    fn shader_packs_say_why_they_need_a_loader() {
        assert_eq!(offer_texts(ContentKind::Mods), ("loader_offer_mods_title", "loader_offer_mods_desc"));
        assert_eq!(
            offer_texts(ContentKind::ShaderPacks),
            ("loader_offer_shaders_title", "loader_offer_shaders_desc")
        );
    }
}
