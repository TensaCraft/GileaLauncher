//! The server builds module in the browser preview: two server builds to install from Home (and
//! the Modpacks page); an installed one is a Fabric build of the server's client, whose Mods tab
//! offers three optional mods (`?tensa=off` starts with Home's switch off, `?tensa=error` with
//! the server away).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use launcher_shared::provider::{
    Action, FileNote, InstallAnswer, InstallArgs, InstallOutcome, Overview, OverviewArgs, PACKS_LIMIT,
    PackInstalled, PackVersion, PlanDto, PlanItem, ProjectHit, SEARCH_LIMIT, SearchArgs, SearchPage,
    UpdateSummary, UpdatesStatus,
};
use launcher_shared::{AppError, AppResult, ErrorCode, Level, LoaderKind, Text};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::mock_builds::{MockBuilds, toast};

/// Id, name, loader, Minecraft, description.
type Pack = (&'static str, &'static str, LoaderKind, &'static str, Option<&'static str>);

const PACKS: [Pack; 2] = [
    (
        "aero",
        "Aero",
        LoaderKind::Fabric,
        "26.3",
        Some("Літаки, дирижаблі та небесні острови.\nСервер play.example"),
    ),
    ("tensa-lite", "Tensa Lite", LoaderKind::NeoForge, "1.21.1", None),
];

/// Id, title, file.
type Optional = (&'static str, &'static str, &'static str);

const OPTIONAL: [Optional; 3] = [
    ("cyber-goggles", "Create: Cyber Goggles", "CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar"),
    ("barrel-roll", "Do a Barrel Roll", "do_a_barrel_roll-neoforge-3.7.3+1.21.jar"),
    ("e4mc", "e4mc", "e4mc-neoforge-6.2.3.jar"),
];

pub struct MockTensa {
    builds: Rc<RefCell<MockBuilds>>,
    show: bool,
    fails: bool,
    /// Packs installed in this preview.
    installed: Vec<&'static str>,
    /// The optional mods installed, by build.
    optional: HashMap<String, BTreeSet<&'static str>>,
}

fn parse<T: DeserializeOwned>(value: &Value) -> AppResult<T> {
    serde_json::from_value(value.clone()).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

fn to_value<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

impl MockTensa {
    pub fn new(show: bool, fails: bool, builds: Rc<RefCell<MockBuilds>>) -> MockTensa {
        MockTensa { builds, show, fails, installed: Vec::new(), optional: HashMap::new() }
    }

    fn is_server_build(&self, key: &str) -> bool {
        self.builds.borrow().client_of(key).is_some_and(|c| c.eq_ignore_ascii_case(module_tensa::CLIENT))
    }

    fn optional_plan(&self, args: &InstallArgs) -> AppResult<PlanDto> {
        let Some(&(id, title, file)) = OPTIONAL.iter().find(|(id, ..)| *id == args.project_id) else {
            return Err(AppError::new(ErrorCode::NotFound, "mock").with_param("name", &args.project_id));
        };
        let there = self.optional.get(&args.key).is_some_and(|set| set.contains(id));
        let main = PlanItem {
            project_id: id.into(),
            version_id: format!("{id}-sha"),
            title: title.into(),
            version_number: file.into(),
            filename: file.into(),
            action: if there { Action::Satisfied } else { Action::Install },
            current: None,
            url: None,
            unrecognized: false,
        };
        Ok(PlanDto {
            main: Some(main),
            install: Vec::new(),
            replace: Vec::new(),
            satisfied: Vec::new(),
            optional: Vec::new(),
            optional_issues: Vec::new(),
            embedded: Vec::new(),
            blocking: Vec::new(),
        })
    }

    /// Answers provider command `cmd` (`provider_*`) for provider `tensa`.
    pub fn handle_provider(&mut self, cmd: &str, call: &Value) -> AppResult<Value> {
        let args = &call["args"];
        match cmd {
            "provider_modpacks" => {
                let hits: Vec<ProjectHit> = PACKS
                    .iter()
                    .map(|(id, name, kind, mc, description)| ProjectHit {
                        project_id: (*id).into(),
                        slug: (*id).into(),
                        title: (*name).into(),
                        author: format!("{} {mc}", kind.display_name()),
                        description: description.unwrap_or_default().into(),
                        downloads: 0,
                        icon_url: None,
                        url: None,
                    })
                    .collect();
                let total = hits.len() as u32;
                to_value(SearchPage { hits, total, offset: 0, limit: PACKS_LIMIT })
            }
            "provider_modpack_versions" => {
                let id = args["project_id"].as_str().unwrap_or_default();
                let found = PACKS.iter().find(|(pack, ..)| *pack == id);
                let versions: Vec<PackVersion> = found
                    .map(|(id, _, kind, mc, _)| PackVersion {
                        id: (*id).into(),
                        version_number: kind.display_name().into(),
                        game_versions: vec![(*mc).into()],
                        loaders: vec![kind.display_name().to_lowercase()],
                    })
                    .into_iter()
                    .collect();
                to_value(versions)
            }
            "provider_install_modpack" => {
                let installed = self.install(args["project_id"].as_str().unwrap_or_default())?;
                to_value(PackInstalled {
                    key: installed["key"].as_str().unwrap_or_default().into(),
                    name: installed["name"].as_str().unwrap_or_default().into(),
                    skipped: Vec::new(),
                })
            }
            "provider_search" => {
                let args: SearchArgs = parse(args)?;
                let query = args.query.trim().to_lowercase();
                let hits: Vec<ProjectHit> = if self.is_server_build(&args.key) {
                    OPTIONAL
                        .iter()
                        .filter(|(_, title, file)| {
                            query.is_empty()
                                || title.to_lowercase().contains(&query)
                                || file.to_lowercase().contains(&query)
                        })
                        .map(|(id, title, file)| ProjectHit {
                            project_id: (*id).into(),
                            slug: (*id).into(),
                            title: (*title).into(),
                            author: (*file).into(),
                            description: String::new(),
                            downloads: 0,
                            icon_url: None,
                            url: None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let total = hits.len() as u32;
                to_value(SearchPage { hits, total, offset: args.offset, limit: SEARCH_LIMIT })
            }
            "provider_plan" => to_value(self.optional_plan(&parse(args)?)?),
            "provider_install" => {
                let args: InstallArgs = parse(args)?;
                let plan = self.optional_plan(&args)?;
                let changes = plan.changes(&[]);
                if !changes.iter().all(|c| args.approved.contains(c)) {
                    return to_value(InstallAnswer::Replanned(Box::new(plan)));
                }
                let main = plan.main.clone().unwrap_or_else(|| unreachable!("the plan has its item"));
                if let Some(&(id, ..)) = OPTIONAL.iter().find(|(id, ..)| *id == main.project_id) {
                    self.optional.entry(args.key.clone()).or_default().insert(id);
                }
                toast(
                    Level::Success,
                    Text::key("tensa_optional_installed").param("name", main.title.clone()),
                );
                to_value(InstallAnswer::Installed(InstallOutcome {
                    project_id: main.project_id,
                    filename: main.filename,
                    version_number: main.version_number,
                }))
            }
            "provider_overview" => {
                let args: OverviewArgs = parse(args)?;
                let there = self.optional.get(&args.key).cloned().unwrap_or_default();
                let notes: Vec<FileNote> = OPTIONAL
                    .iter()
                    .filter(|(id, ..)| there.contains(id))
                    .map(|(id, title, file)| FileNote {
                        file: (*file).into(),
                        project_id: (*id).into(),
                        slug: (*id).into(),
                        title: (*title).into(),
                        version_number: (*file).into(),
                        update: None,
                        url: None,
                        icon_url: None,
                        installed: true,
                    })
                    .collect();
                let updates = args.check_updates.then_some(UpdateSummary {
                    status: UpdatesStatus::Current,
                    available: 0,
                    unchecked: 0,
                });
                to_value(Overview { notes, updates })
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock tensa: no {other}"))),
        }
    }

    fn home_packs(&self) -> AppResult<Value> {
        if !self.show {
            return Ok(json!([]));
        }
        if self.fails {
            return Err(AppError::new(ErrorCode::Network, "mock"));
        }
        let packs: Vec<Value> = PACKS
            .iter()
            .filter(|(id, ..)| !self.installed.contains(id))
            .map(|(id, name, kind, mc, description)| {
                json!({
                    "id": id, "name": name, "description": description, "image": null,
                    "runs": format!("{} {mc}", kind.display_name())
                })
            })
            .collect();
        Ok(Value::Array(packs))
    }

    fn install(&mut self, pack_id: &str) -> AppResult<Value> {
        let Some(&(id, name, kind, mc, _)) = PACKS.iter().find(|(id, ..)| *id == pack_id) else {
            return Err(AppError::new(ErrorCode::NotFound, "mock").with_param("pack", pack_id));
        };
        let mut builds = self.builds.borrow_mut();
        let build = builds.create_loader(name, kind, mc, "0.19.5")?;
        builds.set_client(&build.key, "TensaCraft");
        builds.emit_builds();
        self.installed.push(id);
        Ok(json!({"key": build.key, "name": build.name}))
    }

    /// Answers `module_invoke` for module `tensa`.
    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        match command {
            "home_packs" => self.home_packs(),
            "install" => self.install(args["pack_id"].as_str().unwrap_or_default()),
            "settings" => Ok(json!({"show": self.show})),
            "set_settings" => {
                self.show = args["show"].as_bool().unwrap_or(true);
                self.builds.borrow().emit_builds();
                Ok(Value::Null)
            }
            "force_sync" => {
                let key = args["key"].as_str().unwrap_or_default();
                let name = self.builds.borrow().name_of(key).unwrap_or_else(|| key.to_string());
                let title = Text::key("tensacraft_force_sync_of").param("version", &name);
                crate::mock_ops::run(title, "sync", Text::key("syncing_files"), 30, move || {
                    toast(Level::Success, Text::key("tensacraft_force_sync_complete").param("version", name));
                });
                Ok(Value::Null)
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock tensa: no {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::ContentKind;

    use super::*;

    fn call(args: Value) -> Value {
        json!({"provider": module_tensa::ID, "args": args})
    }

    #[test]
    fn the_preview_offers_optional_mods_to_its_server_builds() {
        let builds = Rc::new(RefCell::new(MockBuilds::new(true, false)));
        let mut tensa = MockTensa::new(true, false, builds.clone());
        let key = tensa.install("aero").unwrap()["key"].as_str().unwrap().to_string();
        let search = json!({"key": key, "kind": "mods", "query": "", "offset": 0});
        let page: SearchPage =
            serde_json::from_value(tensa.handle_provider("provider_search", &call(search)).unwrap()).unwrap();
        assert_eq!(page.hits.len(), 3);
        let args = InstallArgs::new(&key, ContentKind::Mods, "e4mc", "e4mc", "e4mc");
        let plan: PlanDto =
            serde_json::from_value(tensa.handle_provider("provider_plan", &call(json!(args))).unwrap())
                .unwrap();
        let approved = InstallArgs { approved: plan.changes(&[]), ..args };
        tensa.handle_provider("provider_install", &call(json!(approved))).unwrap();
        let overview = json!({"key": key, "kind": "mods", "check_updates": true});
        let overview: Overview =
            serde_json::from_value(tensa.handle_provider("provider_overview", &call(overview)).unwrap())
                .unwrap();
        assert_eq!(overview.notes.iter().map(|n| n.project_id.as_str()).collect::<Vec<_>>(), ["e4mc"]);
        let other = builds.borrow_mut().create_loader("Own", LoaderKind::Fabric, "26.3", "0.19.5").unwrap();
        let search = json!({"key": other.key, "kind": "mods", "query": "", "offset": 0});
        let page: SearchPage =
            serde_json::from_value(tensa.handle_provider("provider_search", &call(search)).unwrap()).unwrap();
        assert!(page.hits.is_empty(), "not a server build");
    }
}
