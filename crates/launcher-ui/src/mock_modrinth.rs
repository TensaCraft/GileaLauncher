//! Browser-preview stand-in for the `modrinth` module's commands (`module_invoke`).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use launcher_shared::provider::{
    Action, FileNote, InstallAnswer, InstallArgs, InstallOutcome, ModpackBuild, NewerVersion, Overview,
    OverviewArgs, PACKS_LIMIT, PackArgs, PackInstallArgs, PackInstalled, PackUpdateArgs, PackUpdated,
    PackVersion, PacksArgs, PlanDto, PlanIssue, PlanItem, ProjectHit, SEARCH_LIMIT, SearchArgs, SearchPage,
    UpdateSummary, UpdatesStatus, installed_key, newer_pack_version, update_texts,
};
use launcher_shared::{AppError, ContentKind, ErrorCode, Level, LoaderKind, Text};
use module_modrinth::types::project_type;
use serde_json::Value;

use crate::mock_builds::{MockBuilds, toast};

/// Id, slug, title, author, downloads.
type Project = (&'static str, &'static str, &'static str, &'static str, u64);

const MODS: [Project; 18] = [
    ("mock-sodium", "sodium", "Sodium", "jellysquid3", 61_234_567),
    ("mock-lithium", "lithium", "Lithium", "jellysquid3", 38_120_000),
    ("mock-fabric-api", "fabric-api", "Fabric API", "modmuss50", 95_400_000),
    ("mock-iris", "iris", "Iris Shaders", "coderbot", 55_700_000),
    ("mock-modmenu", "modmenu", "Mod Menu", "Prospector", 40_300_000),
    ("mock-ferritecore", "ferrite-core", "FerriteCore", "malte0811", 30_900_000),
    ("mock-entityculling", "entityculling", "Entity Culling", "tr7zw", 25_100_000),
    ("mock-immediatelyfast", "immediatelyfast", "ImmediatelyFast", "RaphiMC", 20_400_000),
    ("mock-krypton", "krypton", "Krypton", "astei", 12_300_000),
    ("mock-continuity", "continuity", "Continuity", "PepperCode1", 11_800_000),
    ("mock-cloth-config", "cloth-config", "Cloth Config API", "shedaniel", 45_600_000),
    ("mock-sodium-extra", "sodium-extra", "Sodium Extra", "FlashyReese", 17_200_000),
    ("mock-reeses", "reeses-sodium-options", "Reese's Sodium Options", "FlashyReese", 16_900_000),
    ("mock-dynamic-fps", "dynamic-fps", "Dynamic FPS", "juliand665", 14_500_000),
    ("mock-lambdynamiclights", "lambdynamiclights", "LambDynamicLights", "LambdAurora", 13_700_000),
    ("mock-zoomify", "zoomify", "Zoomify", "isXander", 9_100_000),
    ("mock-appleskin", "appleskin", "AppleSkin", "squeek502", 8_600_000),
    ("mock-broken", "broken-mod", "Broken Mod", "nobody", 12),
];

const PACKS: [Project; 4] = [
    ("mock-faithful", "faithful-32x", "Faithful 32x", "Faithful", 3_400_000),
    ("mock-fresh", "fresh-animations", "Fresh Animations", "FreshLX", 6_100_000),
    ("mock-stay-true", "stay-true", "Stay True", "haylee", 1_200_000),
    ("mock-xali", "xalis-enchanted-books", "Xali's Enchanted Books", "Xali", 2_300_000),
];

const SHADERS: [Project; 4] = [
    ("mock-complementary", "complementary-reimagined", "Complementary Shaders", "EminGT", 9_900_000),
    ("mock-bsl", "bsl-shaders", "BSL Shaders", "capttatsu", 7_800_000),
    ("mock-makeup", "makeup-ultra-fast-shaders", "MakeUp - Ultra Fast", "XorDev", 4_200_000),
    ("mock-solas", "solas-shader", "Solas Shader", "Septonious", 1_900_000),
];

/// Id, title, author, downloads; the page fills the rest with numbered packs so it has a second page.
const MODPACKS: [(&str, &str, &str, u64); 6] = [
    ("mock-fo", "Fabulously Optimized", "robotkoer", 12_400_000),
    ("mock-cobblemon", "Cobblemon Official Modpack", "Cobblemon", 5_300_000),
    ("mock-simply", "Simply Optimized", "Ryan", 3_100_000),
    ("mock-additive", "Additive", "Wither", 1_700_000),
    ("mock-adrenaline", "Adrenaline", "jaxs", 1_200_000),
    ("mock-pack-empty", "Empty Pack", "nobody", 7),
];

/// Modpacks in the preview.
const MODPACK_COUNT: u32 = 26;

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, AppError> {
    serde_json::from_value(value).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

pub struct MockModrinth {
    /// `?modrinth=error`: every search fails as if offline.
    fail: bool,
    /// A modpack installs as a new build here.
    builds: Rc<RefCell<MockBuilds>>,
    installed: HashMap<(String, ContentKind), BTreeSet<String>>,
    /// Versions installed by id: an offered update once installed is offered no more.
    versions: HashSet<String>,
    /// Builds installed from a modpack: key → (project, version).
    packs: HashMap<String, (String, String)>,
    /// Looks at the Downloads folder so far: Entity Culling's held file (the mod's, and the one
    /// Adrenaline takes) is "downloaded" by the third.
    looks: u32,
}

impl MockModrinth {
    pub fn new(fail: bool, builds: Rc<RefCell<MockBuilds>>) -> MockModrinth {
        MockModrinth {
            fail,
            builds,
            installed: HashMap::new(),
            versions: HashSet::new(),
            packs: HashMap::new(),
            looks: 0,
        }
    }

    /// `held_files_found`: the user "downloads" the files after a few looks.
    pub fn held_found(&mut self, count: usize) -> Vec<bool> {
        self.looks += 1;
        vec![self.downloaded(); count]
    }

    fn downloaded(&self) -> bool {
        self.looks >= 3
    }

    fn catalog(kind: ContentKind) -> &'static [Project] {
        match kind {
            ContentKind::Mods => &MODS,
            ContentKind::ResourcePacks => &PACKS,
            ContentKind::ShaderPacks => &SHADERS,
        }
    }

    /// The mock's plans: Sodium Extra needs Sodium and Fabric API, Iris offers Sodium, Reese's
    /// needs a file-only dependency; picks join the install.
    fn plan(&self, args: &InstallArgs) -> Result<PlanDto, AppError> {
        if args.project_id == "mock-broken" {
            return Err(
                AppError::new(ErrorCode::NoCompatibleVersion, "mock").with_param("name", args.title.clone())
            );
        }
        let installed = self.installed.get(&(args.key.clone(), args.kind)).cloned().unwrap_or_default();
        let item = |id: &str, title: &str| PlanItem {
            project_id: id.into(),
            version_id: format!("{id}-1"),
            title: title.into(),
            version_number: "1.0.0".into(),
            filename: format!("{id}.jar"),
            action: if installed.contains(id) { Action::Satisfied } else { Action::Install },
            unrecognized: false,
            current: None,
            url: Some(format!("https://modrinth.com/mod/{}", id.trim_start_matches("mock-"))),
        };
        let mut main = item(&args.project_id, &args.title);
        // The version the user saw; for a project the build has now, it replaces the installed file.
        if let Some(version) = &args.version_id {
            let overview = OverviewArgs { key: args.key.clone(), kind: args.kind, check_updates: false };
            let there = self.overview(&overview).notes.iter().any(|n| n.project_id == args.project_id);
            let action = if there { Action::Replace } else { main.action };
            main = PlanItem { version_id: version.clone(), action, ..main };
        }
        let mut plan = PlanDto { main: Some(main), ..PlanDto::default() };
        let mut add = |i: PlanItem| {
            if i.action == Action::Satisfied { plan.satisfied.push(i) } else { plan.install.push(i) }
        };
        if args.kind == ContentKind::Mods {
            match args.project_id.as_str() {
                "mock-sodium-extra" => {
                    add(item("mock-sodium", "Sodium"));
                    add(item("mock-fabric-api", "Fabric API"));
                }
                "mock-iris" if !args.optional.iter().any(|p| p.project_id == "mock-sodium") => {
                    plan.optional.push(item("mock-sodium", "Sodium"));
                }
                "mock-iris" => add(item("mock-sodium", "Sodium")),
                // A file its author keeps from other apps, until the user downloads it by hand.
                "mock-entityculling" if !self.downloaded() => plan.blocking.push(PlanIssue {
                    code: "file_blocked".into(),
                    name: Some("Entity Culling".into()),
                    file_name: None,
                    url: Some("https://www.curseforge.com/minecraft/mc-mods/entityculling/files/1".into()),
                    blocking: true,
                    held: Some(launcher_shared::provider::HeldFile {
                        title: "Entity Culling".into(),
                        file_name: "entityculling.jar".into(),
                        url: Some(
                            "https://www.curseforge.com/minecraft/mc-mods/entityculling/files/1".into(),
                        ),
                        size: 1,
                        sha1: "0".repeat(40),
                    }),
                }),
                "mock-reeses" => plan.blocking.push(PlanIssue {
                    code: "required_file_only".into(),
                    name: None,
                    file_name: Some("reeses-core.jar".into()),
                    url: None,
                    blocking: true,
                    held: None,
                }),
                _ => {}
            }
        }
        Ok(plan)
    }

    fn search(&self, args: &SearchArgs) -> SearchPage {
        let query = args.query.trim().to_lowercase();
        let hits: Vec<ProjectHit> = Self::catalog(args.kind)
            .iter()
            .filter(|(_, slug, title, ..)| {
                query.is_empty() || slug.contains(&query) || title.to_lowercase().contains(&query)
            })
            .map(|&(id, slug, title, author, downloads)| ProjectHit {
                project_id: id.into(),
                slug: slug.into(),
                title: title.into(),
                author: author.into(),
                description: format!(
                    "{title}: a stand-in project for the browser preview of the Modrinth tab."
                ),
                downloads,
                icon_url: None,
                url: Some(format!("https://modrinth.com/{}/{slug}", project_type(args.kind))),
            })
            .collect();
        let total = hits.len() as u32;
        let shown = hits.into_iter().skip(args.offset as usize).take(SEARCH_LIMIT as usize).collect();
        SearchPage { hits: shown, total, offset: args.offset, limit: SEARCH_LIMIT }
    }

    fn modpacks(args: &PacksArgs) -> SearchPage {
        let query = args.query.trim().to_lowercase();
        let named = MODPACKS
            .iter()
            .map(|&(id, title, author, downloads)| (id.to_string(), title.to_string(), author, downloads));
        let numbered = (MODPACKS.len() as u32 + 1..=MODPACK_COUNT).map(|n| {
            (format!("mock-pack-{n}"), format!("Preview Pack {n}"), "Launcher", u64::from(n) * 1_000)
        });
        let hits: Vec<ProjectHit> = named
            .chain(numbered)
            .filter(|(_, title, ..)| query.is_empty() || title.to_lowercase().contains(&query))
            .map(|(id, title, author, downloads)| ProjectHit {
                slug: id.trim_start_matches("mock-").into(),
                description: format!(
                    "{title}: a stand-in modpack for the browser preview of the modpacks page."
                ),
                title,
                author: author.into(),
                downloads,
                icon_url: None,
                url: Some(format!("https://modrinth.com/modpack/{}", id.trim_start_matches("mock-"))),
                project_id: id,
            })
            .collect();
        let total = hits.len() as u32;
        let shown = hits.into_iter().skip(args.offset as usize).take(PACKS_LIMIT as usize).collect();
        SearchPage { hits: shown, total, offset: args.offset, limit: PACKS_LIMIT }
    }

    /// Three Fabric versions, the newest first; the empty pack has none.
    fn pack_versions(args: &PackArgs) -> Vec<PackVersion> {
        if args.project_id == "mock-pack-empty" {
            return Vec::new();
        }
        [("6.2.0", &["1.21.1"][..]), ("6.1.0", &["1.21", "1.21.1"][..]), ("5.9.0", &["1.20.6"][..])]
            .iter()
            .map(|(number, games)| PackVersion {
                id: format!("{}-{number}", args.project_id),
                version_number: (*number).into(),
                game_versions: games.iter().map(|g| (*g).into()).collect(),
                loaders: vec!["fabric".into()],
            })
            .collect()
    }

    /// A new Fabric build for the version's newest game version, as the backend makes one.
    fn install_pack(&mut self, args: &PackInstallArgs) -> Result<PackInstalled, AppError> {
        // A pack with a file its author keeps from other apps, until the user downloads it by hand.
        if args.project_id == "mock-adrenaline" && !self.downloaded() {
            return Err(launcher_shared::provider::held_error(&[launcher_shared::provider::HeldFile {
                title: "Entity Culling".into(),
                file_name: "entityculling.jar".into(),
                url: Some("https://www.curseforge.com/minecraft/mc-mods/entityculling/files/1".into()),
                size: 1,
                sha1: "0".repeat(40),
            }]));
        }
        let version = Self::pack_versions(&PackArgs { project_id: args.project_id.clone() })
            .into_iter()
            .find(|v| v.id == args.version_id)
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "mock: no such modpack version"))?;
        let mc = version.game_versions.last().cloned().unwrap_or_default();
        let mut builds = self.builds.borrow_mut();
        let build = builds.create_loader(&args.name, LoaderKind::Fabric, &mc, "0.16.9")?;
        builds.emit_builds();
        toast(Level::Success, Text::key("version_install_success").param("version", &build.name));
        drop(builds);
        self.packs.insert(build.key.clone(), (args.project_id.clone(), args.version_id.clone()));
        Ok(PackInstalled { key: build.key, name: build.name })
    }

    /// The builds installed from a preview modpack, with the pack's newest version when newer.
    fn modpack_builds(&self) -> Vec<ModpackBuild> {
        let names: HashMap<String, String> =
            self.builds.borrow().snapshot().builds.into_iter().map(|b| (b.key, b.name)).collect();
        self.packs
            .iter()
            .filter_map(|(key, (project, version))| {
                let list = Self::pack_versions(&PackArgs { project_id: project.clone() });
                let current = list.iter().find(|v| &v.id == version)?;
                // The build runs the version's newest Minecraft, as `install_pack` makes it.
                let games: Vec<String> = current.game_versions.last().cloned().into_iter().collect();
                Some(ModpackBuild {
                    key: key.clone(),
                    name: names.get(key)?.clone(),
                    project_id: project.clone(),
                    version_id: version.clone(),
                    version_number: current.version_number.clone(),
                    newest: newer_pack_version(&list, version, &games, "fabric"),
                })
            })
            .collect()
    }

    fn update_pack(&mut self, args: &PackUpdateArgs) -> Result<PackUpdated, AppError> {
        let (project, _) = self
            .packs
            .get(&args.key)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, "mock: not a modpack build"))?;
        let version = Self::pack_versions(&PackArgs { project_id: project.clone() })
            .into_iter()
            .find(|v| v.id == args.version_id)
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "mock: no such modpack version"))?;
        self.packs.insert(args.key.clone(), (project, version.id.clone()));
        let name =
            self.builds.borrow().snapshot().builds.into_iter().find(|b| b.key == args.key).map(|b| b.name);
        toast(
            Level::Success,
            Text::key("modpack_updated")
                .param("name", name.unwrap_or_default())
                .param("version", &version.version_number),
        );
        Ok(PackUpdated { key: args.key.clone(), version_number: version.version_number, backups: Vec::new() })
    }

    /// The Installed mock's Modrinth files (see `mock_content`) — Sodium with a newer version — and
    /// what the Modrinth tab installed in this preview.
    fn overview(&self, args: &OverviewArgs) -> Overview {
        let note =
            |file: &str, id: &str, slug: &str, title: &str, version: &str, newer: Option<(&str, &str)>| {
                FileNote {
                    file: file.into(),
                    project_id: id.into(),
                    slug: slug.into(),
                    title: title.into(),
                    version_number: version.into(),
                    update: newer.filter(|(vid, _)| !self.versions.contains(*vid)).map(|(vid, number)| {
                        NewerVersion { version_id: vid.into(), version_number: number.into() }
                    }),
                    url: Some(format!("https://modrinth.com/{}/{slug}", project_type(args.kind))),
                    icon_url: None,
                    installed: false,
                }
            };
        let mut notes = Vec::new();
        if args.key == "aeronautics" && args.kind == ContentKind::Mods {
            notes.push(note(
                "sodium-fabric-0.6.0+mc1.21.1.jar",
                "mock-sodium",
                "sodium",
                "Sodium",
                "0.6.0+mc1.21.1",
                Some(("mock-sodium-0.6.1", "0.6.1+mc1.21.1")),
            ));
            notes.push(note(
                "lithium-fabric-0.14.3+mc1.21.1.jar.disabled",
                "mock-lithium",
                "lithium",
                "Lithium",
                "0.14.3+mc1.21.1",
                None,
            ));
        }
        for id in self.installed.get(&(args.key.clone(), args.kind)).into_iter().flatten() {
            if !notes.iter().any(|n| &n.project_id == id) {
                notes.push(note(&format!("{id}.jar"), id, "", "", "1.0.0", None));
            }
        }
        let updates = args.check_updates.then(|| {
            let available = notes.iter().filter(|n| n.update.is_some()).count();
            let status = match (available, notes.is_empty()) {
                (0, true) => UpdatesStatus::NoEnabled,
                (0, false) => UpdatesStatus::Current,
                _ => UpdatesStatus::Available,
            };
            UpdateSummary { status, available, unchecked: 0 }
        });
        Overview { notes, updates }
    }

    /// Answers `module_invoke` for module `modrinth`.
    /// Answers provider command `cmd` (`provider_*`) for provider `modrinth`.
    pub fn handle(&mut self, cmd: &str, call: &Value) -> Result<Value, AppError> {
        // The preview's CurseForge shows the same catalogue (its content tabs only).
        if !matches!(call["provider"].as_str(), Some(id) if id == module_modrinth::ID || id == "curseforge") {
            return Err(AppError::new(ErrorCode::NotFound, "no such provider")
                .with_param("provider", call["provider"].as_str().unwrap_or_default()));
        }
        let args = call["args"].clone();
        match cmd {
            "provider_search" | "provider_modpacks" if self.fail => {
                Err(AppError::new(ErrorCode::Network, "mock: Modrinth is offline"))
            }
            "provider_modpacks" => to_value(Self::modpacks(&parse(args)?)),
            "provider_modpack_versions" => to_value(Self::pack_versions(&parse(args)?)),
            "provider_install_modpack" => to_value(self.install_pack(&parse(args)?)?),
            "provider_modpack_builds" => to_value(self.modpack_builds()),
            "provider_update_modpack" => to_value(self.update_pack(&parse(args)?)?),
            "provider_search" => to_value(self.search(&parse(args)?)),
            "provider_plan" => to_value(self.plan(&parse(args)?)?),
            "provider_overview" => to_value(self.overview(&parse(args)?)),
            "provider_install" => {
                let args: InstallArgs = parse(args)?;
                let plan = self.plan(&args)?;
                let changes = plan.changes(&[]);
                if !plan.can_install() || !changes.iter().all(|c| args.approved.contains(c)) {
                    return to_value(InstallAnswer::Replanned(Box::new(plan)));
                }
                let set = self.installed.entry((args.key.clone(), args.kind)).or_default();
                set.extend(changes.iter().map(|c| c.project_id.clone()));
                self.versions.extend(args.version_id.clone());
                let updated = plan.main.as_ref().is_some_and(|m| m.action == Action::Replace);
                let done = if updated { update_texts(args.kind).done } else { installed_key(args.kind) };
                toast(Level::Success, Text::key(done).param("name", args.title.clone()));
                to_value(InstallAnswer::Installed(InstallOutcome {
                    project_id: args.project_id,
                    filename: format!("{}.jar", args.slug),
                    version_number: "1.0.0".into(),
                }))
            }
            other => {
                Err(AppError::new(ErrorCode::InvalidInput, format!("mock modrinth has no command {other}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(args: Value) -> Value {
        json!({"provider": "modrinth", "args": args})
    }

    fn mock() -> MockModrinth {
        MockModrinth::new(false, Rc::new(RefCell::new(MockBuilds::new(false, false))))
    }

    #[test]
    fn the_mock_installs_a_modpack_as_a_new_build() {
        let builds = Rc::new(RefCell::new(MockBuilds::new(true, false)));
        let mut mock = MockModrinth::new(false, builds.clone());
        let page: SearchPage = serde_json::from_value(
            mock.handle("provider_modpacks", &call(json!({"query": "", "offset": 0}))).unwrap(),
        )
        .unwrap();
        assert_eq!((page.hits.len(), page.limit), (20, 20));
        assert!(page.total > 20, "the preview shows the page switcher");
        let found: SearchPage = serde_json::from_value(
            mock.handle("provider_modpacks", &call(json!({"query": "optim", "offset": 0}))).unwrap(),
        )
        .unwrap();
        assert_eq!(found.hits.len(), 2);
        let pack = page.hits[0].clone();
        let versions: Vec<PackVersion> = serde_json::from_value(
            mock.handle("provider_modpack_versions", &call(json!({"project_id": pack.project_id}))).unwrap(),
        )
        .unwrap();
        assert!(versions.len() > 1);
        let install = |mock: &mut MockModrinth, name: &str| {
            mock.handle(
                "provider_install_modpack",
                &call(json!({"project_id": pack.project_id, "version_id": versions[0].id, "name": name})),
            )
        };
        let done: PackInstalled = serde_json::from_value(install(&mut mock, "Швидка").unwrap()).unwrap();
        let snapshot = builds.borrow().snapshot();
        let build = snapshot.builds.iter().find(|b| b.key == done.key).unwrap();
        assert_eq!((build.name.as_str(), build.client.as_deref()), ("Швидка", Some("Fabric")));
        assert_eq!(install(&mut mock, "швидка").unwrap_err().code, ErrorCode::VersionExists);
        let none: Vec<PackVersion> = serde_json::from_value(
            mock.handle("provider_modpack_versions", &call(json!({"project_id": "mock-pack-empty"})))
                .unwrap(),
        )
        .unwrap();
        assert!(none.is_empty(), "a pack without versions shows the dialog's error");
    }

    #[test]
    fn the_mock_overview_offers_sodium_s_update_until_it_is_installed() {
        let mut mock = mock();
        let ask = |mock: &mut MockModrinth| -> Overview {
            let args = json!({"key": "aeronautics", "kind": "mods", "check_updates": true});
            serde_json::from_value(mock.handle("provider_overview", &call(args)).unwrap()).unwrap()
        };
        let before = ask(&mut mock);
        let sodium = before.notes.iter().find(|n| n.project_id == "mock-sodium").unwrap();
        assert_eq!(sodium.file, "sodium-fabric-0.6.0+mc1.21.1.jar");
        let newer = sodium.update.clone().unwrap();
        assert_eq!(before.updates.unwrap().status, UpdatesStatus::Available);
        let args = json!({"key": "aeronautics", "kind": "mods", "project_id": "mock-sodium", "slug": "sodium",
                          "title": "Sodium", "version_id": newer.version_id, "approved": []});
        let plan: PlanDto =
            serde_json::from_value(mock.handle("provider_plan", &call(args.clone())).unwrap()).unwrap();
        let main = plan.main.as_ref().map(|m| (m.version_id.clone(), m.action));
        assert_eq!(
            main,
            Some((newer.version_id.clone(), Action::Replace)),
            "the exact newer version replaces"
        );
        // As the UI does: the approved plan names the version.
        let base: InstallArgs = serde_json::from_value(args).unwrap();
        mock.handle("provider_install", &call(serde_json::to_value(base.approve(&plan, &[])).unwrap()))
            .unwrap();
        let after = ask(&mut mock);
        assert!(after.notes.iter().all(|n| n.update.is_none()));
        assert_eq!(after.updates.unwrap().status, UpdatesStatus::Current);
        let packs: Overview = serde_json::from_value(
            mock.handle(
                "provider_overview",
                &call(json!({"key": "aeronautics", "kind": "resourcepacks", "check_updates": true})),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(packs.updates.map(|u| u.status), Some(UpdatesStatus::NoEnabled), "packs are checked too");
    }

    #[test]
    fn the_mock_plans_and_installs_with_dependencies() {
        let mut mock = mock();
        let search = mock
            .handle(
                "provider_search",
                &call(json!({"key": "aero", "kind": "mods", "query": "", "offset": 0})),
            )
            .unwrap();
        let first: SearchPage = serde_json::from_value(search).unwrap();
        assert_eq!((first.hits.len(), first.total), (16, 18));
        let args = |id: &str, title: &str| json!({"key": "aero", "kind": "mods", "project_id": id, "slug": id, "title": title});
        let plan: PlanDto = serde_json::from_value(
            mock.handle("provider_plan", &call(args("mock-sodium-extra", "Sodium Extra"))).unwrap(),
        )
        .unwrap();
        assert!(plan.requires_confirmation());
        assert_eq!(
            plan.install.iter().map(|i| i.project_id.as_str()).collect::<Vec<_>>(),
            ["mock-sodium", "mock-fabric-api"]
        );
        let mut approved = args("mock-sodium-extra", "Sodium Extra");
        approved["approved"] = serde_json::to_value(plan.changes(&[])).unwrap();
        let done: InstallAnswer =
            serde_json::from_value(mock.handle("provider_install", &call(approved)).unwrap()).unwrap();
        assert!(matches!(done, InstallAnswer::Installed(_)));
        let overview: Overview = serde_json::from_value(
            mock.handle(
                "provider_overview",
                &call(json!({"key": "aero", "kind": "mods", "check_updates": false})),
            )
            .unwrap(),
        )
        .unwrap();
        let ids: Vec<&str> = overview.notes.iter().map(|n| n.project_id.as_str()).collect();
        assert!(ids.contains(&"mock-sodium-extra") && ids.contains(&"mock-fabric-api"), "{ids:?}");
        // Unapproved changes come back as a plan; blocked plans cannot install.
        let unapproved: InstallAnswer = serde_json::from_value(
            mock.handle("provider_install", &call(args("mock-iris", "Iris"))).unwrap(),
        )
        .unwrap();
        assert!(matches!(unapproved, InstallAnswer::Replanned(_)));
        let reeses: PlanDto = serde_json::from_value(
            mock.handle("provider_plan", &call(args("mock-reeses", "Reese's"))).unwrap(),
        )
        .unwrap();
        assert!(!reeses.can_install() && reeses.blocking[0].code == "required_file_only");
        assert_eq!(
            mock.handle("provider_plan", &call(args("mock-broken", "Broken"))).unwrap_err().code,
            ErrorCode::NoCompatibleVersion
        );
        let offline = MockModrinth::new(true, Rc::new(RefCell::new(MockBuilds::new(false, false))))
            .handle(
                "provider_search",
                &call(json!({"key": "aero", "kind": "mods", "query": "", "offset": 0})),
            )
            .unwrap_err();
        assert_eq!(offline.code, ErrorCode::Network);
    }

    #[test]
    fn the_mock_answers_for_the_preview_s_providers_only() {
        let wrong = json!({"provider": "nowhere", "args": {"query": "", "offset": 0}});
        assert_eq!(mock().handle("provider_modpacks", &wrong).unwrap_err().code, ErrorCode::NotFound);
        let curseforge = json!({"provider": "curseforge", "args": {"key": "aero", "kind": "mods", "query": "", "offset": 0}});
        assert!(
            mock().handle("provider_search", &curseforge).is_ok(),
            "CurseForge's content shows as Modrinth's"
        );
    }

    #[test]
    fn an_approved_plan_installs_at_once_as_the_ui_sends_it() {
        let mut mock = mock();
        let base =
            InstallArgs::new("aero", ContentKind::Mods, "mock-sodium-extra", "sodium-extra", "Sodium Extra");
        let plan: PlanDto = serde_json::from_value(
            mock.handle("provider_plan", &call(serde_json::to_value(&base).unwrap())).unwrap(),
        )
        .unwrap();
        let approved = serde_json::to_value(base.approve(&plan, &[])).unwrap();
        let done: InstallAnswer =
            serde_json::from_value(mock.handle("provider_install", &call(approved)).unwrap()).unwrap();
        assert!(
            matches!(done, InstallAnswer::Installed(_)),
            "a new project with its version is no update: {done:?}"
        );
    }

    #[test]
    fn a_mock_modpack_build_offers_its_update_until_updated() {
        let builds = Rc::new(RefCell::new(MockBuilds::new(true, false)));
        let mut mock = MockModrinth::new(false, builds.clone());
        let versions: Vec<PackVersion> = serde_json::from_value(
            mock.handle("provider_modpack_versions", &call(json!({"project_id": "mock-fo"}))).unwrap(),
        )
        .unwrap();
        // 6.1.0 runs 1.21.1, as 6.2.0 does; 5.9.0 (1.20.6) has no newer version for its Minecraft.
        let older = versions[1].id.clone();
        let done: PackInstalled = serde_json::from_value(
            mock.handle(
                "provider_install_modpack",
                &call(json!({"project_id": "mock-fo", "version_id": older, "name": "FO"})),
            )
            .unwrap(),
        )
        .unwrap();
        let listed = |mock: &mut MockModrinth| -> Vec<ModpackBuild> {
            serde_json::from_value(mock.handle("provider_modpack_builds", &call(json!(null))).unwrap())
                .unwrap()
        };
        let before = listed(&mut mock);
        let mine = before.iter().find(|b| b.key == done.key).unwrap();
        assert_eq!(mine.newest.as_ref().map(|v| v.id.clone()), Some(versions[0].id.clone()));
        mock.handle("provider_update_modpack", &call(json!({"key": done.key, "version_id": versions[0].id})))
            .unwrap();
        assert!(listed(&mut mock).iter().find(|b| b.key == done.key).unwrap().newest.is_none());
    }
}
