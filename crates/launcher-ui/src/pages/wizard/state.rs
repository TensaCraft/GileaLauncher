//! The build wizard's state: the four steps, what each needs before «Далі», the loaders a version
//! has, the picked content (the plan) and what «Створити» sends. No view here: every rule is a
//! plain function, tested on its own.

use std::collections::HashMap;

use launcher_shared::provider::ProviderInfo;
use launcher_shared::{
    CatalogVersion, ContentKind, LoaderCatalog, LoaderKind, PlannedBuild, PlannedPick, PlannedSettings,
};

use crate::builds::description::description_problem;

/// Fabric API on Modrinth: what nearly every Fabric mod needs.
pub const FABRIC_API: &str = "P7dR8mSH";
/// QFAPI / QSL on Modrinth: Quilt's.
pub const QFAPI: &str = "qvIfYCYJ";
/// The provider the starter picks come from.
const MODRINTH: &str = "modrinth";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Basics,
    Content,
    Settings,
    Summary,
}

impl WizardStep {
    pub const ALL: [WizardStep; 4] =
        [WizardStep::Basics, WizardStep::Content, WizardStep::Settings, WizardStep::Summary];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<WizardStep> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<WizardStep> {
        self.index().checked_sub(1).and_then(|i| Self::ALL.get(i).copied())
    }

    pub fn title_key(self) -> &'static str {
        match self {
            WizardStep::Basics => "wizard_step_basics",
            WizardStep::Content => "wizard_step_content",
            WizardStep::Settings => "wizard_step_settings",
            WizardStep::Summary => "wizard_step_summary",
        }
    }
}

/// Each loader's catalog, or why it could not be read; a loader not in it has not answered yet.
pub type Catalogs = HashMap<LoaderKind, Result<LoaderCatalog, String>>;

/// What a loader's card says of a Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoaderState {
    Available,
    /// Its catalog has not come yet.
    Loading,
    /// Its catalog could not be read: why.
    Failed(String),
    /// It has no build for the version.
    Missing,
}

/// Whether `loader` has a build for Minecraft `mc` (vanilla always does), or why it is not known.
pub fn loader_state(loader: LoaderKind, mc: &str, catalogs: &Catalogs) -> LoaderState {
    if loader == LoaderKind::Minecraft {
        return LoaderState::Available;
    }
    match catalogs.get(&loader) {
        None => LoaderState::Loading,
        Some(Err(e)) => LoaderState::Failed(e.clone()),
        Some(Ok(c)) if c.options.iter().any(|o| o.mc == mc) => LoaderState::Available,
        Some(Ok(_)) => LoaderState::Missing,
    }
}

/// The loader has a build for Minecraft `mc` (vanilla always does).
pub fn loader_available(loader: LoaderKind, mc: &str, catalogs: &Catalogs) -> bool {
    loader_state(loader, mc, catalogs) == LoaderState::Available
}

/// The recommended loader build for `mc` (empty for vanilla or when there is none).
pub fn default_loader_version(loader: LoaderKind, mc: &str, catalogs: &Catalogs) -> String {
    if loader == LoaderKind::Minecraft {
        return String::new();
    }
    catalogs
        .get(&loader)
        .and_then(|c| c.as_ref().ok())
        .and_then(|c| c.options.iter().find(|o| o.mc == mc))
        .map(|o| o.default_version.clone())
        .unwrap_or_default()
}

/// What a build of `loader` can be given: vanilla runs resource packs only.
pub fn content_kinds(loader: LoaderKind) -> Vec<ContentKind> {
    if loader == LoaderKind::Minecraft {
        vec![ContentKind::ResourcePacks]
    } else {
        vec![ContentKind::Mods, ContentKind::ResourcePacks, ContentKind::ShaderPacks]
    }
}

/// The newest of `versions` (newest first), a snapshot only when `snapshots`.
pub fn newest_release(versions: &[CatalogVersion], snapshots: bool) -> Option<String> {
    versions.iter().find(|v| snapshots || v.kind == "release").map(|v| v.id.clone())
}

/// Everything the wizard has been told so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardState {
    pub name: String,
    pub mc: String,
    pub snapshots: bool,
    pub loader: LoaderKind,
    pub loader_version: String,
    pub picks: Vec<PlannedPick>,
    pub settings: PlannedSettings,
    /// The starter picks were offered for this target (and stay removed once removed).
    pub starters_offered: bool,
    /// A change of target dropped the picks, and the player has not changed the plan since.
    pub picks_dropped: bool,
}

impl Default for WizardState {
    fn default() -> WizardState {
        WizardState {
            name: String::new(),
            mc: String::new(),
            snapshots: false,
            loader: LoaderKind::Minecraft,
            loader_version: String::new(),
            picks: Vec::new(),
            settings: PlannedSettings::default(),
            starters_offered: false,
            picks_dropped: false,
        }
    }
}

impl WizardState {
    /// Minecraft `mc` with `loader` (vanilla when it has no build for `mc`). Another target than
    /// before drops the picks (they were found for the old one), and says so when there were some.
    pub fn set_target(&mut self, mc: &str, loader: LoaderKind, catalogs: &Catalogs) {
        let loader = if loader_available(loader, mc, catalogs) { loader } else { LoaderKind::Minecraft };
        if self.mc == mc && self.loader == loader {
            if self.loader_version.is_empty() {
                self.loader_version = default_loader_version(loader, mc, catalogs);
            }
            return;
        }
        self.mc = mc.to_string();
        self.loader = loader;
        self.loader_version = default_loader_version(loader, mc, catalogs);
        self.starters_offered = false;
        self.picks_dropped |= !self.picks.is_empty();
        self.picks.clear();
    }

    pub fn has_pick(&self, provider: &str, project_id: &str) -> bool {
        self.picks.iter().any(|p| p.provider == provider && p.project_id == project_id)
    }

    /// The player's pick into the plan, once.
    pub fn add_pick(&mut self, pick: PlannedPick) {
        self.picks_dropped = false;
        self.insert_pick(pick);
    }

    pub fn remove_pick(&mut self, provider: &str, project_id: &str) {
        self.picks_dropped = false;
        self.picks.retain(|p| !(p.provider == provider && p.project_id == project_id));
    }

    fn insert_pick(&mut self, pick: PlannedPick) {
        if !self.has_pick(&pick.provider, &pick.project_id) {
            self.picks.push(pick);
        }
    }

    /// The loader's own API (Fabric API, QFAPI), picked the first time the content step opens
    /// for this target, when Modrinth is there to give it.
    pub fn offer_starters(&mut self, providers: &[ProviderInfo]) {
        if self.starters_offered {
            return;
        }
        self.starters_offered = true;
        if !providers.iter().any(|p| p.id == MODRINTH) {
            return;
        }
        let starter = match self.loader {
            LoaderKind::Fabric => Some((FABRIC_API, "fabric-api", "Fabric API")),
            LoaderKind::Quilt => Some((QFAPI, "qsl", "QFAPI / QSL")),
            _ => None,
        };
        if let Some((id, slug, title)) = starter {
            self.insert_pick(PlannedPick {
                provider: MODRINTH.into(),
                kind: ContentKind::Mods,
                project_id: id.into(),
                slug: slug.into(),
                title: title.into(),
                icon_url: None,
            });
        }
    }

    /// Whether `step` and every step before it let the player go on; the first reason (a
    /// translation key) when not. `name_problem`: what the name check said of the name.
    pub fn ready(
        &self,
        step: WizardStep,
        name_problem: Option<&'static str>,
        catalogs: &Catalogs,
    ) -> Result<(), &'static str> {
        WizardStep::ALL[..=step.index()]
            .iter()
            .try_for_each(|&each| self.own_ready(each, name_problem, catalogs))
    }

    /// Whether the player may open `step` from the row of steps: every step before it is ready.
    pub fn can_open(
        &self,
        step: WizardStep,
        name_problem: Option<&'static str>,
        catalogs: &Catalogs,
    ) -> bool {
        step.prev().is_none_or(|before| self.ready(before, name_problem, catalogs).is_ok())
    }

    /// What `step` itself asks for.
    fn own_ready(
        &self,
        step: WizardStep,
        name_problem: Option<&'static str>,
        catalogs: &Catalogs,
    ) -> Result<(), &'static str> {
        match step {
            WizardStep::Basics => {
                if let Some(problem) = name_problem {
                    return Err(problem);
                }
                if self.mc.trim().is_empty() {
                    return Err("wizard_no_version");
                }
                if !loader_available(self.loader, &self.mc, catalogs) {
                    return Err("wizard_loader_unavailable");
                }
                if self.loader != LoaderKind::Minecraft && self.loader_version.trim().is_empty() {
                    return Err("wizard_no_loader_version");
                }
                Ok(())
            }
            WizardStep::Settings => description_problem(&self.settings.description).map_or(Ok(()), Err),
            _ => Ok(()),
        }
    }

    /// What «Створити» sends.
    pub fn planned(&self) -> PlannedBuild {
        PlannedBuild {
            name: self.name.trim().to_string(),
            mc: self.mc.clone(),
            loader: self.loader,
            loader_version: (self.loader != LoaderKind::Minecraft).then(|| self.loader_version.clone()),
            settings: self.settings.clone(),
            picks: self.picks.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::{LoaderBuild, LoaderOption};

    use super::*;

    fn catalog(versions: &[&str]) -> Result<LoaderCatalog, String> {
        let options = versions
            .iter()
            .map(|mc| LoaderOption {
                mc: (*mc).into(),
                snapshot: false,
                builds: vec![LoaderBuild { version: "0.16.10".into(), stable: true }],
                default_version: "0.16.10".into(),
            })
            .collect();
        Ok(LoaderCatalog::pack(options))
    }

    fn catalogs() -> Catalogs {
        let mut all = Catalogs::new();
        all.insert(LoaderKind::Fabric, catalog(&["1.21.1", "1.20.1"]));
        all.insert(LoaderKind::NeoForge, catalog(&["1.21.1"]));
        all.insert(LoaderKind::Forge, Err("offline".into()));
        all
    }

    fn provider(id: &str) -> ProviderInfo {
        ProviderInfo {
            id: id.into(),
            name: id.into(),
            icon: "search".into(),
            content: vec![ContentKind::Mods],
            updates: Vec::new(),
            modpacks: false,
            modpack_updates: false,
            content_clients: Vec::new(),
            content_label: None,
        }
    }

    fn pick(id: &str) -> PlannedPick {
        PlannedPick {
            provider: "modrinth".into(),
            kind: ContentKind::Mods,
            project_id: id.into(),
            slug: id.into(),
            title: id.to_uppercase(),
            icon_url: None,
        }
    }

    fn fabric_on(mc: &str) -> WizardState {
        let mut state = WizardState { name: "Моя збірка".into(), ..WizardState::default() };
        state.set_target(mc, LoaderKind::Fabric, &catalogs());
        state
    }

    #[test]
    fn the_steps_go_in_order() {
        assert_eq!(WizardStep::Basics.next(), Some(WizardStep::Content));
        assert_eq!(WizardStep::Summary.next(), None);
        assert_eq!(WizardStep::Content.prev(), Some(WizardStep::Basics));
        assert_eq!(WizardStep::Basics.prev(), None);
        assert_eq!(WizardStep::ALL.map(WizardStep::index), [0, 1, 2, 3]);
        assert_eq!(WizardStep::Settings.title_key(), "wizard_step_settings");
    }

    #[test]
    fn a_loader_without_the_version_is_not_available() {
        let all = catalogs();
        assert!(loader_available(LoaderKind::Minecraft, "1.21.1", &all), "vanilla always");
        assert!(loader_available(LoaderKind::Fabric, "1.20.1", &all));
        assert!(!loader_available(LoaderKind::NeoForge, "1.20.1", &all));
        assert!(!loader_available(LoaderKind::Forge, "1.21.1", &all), "a catalog that could not be read");
        assert!(!loader_available(LoaderKind::Quilt, "1.21.1", &all), "not asked yet");
        assert_eq!(default_loader_version(LoaderKind::Fabric, "1.21.1", &all), "0.16.10");
        assert_eq!(default_loader_version(LoaderKind::Minecraft, "1.21.1", &all), "");
    }

    #[test]
    fn a_loader_card_tells_a_catalog_on_its_way_or_unread_from_one_without_the_version() {
        let all = catalogs();
        assert_eq!(loader_state(LoaderKind::Minecraft, "1.21.1", &all), LoaderState::Available);
        assert_eq!(loader_state(LoaderKind::Fabric, "1.20.1", &all), LoaderState::Available);
        assert_eq!(loader_state(LoaderKind::NeoForge, "1.20.1", &all), LoaderState::Missing);
        assert_eq!(loader_state(LoaderKind::Forge, "1.21.1", &all), LoaderState::Failed("offline".into()));
        assert_eq!(loader_state(LoaderKind::Quilt, "1.21.1", &all), LoaderState::Loading, "not come yet");
    }

    #[test]
    fn a_new_version_keeps_the_loader_it_still_has_or_falls_back_to_vanilla() {
        let mut state = fabric_on("1.21.1");
        state.set_target("1.20.1", LoaderKind::Fabric, &catalogs());
        assert_eq!((state.loader, state.loader_version.as_str()), (LoaderKind::Fabric, "0.16.10"));
        let mut neo = fabric_on("1.21.1");
        neo.set_target("1.21.1", LoaderKind::NeoForge, &catalogs());
        neo.set_target("1.20.1", LoaderKind::NeoForge, &catalogs());
        assert_eq!((neo.loader, neo.loader_version.as_str()), (LoaderKind::Minecraft, ""));
    }

    #[test]
    fn changing_the_target_drops_the_picks_and_says_so() {
        let mut state = fabric_on("1.21.1");
        state.add_pick(pick("sodium"));
        state.set_target("1.21.1", LoaderKind::Fabric, &catalogs());
        assert_eq!((state.picks.len(), state.picks_dropped), (1, false), "the same target keeps them");
        state.set_target("1.20.1", LoaderKind::Fabric, &catalogs());
        assert_eq!((state.picks.len(), state.picks_dropped), (0, true), "another version drops them");
        state.set_target("1.21.1", LoaderKind::Fabric, &catalogs());
        assert!(state.picks_dropped, "still to be told, with nothing more to drop");
    }

    #[test]
    fn the_dropped_picks_are_told_until_the_player_changes_the_plan() {
        let dropped = || {
            let mut state = fabric_on("1.21.1");
            state.add_pick(pick("sodium"));
            state.set_target("1.20.1", LoaderKind::Fabric, &catalogs());
            state.offer_starters(&[provider("modrinth")]);
            assert!(state.picks_dropped, "the wizard's own starter is no change of the player's");
            state
        };
        let mut removed = dropped();
        removed.remove_pick("modrinth", FABRIC_API);
        assert!(!removed.picks_dropped, "a pick taken out of the plan");
        let mut added = dropped();
        added.add_pick(pick("iris"));
        assert!(!added.picks_dropped, "a pick put into it");
    }

    #[test]
    fn fabric_offers_fabric_api_once_and_quilt_qfapi_and_vanilla_nothing() {
        let providers = [provider("modrinth"), provider("curseforge")];
        let mut state = fabric_on("1.21.1");
        state.offer_starters(&providers);
        state.offer_starters(&providers);
        assert_eq!(state.picks.iter().map(|p| p.project_id.as_str()).collect::<Vec<_>>(), [FABRIC_API]);
        state.remove_pick("modrinth", FABRIC_API);
        state.offer_starters(&providers);
        assert!(state.picks.is_empty(), "once removed, it stays removed");
        let mut quilt = WizardState { loader: LoaderKind::Quilt, ..WizardState::default() };
        quilt.offer_starters(&providers);
        assert_eq!(quilt.picks[0].project_id, QFAPI);
        let mut vanilla = WizardState::default();
        vanilla.offer_starters(&providers);
        assert!(vanilla.picks.is_empty());
        let mut alone = fabric_on("1.21.1");
        alone.offer_starters(&[provider("curseforge")]);
        assert!(alone.picks.is_empty(), "no Modrinth, no starter");
    }

    #[test]
    fn a_pick_is_in_the_plan_once() {
        let mut state = fabric_on("1.21.1");
        state.offer_starters(&[provider("modrinth")]);
        state.add_pick(PlannedPick { title: "Fabric API (again)".into(), ..state.picks[0].clone() });
        state.add_pick(pick("sodium"));
        assert_eq!(state.picks.len(), 2);
        assert!(state.has_pick("modrinth", "sodium") && !state.has_pick("curseforge", "sodium"));
    }

    #[test]
    fn vanilla_picks_resource_packs_only() {
        assert_eq!(content_kinds(LoaderKind::Minecraft), [ContentKind::ResourcePacks]);
        assert_eq!(
            content_kinds(LoaderKind::Fabric),
            [ContentKind::Mods, ContentKind::ResourcePacks, ContentKind::ShaderPacks]
        );
    }

    #[test]
    fn the_basics_are_ready_only_with_a_name_a_version_and_a_loader_build() {
        let all = catalogs();
        let state = fabric_on("1.21.1");
        assert_eq!(state.ready(WizardStep::Basics, None, &all), Ok(()));
        assert_eq!(
            state.ready(WizardStep::Basics, Some("version_name_empty"), &all),
            Err("version_name_empty")
        );
        let empty = WizardState { mc: String::new(), ..state.clone() };
        assert_eq!(empty.ready(WizardStep::Basics, None, &all), Err("wizard_no_version"));
        let forge = WizardState { loader: LoaderKind::Forge, ..state.clone() };
        assert_eq!(forge.ready(WizardStep::Basics, None, &all), Err("wizard_loader_unavailable"));
        let no_build = WizardState { loader_version: String::new(), ..state.clone() };
        assert_eq!(no_build.ready(WizardStep::Basics, None, &all), Err("wizard_no_loader_version"));
        assert_eq!(state.ready(WizardStep::Content, None, &all), Ok(()));
    }

    #[test]
    fn a_long_description_is_not_ready() {
        let mut state = fabric_on("1.21.1");
        state.settings.description = "а".repeat(301);
        assert_eq!(state.ready(WizardStep::Settings, None, &catalogs()), Err("build_description_too_long"));
        state.settings.description = "а".repeat(300);
        assert_eq!(state.ready(WizardStep::Settings, None, &catalogs()), Ok(()));
    }

    #[test]
    fn the_summary_waits_for_every_step_before_it() {
        let all = catalogs();
        let mut state = fabric_on("1.21.1");
        assert_eq!(state.ready(WizardStep::Summary, None, &all), Ok(()));
        assert_eq!(
            state.ready(WizardStep::Summary, Some("version_exists"), &all),
            Err("version_exists"),
            "a name taken after the basics were passed"
        );
        state.settings.description = "а".repeat(301);
        assert_eq!(state.ready(WizardStep::Summary, None, &all), Err("build_description_too_long"));
    }

    #[test]
    fn a_step_opens_once_the_steps_before_it_are_ready() {
        let all = catalogs();
        let state = fabric_on("1.21.1");
        assert!(state.can_open(WizardStep::Basics, Some("version_name_empty"), &all), "the first one always");
        assert!(state.can_open(WizardStep::Summary, None, &all));
        assert!(!state.can_open(WizardStep::Content, Some("version_name_empty"), &all));
        assert!(!state.can_open(WizardStep::Summary, Some("version_name_empty"), &all));
    }

    #[test]
    fn the_plan_carries_everything_chosen() {
        let mut state = fabric_on("1.21.1");
        state.name = "  Друзі  ".into();
        state.add_pick(pick("sodium"));
        state.settings.max_ram_gb = Some(6);
        let planned = state.planned();
        assert_eq!((planned.name.as_str(), planned.mc.as_str()), ("Друзі", "1.21.1"));
        assert_eq!(
            (planned.loader, planned.loader_version.as_deref()),
            (LoaderKind::Fabric, Some("0.16.10"))
        );
        assert_eq!((planned.picks.len(), planned.settings.max_ram_gb), (1, Some(6)));
        let vanilla = WizardState { loader: LoaderKind::Minecraft, ..state }.planned();
        assert_eq!(vanilla.loader_version, None);
    }

    #[test]
    fn unticking_snapshots_falls_back_to_the_newest_release() {
        let version =
            |id: &str, kind: &str| CatalogVersion { id: id.into(), kind: kind.into(), release_time: None };
        let versions =
            [version("25w14a", "snapshot"), version("1.21.5", "release"), version("1.21.4", "release")];
        assert_eq!(newest_release(&versions, true).as_deref(), Some("25w14a"));
        assert_eq!(newest_release(&versions, false).as_deref(), Some("1.21.5"));
        assert_eq!(newest_release(&[], false), None);
    }
}
