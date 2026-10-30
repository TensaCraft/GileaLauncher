//! Choosing a build's mod loader: the wizard's cards (vanilla, Fabric, NeoForge, Forge, Quilt),
//! each saying whether it has a build for the Minecraft version, and the loader's own version.
//! The wizard, a vanilla build's mods and the build's settings share them; the settings start on
//! the build's own pick and never leave it by themselves.

use launcher_shared::{BuildDto, CatalogVersion, LoaderBuild, LoaderKind};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Button, ChoiceCards, ChoiceOption, Field, Icon, Select, SelectOption, Variant};

use super::catalog::loader_catalog;
use super::{build_loader, build_minecraft, loader_icon};
use crate::pages::wizard::state::{Catalogs, LoaderState, default_loader_version, loader_state};

/// The mod loaders on offer, after vanilla.
pub const LOADERS: [LoaderKind; 4] =
    [LoaderKind::Fabric, LoaderKind::NeoForge, LoaderKind::Forge, LoaderKind::Quilt];

/// A loader as the cards name it.
pub fn loader_value(kind: LoaderKind) -> String {
    kind.display_name().to_lowercase()
}

pub fn loader_of(value: &str) -> LoaderKind {
    LOADERS.into_iter().find(|k| loader_value(*k) == value).unwrap_or(LoaderKind::Minecraft)
}

pub fn loader_mark(kind: LoaderKind) -> &'static str {
    loader_icon(Some(kind.display_name()))
}

/// The line under a loader's card.
pub fn loader_hint(state: &LoaderState) -> &'static str {
    match state {
        LoaderState::Available => "wizard_loader_desc",
        LoaderState::Loading => "wizard_loading",
        LoaderState::Failed(_) => "wizard_loader_failed_hint",
        LoaderState::Missing => "wizard_loader_unavailable_hint",
    }
}

/// The cards for Minecraft `mc`: vanilla first when offered, then each loader; one without a build
/// there is shown but not to be chosen. `t` translates.
pub fn loader_cards(
    vanilla: bool,
    mc: &str,
    catalogs: &Catalogs,
    t: impl Fn(&str) -> String,
) -> Vec<ChoiceOption> {
    let plain = ChoiceOption::new(
        loader_value(LoaderKind::Minecraft),
        t("wizard_vanilla"),
        loader_mark(LoaderKind::Minecraft),
    )
    .with_desc(t("wizard_vanilla_desc"));
    let loaders = LOADERS.into_iter().map(|kind| {
        let state = loader_state(kind, mc, catalogs);
        ChoiceOption::new(loader_value(kind), kind.display_name(), loader_mark(kind))
            .with_desc(t(loader_hint(&state)))
            .with_disabled(state != LoaderState::Available)
    });
    vanilla.then_some(plain).into_iter().chain(loaders).collect()
}

/// The build's own loader, Minecraft and loader build (empty for vanilla): its settings start on
/// them and never leave them by themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnPick {
    pub kind: LoaderKind,
    pub mc: String,
    pub build: String,
}

impl OwnPick {
    pub fn of(build: &BuildDto) -> OwnPick {
        let kind = build_loader(build);
        let lv = build.loader_version.as_deref().map(str::trim).unwrap_or_default();
        let lv = if kind == LoaderKind::Minecraft { String::new() } else { lv.to_string() };
        OwnPick { kind, mc: build_minecraft(build), build: lv }
    }

    /// Loader `kind` for Minecraft `mc` is the build's own.
    fn is(&self, kind: LoaderKind, mc: &str) -> bool {
        self.kind == kind && self.mc == mc
    }
}

/// The words that mark a prerelease of a loader or of Minecraft.
const PRERELEASE_WORDS: [&str; 5] = ["alpha", "beta", "pre", "rc", "snapshot"];

/// The numbers in `text`, in order ("0.17.0-beta.1" → 0, 17, 0, 1).
fn numbers(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
}

/// A version as it compares: its numbers before a prerelease word, a release over its
/// prereleases, then the numbers after the word.
fn version_key(version: &str) -> (Vec<u64>, bool, Vec<u64>) {
    let lower = version.to_lowercase();
    let cut = PRERELEASE_WORDS.iter().filter_map(|word| lower.find(word)).min();
    let (base, rest) = lower.split_at(cut.unwrap_or(lower.len()));
    (numbers(base), cut.is_none(), numbers(rest))
}

/// A loader build that is a prerelease (`0.17.0-beta.1`, `21.1.0-rc2`).
pub fn is_beta(version: &str) -> bool {
    !version.trim().is_empty() && !version_key(version).1
}

/// `to` came before `from`: by their places in `newest_first` when it has both, else by their
/// numbers.
fn older(to: &str, from: &str, newest_first: &[&str]) -> bool {
    let at = |v: &str| newest_first.iter().position(|listed| *listed == v);
    match (at(to), at(from)) {
        (Some(to), Some(from)) => to > from,
        _ => version_key(to) < version_key(from),
    }
}

/// The builds of `kind` for Minecraft `mc`, prereleases marked with `beta`; the build's own
/// (`own`) first when the catalog does not list it.
pub fn build_options(
    kind: LoaderKind,
    mc: &str,
    catalogs: &Catalogs,
    beta: &str,
    own: Option<&OwnPick>,
) -> Vec<SelectOption> {
    let builds =
        catalogs.get(&kind).and_then(|c| c.as_ref().ok()).map(|c| c.builds_for(mc)).unwrap_or_default();
    let label = |version: &str, stable: bool| {
        if stable { version.to_string() } else { format!("{version} · {beta}") }
    };
    let mut options: Vec<SelectOption> =
        builds.iter().map(|b| SelectOption::new(b.version.clone(), label(&b.version, b.stable))).collect();
    let unlisted = |o: &&OwnPick| !builds.iter().any(|b| b.version == o.build);
    if let Some(own) = own.filter(|o| o.is(kind, mc) && !o.build.is_empty()).filter(unlisted) {
        options.insert(0, SelectOption::new(own.build.clone(), label(&own.build, !is_beta(&own.build))));
    }
    options
}

/// The loader to show once the catalogs tell more: `picked` while it has a build for `mc` (vanilla
/// always has) or its catalog is on its way, else the first loader that has one, else vanilla
/// where it is offered. A pick that starts on the build's own (`own`) stays where the player puts
/// it: a catalog that could not be read or has no stable build says so, and nothing installs.
pub fn settled_loader(
    picked: LoaderKind,
    mc: &str,
    catalogs: &Catalogs,
    vanilla: bool,
    own: Option<&OwnPick>,
) -> LoaderKind {
    if own.is_some()
        || matches!(loader_state(picked, mc, catalogs), LoaderState::Available | LoaderState::Loading)
    {
        return picked;
    }
    let available =
        LOADERS.into_iter().find(|kind| loader_state(*kind, mc, catalogs) == LoaderState::Available);
    available.unwrap_or(if vanilla { LoaderKind::Minecraft } else { picked })
}

/// The loader build to show: `current` while `kind` offers it for `mc` or its catalog has not
/// told, else the build's own (`own`) for its own loader and Minecraft, else the recommended one
/// (none for vanilla).
pub fn settled_build(
    kind: LoaderKind,
    mc: &str,
    current: &str,
    catalogs: &Catalogs,
    own: Option<&OwnPick>,
) -> String {
    if kind == LoaderKind::Minecraft {
        return String::new();
    }
    match catalogs.get(&kind) {
        Some(Ok(catalog)) if !catalog.builds_for(mc).iter().any(|b| b.version == current) => {
            match own.filter(|o| o.is(kind, mc) && !o.build.is_empty()) {
                Some(own) => own.build.clone(),
                None => default_loader_version(kind, mc, catalogs),
            }
        }
        _ => current.to_string(),
    }
}

/// A translation key and its words.
pub type Said = (&'static str, Vec<(&'static str, String)>);

/// What putting loader `to` on a vanilla build risks for the `count` mod files left in its mods
/// folder: mods made for another loader may not load.
pub fn leftover_warning(count: u32, to: LoaderKind) -> Option<Said> {
    (count > 0 && to != LoaderKind::Minecraft).then(|| {
        (
            "build_loader_leftover_warning",
            vec![("count", count.to_string()), ("to", to.display_name().into())],
        )
    })
}

/// What installing `to` over the build's own `from` risks, each as a translation key and its
/// words: mods of another loader, the `leftover` mods of one before, a Minecraft of another time
/// for its worlds (`versions`, Minecraft's catalog, newest first), an older build of its loader for
/// its mods (`offered`, the loader's builds for the version, newest first).
pub fn pick_warnings(
    from: &OwnPick,
    to: &OwnPick,
    leftover: u32,
    versions: &[CatalogVersion],
    offered: &[LoaderBuild],
) -> Vec<Said> {
    let mut said = Vec::new();
    if from.kind != to.kind {
        said.extend(change_warning(from.kind, to.kind));
    }
    if from.kind == LoaderKind::Minecraft {
        said.extend(leftover_warning(leftover, to.kind));
    }
    if !from.mc.is_empty() && from.mc != to.mc {
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        let key =
            if older(&to.mc, &from.mc, &ids) { "build_mc_older_warning" } else { "build_mc_newer_warning" };
        said.push((key, vec![("from", from.mc.clone()), ("to", to.mc.clone())]));
    }
    let same_loader = from.kind == to.kind && from.mc == to.mc && to.kind != LoaderKind::Minecraft;
    if same_loader && !from.build.is_empty() && from.build != to.build {
        let builds: Vec<&str> = offered.iter().map(|b| b.version.as_str()).collect();
        if older(&to.build, &from.build, &builds) {
            let words = vec![
                ("loader", to.kind.display_name().to_string()),
                ("from", from.build.clone()),
                ("to", to.build.clone()),
            ];
            said.push(("build_loader_older_warning", words));
        }
    }
    said
}

/// What changing the loader from `from` to `to` risks for the build's mods, as a translation key
/// and its words: mods made for one loader do not load under another, nor under vanilla.
pub fn change_warning(
    from: LoaderKind,
    to: LoaderKind,
) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let name = |kind: LoaderKind| kind.display_name().to_string();
    match (from, to) {
        (LoaderKind::Minecraft, _) => None,
        (from, to) if from == to => None,
        (from, LoaderKind::Minecraft) => Some(("build_loader_vanilla_warning", vec![("from", name(from))])),
        (from, to) => Some(("build_loader_change_warning", vec![("from", name(from)), ("to", name(to))])),
    }
}

/// The loaders whose catalog could not be read, in the cards' order, with the first one's reason.
pub fn unread_loaders(catalogs: &Catalogs) -> Option<(Vec<LoaderKind>, String)> {
    let unread: Vec<(LoaderKind, &String)> =
        LOADERS.into_iter().filter_map(|k| catalogs.get(&k)?.as_ref().err().map(|e| (k, e))).collect();
    let reason = unread.first()?.1.clone();
    Some((unread.into_iter().map(|(k, _)| k).collect(), reason))
}

/// `value` into `field`, waking those who read it only when it is another.
pub fn follow<T: PartialEq + Send + Sync + 'static>(field: RwSignal<T>, value: T) {
    field.maybe_update(|shown| {
        let changed = *shown != value;
        if changed {
            *shown = value;
        }
        changed
    });
}

/// A loader choice: every loader's catalog, the picked loader (`loader_value`) and its build;
/// `unstable` asks the catalogs with their prereleases, and `own` keeps the pick on the build's
/// own until the player moves it.
#[derive(Clone, Copy)]
pub struct LoaderPick {
    pub catalogs: RwSignal<Catalogs>,
    pub loader: RwSignal<String>,
    pub build: RwSignal<String>,
    pub unstable: RwSignal<bool>,
    pub own: RwSignal<Option<OwnPick>>,
    i18n: I18nCtx,
}

impl LoaderPick {
    pub fn new(kind: LoaderKind) -> LoaderPick {
        LoaderPick {
            catalogs: RwSignal::new(Catalogs::new()),
            loader: RwSignal::new(loader_value(kind)),
            build: RwSignal::new(String::new()),
            unstable: RwSignal::new(false),
            own: RwSignal::new(None),
            i18n: use_i18n(),
        }
    }

    /// Starts on the build's own pick, with the prereleases shown when it runs one.
    pub fn start_on(&self, own: OwnPick) {
        self.loader.set(loader_value(own.kind));
        self.build.set(own.build.clone());
        self.unstable.set(is_beta(&own.build));
        self.own.set(Some(own));
    }

    pub fn kind(&self) -> LoaderKind {
        self.loader.with(|value| loader_of(value))
    }

    /// Every loader's catalog; one that cannot be read leaves the others.
    pub fn load(&self) {
        for kind in LOADERS {
            self.load_one(kind);
        }
    }

    /// `kind`'s catalog (asked again after a failure: its card shows it on its way meanwhile).
    pub fn load_one(&self, kind: LoaderKind) {
        let (catalogs, i18n, unstable) = (self.catalogs, self.i18n, self.unstable.get_untracked());
        catalogs.update(|all| {
            all.remove(&kind);
        });
        spawn_local(async move {
            let answer = loader_catalog(kind, unstable).await.map_err(|e| i18n.error(&e));
            let _ = catalogs.try_update(|all| all.insert(kind, answer));
        });
    }

    /// The loader and its build to show for Minecraft `mc` now.
    fn settle(&self, mc: &str, vanilla: bool) {
        let own = self.own.get_untracked();
        let (kind, build) = self.catalogs.with(|all| {
            let kind = settled_loader(self.kind(), mc, all, vanilla, own.as_ref());
            (kind, settled_build(kind, mc, &self.build.get_untracked(), all, own.as_ref()))
        });
        follow(self.loader, loader_value(kind));
        follow(self.build, build);
    }
}

/// The loader cards for Minecraft `mc` (with vanilla's when `vanilla`), and why catalogs could not
/// be read with a way to ask again. The picked loader and its build follow the catalogs.
#[component]
pub fn LoaderCards(pick: LoaderPick, #[prop(into)] mc: Signal<String>, vanilla: bool) -> impl IntoView {
    let i18n = use_i18n();
    Effect::new(move |_| {
        let mc = mc.get();
        pick.catalogs.track();
        pick.loader.track();
        pick.settle(&mc, vanilla);
    });
    let cards = Signal::derive(move || {
        let mc = mc.get();
        pick.catalogs.with(|all| loader_cards(vanilla, &mc, all, |key| i18n.t(key)))
    });
    let columns = LOADERS.len() + usize::from(vanilla);
    let unread = Memo::new(move |_| pick.catalogs.with(unread_loaders));
    let retry = move || {
        for kind in unread.get_untracked().map(|(kinds, _)| kinds).unwrap_or_default() {
            pick.load_one(kind);
        }
    };
    view! {
        <ChoiceCards options=cards value=pick.loader columns=columns label=Signal::derive(move || i18n.t("wizard_type")) />
        {move || unread.get().map(|(kinds, reason)| {
            let names = kinds.iter().map(|k| k.display_name()).collect::<Vec<_>>().join(", ");
            view! {
                <div class="wizard__status is-error build-wizard__unread">
                    <Icon name="error_outline" />
                    <span class="grow">{i18n.tp("wizard_loaders_failed", &[("loaders", names), ("error", reason)])}</span>
                    <Button variant=Variant::Ghost icon="refresh" on_click=move |_| retry()>
                        {move || i18n.t("wizard_retry")}
                    </Button>
                </div>
            }
        })}
    }
}

/// The picked loader's build for Minecraft `mc`; nothing for vanilla.
#[component]
pub fn LoaderBuildField(pick: LoaderPick, #[prop(into)] mc: Signal<String>) -> impl IntoView {
    let i18n = use_i18n();
    let options = Signal::derive(move || {
        let (kind, mc) = (pick.kind(), mc.get());
        let beta = i18n.t("version_create_unstable_loader_badge");
        let own = pick.own.get();
        pick.catalogs.with(|all| build_options(kind, &mc, all, &beta, own.as_ref()))
    });
    view! {
        <Show when=move || pick.kind() != LoaderKind::Minecraft>
            <Field label=Signal::derive(move || i18n.t("minecraft_components_loader_build_label"))>
                <Select options=options value=pick.build />
            </Field>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{LoaderCatalog, LoaderOption};

    use super::*;

    /// A Minecraft version, its loader builds (stable or not) and the recommended one.
    type Offer<'a> = (&'a str, &'a [(&'a str, bool)], &'a str);

    fn catalog(mcs: &[Offer]) -> LoaderCatalog {
        LoaderCatalog::pack(
            mcs.iter()
                .map(|(mc, builds, default)| LoaderOption {
                    mc: mc.to_string(),
                    snapshot: false,
                    builds: builds
                        .iter()
                        .map(|(v, stable)| LoaderBuild { version: v.to_string(), stable: *stable })
                        .collect(),
                    default_version: default.to_string(),
                })
                .collect(),
        )
    }

    /// Fabric for 1.21.1 (0.16.9 recommended, a beta too) and 1.20.1, NeoForge unread, Forge for
    /// 1.20.1 only, Quilt on its way.
    fn catalogs() -> Catalogs {
        let mut all = Catalogs::new();
        all.insert(
            LoaderKind::Fabric,
            Ok(catalog(&[
                ("1.21.1", &[("0.17.0-beta.1", false), ("0.16.9", true)], "0.16.9"),
                ("1.20.1", &[("0.16.9", true)], "0.16.9"),
            ])),
        );
        all.insert(LoaderKind::NeoForge, Err("offline".into()));
        all.insert(LoaderKind::Forge, Ok(catalog(&[("1.20.1", &[("47.4.10", true)], "47.4.10")])));
        all
    }

    #[test]
    fn a_card_tells_a_catalog_on_its_way_or_unread_apart_from_one_without_the_version() {
        assert_eq!(loader_hint(&LoaderState::Available), "wizard_loader_desc");
        assert_eq!(loader_hint(&LoaderState::Loading), "wizard_loading");
        assert_eq!(loader_hint(&LoaderState::Failed("offline".into())), "wizard_loader_failed_hint");
        assert_eq!(loader_hint(&LoaderState::Missing), "wizard_loader_unavailable_hint");
    }

    #[test]
    fn the_cards_offer_the_loaders_with_a_build_for_the_version() {
        let all = catalogs();
        let cards = loader_cards(true, "1.21.1", &all, |key| key.to_string());
        let shown: Vec<(&str, &str, bool)> =
            cards.iter().map(|c| (c.value.as_str(), c.desc.as_deref().unwrap_or(""), c.disabled)).collect();
        assert_eq!(
            shown,
            [
                ("minecraft", "wizard_vanilla_desc", false),
                ("fabric", "wizard_loader_desc", false),
                ("neoforge", "wizard_loader_failed_hint", true),
                ("forge", "wizard_loader_unavailable_hint", true),
                ("quilt", "wizard_loading", true),
            ]
        );
        assert_eq!((cards[0].label.as_str(), cards[0].icon.as_str()), ("wizard_vanilla", "loader:vanilla"));
        assert_eq!((cards[1].label.as_str(), cards[1].icon.as_str()), ("Fabric", "loader:fabric"));
        let loaders: Vec<String> =
            loader_cards(false, "1.20.1", &all, |key| key.to_string()).into_iter().map(|c| c.value).collect();
        assert_eq!(loaders, ["fabric", "neoforge", "forge", "quilt"], "a build that has vanilla already");
    }

    #[test]
    fn the_builds_of_a_loader_mark_prereleases() {
        let all = catalogs();
        let labels: Vec<(String, String)> = build_options(LoaderKind::Fabric, "1.21.1", &all, "бета", None)
            .into_iter()
            .map(|o| (o.value, o.label))
            .collect();
        assert_eq!(
            labels,
            [
                ("0.17.0-beta.1".to_string(), "0.17.0-beta.1 · бета".to_string()),
                ("0.16.9".to_string(), "0.16.9".to_string())
            ]
        );
        assert!(build_options(LoaderKind::Forge, "1.21.1", &all, "бета", None).is_empty());
        assert!(build_options(LoaderKind::Minecraft, "1.21.1", &all, "бета", None).is_empty());
    }

    #[test]
    fn the_pick_moves_to_a_loader_that_has_the_version() {
        let all = catalogs();
        assert_eq!(settled_loader(LoaderKind::Fabric, "1.21.1", &all, false, None), LoaderKind::Fabric);
        assert_eq!(
            settled_loader(LoaderKind::Quilt, "1.21.1", &all, false, None),
            LoaderKind::Quilt,
            "on its way"
        );
        assert_eq!(settled_loader(LoaderKind::Forge, "1.21.1", &all, false, None), LoaderKind::Fabric);
        assert_eq!(settled_loader(LoaderKind::NeoForge, "1.20.1", &all, true, None), LoaderKind::Fabric);
        assert_eq!(settled_loader(LoaderKind::Minecraft, "1.12.2", &all, true, None), LoaderKind::Minecraft);
        assert_eq!(settled_loader(LoaderKind::Forge, "1.12.2", &all, true, None), LoaderKind::Minecraft);
        assert_eq!(
            settled_loader(LoaderKind::Forge, "1.12.2", &all, false, None),
            LoaderKind::Forge,
            "no loader has it and vanilla is not offered: the pick stays, and nothing installs"
        );
    }

    #[test]
    fn the_build_stays_while_offered_else_the_recommended_one() {
        let all = catalogs();
        assert_eq!(settled_build(LoaderKind::Fabric, "1.21.1", "0.17.0-beta.1", &all, None), "0.17.0-beta.1");
        assert_eq!(settled_build(LoaderKind::Fabric, "1.21.1", "", &all, None), "0.16.9");
        assert_eq!(settled_build(LoaderKind::Fabric, "1.21.1", "47.4.10", &all, None), "0.16.9");
        assert_eq!(settled_build(LoaderKind::Forge, "1.21.1", "47.4.10", &all, None), "");
        assert_eq!(settled_build(LoaderKind::Minecraft, "1.21.1", "0.16.9", &all, None), "");
        assert_eq!(
            settled_build(LoaderKind::Quilt, "1.21.1", "0.27.1", &all, None),
            "0.27.1",
            "the build's own stays while the catalog is on its way"
        );
        assert_eq!(
            settled_build(LoaderKind::NeoForge, "1.21.1", "21.1.77", &all, None),
            "21.1.77",
            "or unread"
        );
    }

    #[test]
    fn another_loader_warns_for_the_mods_the_build_has() {
        assert_eq!(change_warning(LoaderKind::Minecraft, LoaderKind::Fabric), None, "no mods to lose");
        assert_eq!(change_warning(LoaderKind::Fabric, LoaderKind::Fabric), None, "another build of it");
        assert_eq!(
            change_warning(LoaderKind::Fabric, LoaderKind::Forge),
            Some(("build_loader_change_warning", vec![("from", "Fabric".into()), ("to", "Forge".into())]))
        );
        assert_eq!(
            change_warning(LoaderKind::NeoForge, LoaderKind::Minecraft),
            Some(("build_loader_vanilla_warning", vec![("from", "NeoForge".into())]))
        );
    }

    #[test]
    fn the_unread_catalogs_are_named_in_the_cards_order_with_the_first_reason() {
        let mut all = Catalogs::new();
        all.insert(LoaderKind::Quilt, Err("timed out".into()));
        all.insert(LoaderKind::Fabric, Ok(LoaderCatalog::default()));
        all.insert(LoaderKind::Forge, Err("offline".into()));
        assert_eq!(
            unread_loaders(&all),
            Some((vec![LoaderKind::Forge, LoaderKind::Quilt], "offline".to_string()))
        );
        all.remove(&LoaderKind::Forge);
        all.remove(&LoaderKind::Quilt);
        assert_eq!(unread_loaders(&all), None, "on its way or read");
    }

    #[test]
    fn loaders_are_named_by_their_cards() {
        for kind in LoaderKind::ALL {
            assert_eq!(loader_of(&loader_value(kind)), kind);
        }
        assert_eq!(loader_of("nonsense"), LoaderKind::Minecraft);
        assert_eq!(loader_mark(LoaderKind::NeoForge), "loader:neoforge");
    }

    fn own(kind: LoaderKind, mc: &str, build: &str) -> OwnPick {
        OwnPick { kind, mc: mc.into(), build: build.into() }
    }

    #[test]
    fn the_settings_keep_the_build_s_loader_when_its_catalog_fails_or_has_only_betas() {
        let all = catalogs();
        // NeoForge's catalog could not be read: the wizard moves on, the build's settings stay.
        let neo = own(LoaderKind::NeoForge, "1.21.1", "21.1.77");
        assert_eq!(settled_loader(LoaderKind::NeoForge, "1.21.1", &all, true, None), LoaderKind::Fabric);
        assert_eq!(
            settled_loader(LoaderKind::NeoForge, "1.21.1", &all, true, Some(&neo)),
            LoaderKind::NeoForge
        );
        assert_eq!(settled_build(LoaderKind::NeoForge, "1.21.1", "21.1.77", &all, Some(&neo)), "21.1.77");
        // Forge has no stable build for 1.21.1 (only betas, not asked for): the pick stays.
        let forge = own(LoaderKind::Forge, "1.21.1", "52.0.1-beta");
        assert_eq!(settled_loader(LoaderKind::Forge, "1.21.1", &all, true, Some(&forge)), LoaderKind::Forge);
        assert_eq!(
            settled_build(LoaderKind::Forge, "1.21.1", "52.0.1-beta", &all, Some(&forge)),
            "52.0.1-beta"
        );
        // A beta the stable catalog does not list stays, and is offered first.
        let fabric = own(LoaderKind::Fabric, "1.21.1", "0.17.0-beta.2");
        assert_eq!(
            settled_build(LoaderKind::Fabric, "1.21.1", "0.17.0-beta.2", &all, None),
            "0.16.9",
            "the wizard's pick goes to the recommended build"
        );
        assert_eq!(
            settled_build(LoaderKind::Fabric, "1.21.1", "0.17.0-beta.2", &all, Some(&fabric)),
            "0.17.0-beta.2"
        );
        let offered: Vec<(String, String)> =
            build_options(LoaderKind::Fabric, "1.21.1", &all, "бета", Some(&fabric))
                .into_iter()
                .map(|o| (o.value, o.label))
                .collect();
        assert_eq!(offered[0], ("0.17.0-beta.2".to_string(), "0.17.0-beta.2 · бета".to_string()));
        assert_eq!(offered.len(), 3);
        assert_eq!(
            build_options(LoaderKind::Fabric, "1.20.1", &all, "бета", Some(&fabric)).len(),
            1,
            "only for the build's own Minecraft"
        );
        // Back on its own loader after another, the build's own build comes back.
        assert_eq!(
            settled_build(LoaderKind::Fabric, "1.21.1", "47.4.10", &all, Some(&fabric)),
            "0.17.0-beta.2"
        );
        assert_eq!(
            settled_build(LoaderKind::Fabric, "1.20.1", "0.17.0-beta.2", &all, Some(&fabric)),
            "0.16.9"
        );
    }

    #[test]
    fn prereleases_are_told_by_their_words() {
        for beta in ["0.17.0-beta.1", "21.1.0-rc2", "1.21-pre1", "0.27.0-Alpha.3"] {
            assert!(is_beta(beta), "{beta}");
        }
        for stable in ["0.16.9", "47.4.10", "21.1.77", ""] {
            assert!(!is_beta(stable), "{stable}");
        }
        assert!(version_key("0.17.0") > version_key("0.17.0-beta.1"), "a release over its prereleases");
        assert!(version_key("0.16.10") > version_key("0.16.9"));
    }

    fn release(id: &str) -> CatalogVersion {
        CatalogVersion { id: id.into(), kind: "release".into(), release_time: None }
    }

    #[test]
    fn going_older_or_to_another_minecraft_warns_before_it_installs() {
        let versions = [release("1.21.4"), release("1.21.1"), release("1.20.1")];
        let offered = |list: &[&str]| -> Vec<LoaderBuild> {
            list.iter().map(|v| LoaderBuild { version: v.to_string(), stable: !is_beta(v) }).collect()
        };
        let builds = offered(&["0.16.10", "0.16.9"]);
        let keys = |from: &OwnPick, to: &OwnPick| -> Vec<&'static str> {
            pick_warnings(from, to, 0, &versions, &builds).into_iter().map(|(key, _)| key).collect()
        };
        let fabric = own(LoaderKind::Fabric, "1.21.1", "0.16.10");
        assert!(keys(&fabric, &own(LoaderKind::Fabric, "1.21.1", "0.16.10")).is_empty(), "nothing changes");
        assert_eq!(
            pick_warnings(&fabric, &own(LoaderKind::Fabric, "1.21.1", "0.16.9"), 0, &versions, &builds),
            [(
                "build_loader_older_warning",
                vec![("loader", "Fabric".into()), ("from", "0.16.10".into()), ("to", "0.16.9".into())]
            )]
        );
        assert!(keys(&own(LoaderKind::Fabric, "1.21.1", "0.16.9"), &fabric).is_empty(), "a newer build");
        assert!(
            keys(
                &own(LoaderKind::Fabric, "1.21.1", "0.17.0-beta.1"),
                &own(LoaderKind::Fabric, "1.21.1", "0.17.0")
            )
            .is_empty(),
            "from a beta to its release, unlisted"
        );
        assert_eq!(keys(&fabric, &own(LoaderKind::Fabric, "1.20.1", "0.16.10")), ["build_mc_older_warning"]);
        assert_eq!(keys(&fabric, &own(LoaderKind::Fabric, "1.21.4", "0.16.10")), ["build_mc_newer_warning"]);
        assert_eq!(
            keys(&fabric, &own(LoaderKind::Forge, "1.20.1", "47.4.10")),
            ["build_loader_change_warning", "build_mc_older_warning"]
        );
        let vanilla = own(LoaderKind::Minecraft, "1.21.1", "");
        assert_eq!(
            keys(&vanilla, &own(LoaderKind::Minecraft, "1.12.2", "")),
            ["build_mc_older_warning"],
            "out of the catalog, by the numbers"
        );
    }

    #[test]
    fn a_loader_on_a_vanilla_build_warns_of_the_mods_left_in_it() {
        assert_eq!(leftover_warning(0, LoaderKind::Fabric), None, "nothing left");
        assert_eq!(leftover_warning(3, LoaderKind::Minecraft), None, "vanilla loads none");
        assert_eq!(
            leftover_warning(3, LoaderKind::Forge),
            Some(("build_loader_leftover_warning", vec![("count", "3".into()), ("to", "Forge".into())]))
        );
        let vanilla = own(LoaderKind::Minecraft, "1.21.1", "");
        let forge = own(LoaderKind::Forge, "1.21.1", "52.0.1");
        let keys: Vec<&str> =
            pick_warnings(&vanilla, &forge, 2, &[], &[]).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["build_loader_leftover_warning"]);
        assert!(pick_warnings(&vanilla, &forge, 0, &[], &[]).is_empty(), "an empty mods folder");
    }

    #[test]
    fn the_build_s_own_pick_is_its_loader_minecraft_and_build() {
        let mut build = BuildDto {
            version: Some("1.21.1".into()),
            loader: Some("fabric-loader-0.16.9-1.21.1".into()),
            client: Some("Fabric".into()),
            loader_version: Some(" 0.16.9 ".into()),
            ..BuildDto::default()
        };
        assert_eq!(OwnPick::of(&build), own(LoaderKind::Fabric, "1.21.1", "0.16.9"));
        build.loader = Some("1.21.1".into());
        build.client = Some("Minecraft".into());
        assert_eq!(OwnPick::of(&build), own(LoaderKind::Minecraft, "1.21.1", ""), "vanilla has no build");
    }
}
