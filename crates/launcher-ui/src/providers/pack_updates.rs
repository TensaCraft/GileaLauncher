//! Modpack updates on the Builds page: what each provider that updates modpacks says of the builds
//! it installed (`modpack_builds`), and the dialog that updates one to the version picked.

use std::collections::{HashMap, HashSet};

use launcher_shared::Level;
use launcher_shared::provider::{
    HeldFile, ModpackBuild, PackArgs, PackUpdateArgs, PackVersion, ProviderInfo, held_files,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Field, LatestRequest, Select, SelectOption, Variant, use_toasts};

use super::held::HeldDialog;
use super::{api, describe, open_page};

/// What the providers said of their modpack builds, by build key; and the builds updating now.
/// Arc signals in a thread local, so no page's disposal takes them along.
#[derive(Clone)]
pub struct PackUpdates {
    builds: ArcRwSignal<HashMap<String, (ProviderInfo, ModpackBuild)>>,
    updating: ArcRwSignal<HashSet<String>>,
}

pub fn updates() -> PackUpdates {
    thread_local! {
        static UPDATES: PackUpdates =
            PackUpdates { builds: ArcRwSignal::new(HashMap::new()), updating: ArcRwSignal::new(HashSet::new()) };
    }
    UPDATES.with(PackUpdates::clone)
}

impl PackUpdates {
    /// `provider`'s answer: its modpack builds now (its earlier ones go).
    pub fn found(&self, provider: &ProviderInfo, builds: Vec<ModpackBuild>) {
        self.builds.update(|all| {
            all.retain(|_, (p, _)| p.id != provider.id);
            for build in builds {
                all.insert(build.key.clone(), (provider.clone(), build));
            }
        });
    }

    /// Build `key`'s provider and modpack, when a newer version waits.
    pub fn update_of(&self, key: &str) -> Option<(ProviderInfo, ModpackBuild)> {
        self.builds.with(|all| all.get(key).filter(|(_, b)| b.newest.is_some()).cloned())
    }

    pub fn updating(&self, key: &str) -> bool {
        self.updating.with(|u| u.contains(key))
    }

    fn set_updating(&self, key: &str, on: bool) {
        self.updating.update(|u| {
            if on {
                u.insert(key.to_string());
            } else {
                u.remove(key);
            }
        });
    }
}

/// Asks each provider for its modpack builds; one that does not answer keeps what it said before.
pub fn load(providers: &[ProviderInfo]) {
    for provider in providers.iter().cloned() {
        spawn_local(async move {
            if let Ok(builds) = api::modpack_builds(&provider.id).await {
                updates().found(&provider, builds);
            }
        });
    }
}

/// "Update modpack <name>?" with its versions, the newest picked; the current one is marked.
#[component]
pub fn PackUpdateDialog(
    open: RwSignal<bool>,
    target: RwSignal<Option<(ProviderInfo, ModpackBuild)>>,
) -> impl IntoView {
    let (i18n, toasts) = (use_i18n(), use_toasts());
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let latest = StoredValue::new_local(LatestRequest::new());
    // `None` while loading; the error is already translated.
    let versions = RwSignal::new(None::<Result<Vec<PackVersion>, String>>);
    let picked = RwSignal::new(String::new());
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some((provider, build)) = target.get_untracked() else { return };
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        versions.set(None);
        spawn_local(async move {
            let answer =
                api::modpack_versions(&provider.id, &PackArgs { project_id: build.project_id.clone() })
                    .await
                    .map_err(|e| describe(i18n, &provider, &e));
            if !latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                return;
            }
            if let Ok(list) = &answer {
                let newest = build.newest.as_ref().map(|v| v.id.clone());
                let _ =
                    picked.try_set(newest.or_else(|| list.first().map(|v| v.id.clone())).unwrap_or_default());
            }
            let _ = versions.try_set(Some(answer));
        });
    });
    let options = Signal::derive(move || {
        let current = target.with(|t| t.as_ref().map(|(_, b)| b.version_id.clone())).unwrap_or_default();
        versions.with(|v| match v {
            Some(Ok(list)) => list
                .iter()
                .map(|v| {
                    let label = if v.id == current {
                        i18n.tp("modpack_current_version", &[("version", v.label())])
                    } else {
                        v.label()
                    };
                    SelectOption::new(v.id.clone(), label)
                })
                .collect(),
            _ => Vec::new(),
        })
    });
    let ready = move || {
        versions.with(|v| matches!(v, Some(Ok(_))))
            && target.with(|t| {
                t.as_ref().is_some_and(|(_, b)| picked.with(|p| !p.is_empty() && *p != b.version_id))
            })
    };
    // Files to download by hand before the update can go on, and the update that waits on them.
    let held = RwSignal::new(Vec::<HeldFile>::new());
    let waiting = StoredValue::new(None::<(ProviderInfo, PackUpdateArgs)>);
    let run = Callback::new(move |(provider, args): (ProviderInfo, PackUpdateArgs)| {
        updates().set_updating(&args.key, true);
        spawn_local(async move {
            let answer = api::update_modpack(&provider.id, &args).await;
            updates().set_updating(&args.key, false);
            match answer {
                Ok(_) => load(std::slice::from_ref(&provider)),
                Err(e) if !held_files(&e).is_empty() => {
                    let said = describe(i18n, &provider, &e);
                    waiting.try_set_value(Some((provider, args)));
                    // The page is gone by now: the user still hears which files to download.
                    if held.try_set(held_files(&e)).is_some() {
                        toasts.show(Level::Error, said, None);
                    }
                }
                Err(e) => toasts.show(Level::Error, describe(i18n, &provider, &e), None),
            }
        });
    });
    let go_on = Callback::new(move |()| {
        if let Some(waits) = waiting.try_update_value(Option::take).flatten() {
            run.run(waits);
        }
    });
    let open_held = Callback::new(move |url: String| {
        if let Some((provider, _)) = waiting.try_with_value(Clone::clone).flatten() {
            open_page(i18n, toasts, &provider, url);
        }
    });
    let held_by = Signal::derive(move || {
        held.track();
        waiting.try_with_value(|w| w.as_ref().map(|(p, _)| p.name.clone())).flatten().unwrap_or_default()
    });
    let update = move || {
        // Taken now: the page may be gone when the update ends.
        let Some((provider, build)) = target.get_untracked() else { return };
        let args = PackUpdateArgs { key: build.key.clone(), version_id: picked.get_untracked() };
        open.set(false);
        run.run((provider, args));
    };
    let title = Signal::derive(move || {
        let name = target.with(|t| t.as_ref().map(|(_, b)| b.name.clone())).unwrap_or_default();
        i18n.tp("modpack_update_title", &[("name", name)])
    });
    view! {
        <Dialog open=open title=title subtitle=t("modpack_update_note") icon="upgrade">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="upgrade"
                    disabled=Signal::derive(move || !ready())
                    on_click=move |_| update()
                >
                    {move || i18n.t("update_modrinth_content")}
                </Button>
            </DialogFooter>
            <Field
                label=t("modpack_version_label")
                hint=Signal::derive(move || versions.with(Option::is_none).then(|| i18n.t("loading_modpack_details")))
                error=Signal::derive(move || versions.with(|v| v.as_ref().and_then(|r| r.as_ref().err().cloned())))
            >
                <Select options=options value=picked disabled=Signal::derive(move || versions.with(|v| !matches!(v, Some(Ok(_))))) />
            </Field>
        </Dialog>
        <HeldDialog provider=held_by held=held on_continue=go_on on_open=open_held />
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::provider::{ModpackBuild, PackVersion, ProviderInfo};

    use super::*;

    fn provider(id: &str) -> ProviderInfo {
        ProviderInfo {
            id: id.into(),
            name: id.to_uppercase(),
            icon: "search".into(),
            content: Vec::new(),
            updates: Vec::new(),
            modpacks: true,
            modpack_updates: true,
        }
    }

    fn build(key: &str, newest: Option<&str>) -> ModpackBuild {
        ModpackBuild {
            key: key.into(),
            name: key.to_uppercase(),
            project_id: "SPEEDY".into(),
            version_id: "sp-1".into(),
            version_number: "1.0".into(),
            newest: newest.map(|id| PackVersion {
                id: id.into(),
                version_number: "2.0".into(),
                game_versions: vec!["1.21.1".into()],
                loaders: vec!["fabric".into()],
            }),
        }
    }

    #[test]
    fn a_build_offers_an_update_only_with_a_newer_version() {
        let owner = leptos::prelude::Owner::new();
        owner.with(|| {
            let u = updates();
            u.found(&provider("modrinth"), vec![build("aero", Some("sp-2")), build("zeta", None)]);
            assert_eq!(
                u.update_of("aero").map(|(p, b)| (p.id, b.newest.unwrap().id)),
                Some(("modrinth".into(), "sp-2".into()))
            );
            assert!(u.update_of("zeta").is_none(), "current");
            assert!(u.update_of("ghost").is_none());
            // A provider's new answer replaces only its own builds.
            u.found(&provider("curseforge"), vec![build("cf", Some("x"))]);
            u.found(&provider("modrinth"), vec![build("zeta", Some("sp-3"))]);
            assert!(u.update_of("aero").is_none(), "no longer listed by its provider");
            assert!(u.update_of("zeta").is_some() && u.update_of("cf").is_some());
        });
        owner.cleanup();
    }
}
