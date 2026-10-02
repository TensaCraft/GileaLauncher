//! The dependency plan of a Modrinth install (the original's
//! `build_dependency_plan`): the project's version, what each dependency resolves to and whether it
//! is to install, replace or already there, the optional ones to offer, and what blocks the install.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode};
use serde_json::{Value, json};

use super::api::ModrinthApi;
use super::catalog::{InstallFile, compatible, primary_file, text, version_fits};
use super::inventory::{Found, InstalledItem, Inventory};
use super::provenance::{Record, algorithm_name};
use crate::types::modrinth_url;
use launcher_shared::provider::{Action, Change, Pick, PlanDto, PlanIssue, PlanItem};

/// Fabric API, which every Fabric mod needs even when its version does not say so.
pub const FABRIC_API: &str = "P7dR8mSH";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Selected,
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub project_type: String,
    /// Modrinth answered for it.
    pub resolved: bool,
}

impl Project {
    /// A project the caller knows (a search hit).
    pub fn named(id: &str, slug: &str, title: &str, project_type: &str) -> Project {
        let slug = if slug.trim().is_empty() { id } else { slug.trim() };
        let title = if title.trim().is_empty() { id } else { title.trim() };
        Project {
            id: id.to_string(),
            slug: slug.to_string(),
            title: title.to_string(),
            project_type: project_type.to_string(),
            resolved: true,
        }
    }

    fn from_json(raw: &Value, id: &str) -> Project {
        let id = Some(text(raw, "id")).filter(|s| !s.is_empty()).unwrap_or_else(|| id.to_string());
        Project::named(&id, &text(raw, "slug"), &text(raw, "title"), &text(raw, "project_type"))
    }

    /// Modrinth did not answer for it: its id stands for its name, and it has no page.
    fn unknown(id: &str) -> Project {
        Project {
            id: id.into(),
            slug: String::new(),
            title: id.into(),
            project_type: String::new(),
            resolved: false,
        }
    }

    /// Its page on modrinth.com.
    pub fn url(&self) -> Option<String> {
        (self.resolved && !self.slug.is_empty()).then(|| modrinth_url(&self.project_type, &self.slug))
    }
}

/// A project version the plan would put in the build.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub project: Project,
    pub version: Value,
    pub file: InstallFile,
    pub action: Action,
    pub kind: DepKind,
    /// The project's file in the build, when there is one.
    pub installed: Option<InstalledItem>,
    /// That file was found by its name, not known as the project's.
    pub hinted: bool,
}

impl Candidate {
    pub fn version_id(&self) -> String {
        text(&self.version, "id")
    }

    /// The installed file this candidate takes the place of.
    pub fn replaced(&self) -> Option<&InstalledItem> {
        self.installed.as_ref().filter(|_| self.action == Action::Replace)
    }

    pub fn change(&self) -> Change {
        Change { project_id: self.project.id.clone(), version_id: self.version_id(), action: self.action }
    }

    /// Its provenance record.
    pub fn record(&self, kind: ContentKind) -> Record {
        Record {
            kind,
            filename: self.file.filename.clone(),
            project_id: self.project.id.clone(),
            project_slug: self.project.slug.clone(),
            project_title: self.project.title.clone(),
            version_id: self.version_id(),
            version_number: text(&self.version, "version_number"),
            hash_algorithm: algorithm_name(self.file.hash.kind),
            file_hash: self.file.hash.hex.clone(),
        }
    }

    pub fn item(&self) -> PlanItem {
        let current = self.replaced().map(|i| {
            i.version_number.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| i.filename.clone())
        });
        PlanItem {
            project_id: self.project.id.clone(),
            version_id: self.version_id(),
            title: self.project.title.clone(),
            version_number: text(&self.version, "version_number"),
            filename: self.file.filename.clone(),
            action: self.action,
            current,
            url: self.project.url(),
            unrecognized: self.hinted && self.replaced().is_some(),
        }
    }
}

/// What keeps a dependency out of the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub code: &'static str,
    pub blocking: bool,
    pub project: Option<Project>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub file_name: Option<String>,
}

impl Issue {
    fn to_dto(&self) -> PlanIssue {
        let name = self
            .project
            .as_ref()
            .map(|p| p.title.clone())
            .or_else(|| self.project_id.clone())
            .or_else(|| self.file_name.clone())
            .or_else(|| self.version_id.clone());
        PlanIssue {
            code: self.code.to_string(),
            name,
            file_name: self.file_name.clone(),
            url: self.project.as_ref().and_then(Project::url),
            blocking: self.blocking,
            held: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub main: Option<Candidate>,
    pub install: Vec<Candidate>,
    pub replace: Vec<Candidate>,
    pub satisfied: Vec<Candidate>,
    pub optional: Vec<Candidate>,
    pub embedded: Vec<Issue>,
    pub blocking: Vec<Issue>,
    pub optional_issues: Vec<Issue>,
}

impl Plan {
    pub fn can_install(&self) -> bool {
        self.main.is_some() && self.blocking.is_empty()
    }

    /// Replacements, new dependencies, then the project (picked optional dependencies are
    /// required ones by now).
    pub fn install_order(&self) -> Vec<&Candidate> {
        self.replace.iter().chain(&self.install).chain(self.main.iter()).collect()
    }

    pub fn to_dto(&self) -> PlanDto {
        let items = |list: &[Candidate]| list.iter().map(Candidate::item).collect();
        let issues = |list: &[Issue]| list.iter().map(Issue::to_dto).collect();
        PlanDto {
            main: self.main.as_ref().map(Candidate::item),
            install: items(&self.install),
            replace: items(&self.replace),
            satisfied: items(&self.satisfied),
            optional: items(&self.optional),
            optional_issues: issues(&self.optional_issues),
            embedded: issues(&self.embedded),
            blocking: issues(&self.blocking),
        }
    }
}

/// What the plan is for.
pub struct Target<'a> {
    pub kind: ContentKind,
    /// Modrinth's name of the build's loader (only mods are narrowed by it).
    pub loader: Option<&'static str>,
    pub game_version: Option<&'a str>,
    pub inventory: &'a Inventory,
}

fn published(version: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&text(version, "date_published")).ok().map(|d| d.with_timezone(&Utc))
}

/// `candidate` came out after `current` (a tie goes to the larger id).
pub fn newer(candidate: &Value, current: &Value) -> bool {
    let (a, b) = (published(candidate), published(current));
    if a != b {
        return a > b;
    }
    text(candidate, "id") > text(current, "id")
}

/// The newest of `versions` by date (the first of equals).
fn newest(versions: Vec<&Value>) -> Option<Value> {
    versions
        .into_iter()
        .enumerate()
        .max_by(|(i, a), (j, b)| published(a).cmp(&published(b)).then(j.cmp(i)))
        .map(|(_, v)| v.clone())
}

/// The plan of installing `project` — `exact` when the user saw a version, else its newest
/// compatible one — with the optional dependencies they `picked`.
pub async fn plan(
    api: &ModrinthApi,
    target: &Target<'_>,
    project: Project,
    exact: Option<Value>,
    picked: &[Pick],
) -> AppResult<Plan> {
    let (version, is_exact) = match exact {
        Some(version) => (version, true),
        None => {
            let versions = api.project_versions(&project.id, target.loader, target.game_version).await?;
            let Some(first) =
                compatible(&versions, target.loader, target.game_version).into_iter().next().cloned()
            else {
                return Err(AppError::new(
                    ErrorCode::NoCompatibleVersion,
                    format!("{} has no version for this build", project.id),
                )
                .with_param("name", project.title.clone()));
            };
            (first, false)
        }
    };
    let Some(file) = primary_file(&version) else {
        return Err(AppError::new(
            ErrorCode::NoFileFound,
            format!("{} lists no file to install", project.id),
        )
        .with_param("name", project.title.clone()));
    };
    let mut resolver = Resolver::new(api, target);
    let main = match resolver.candidate(project, version, file, DepKind::Selected, is_exact).await {
        Ok(main) => main,
        Err(issue) => return Ok(Plan { blocking: vec![*issue], ..Plan::default() }),
    };
    if target.kind != ContentKind::Mods {
        return Ok(Plan { main: Some(main), ..Plan::default() });
    }
    Ok(resolver.dependencies(main, picked).await)
}

type Answer = (Option<Candidate>, Vec<Issue>);

struct Resolver<'a, 'b> {
    api: &'a ModrinthApi,
    target: &'a Target<'b>,
    projects: HashMap<String, Project>,
    versions: HashMap<String, Option<Value>>,
    answers: HashMap<(String, String, String, bool), Answer>,
}

impl<'a, 'b> Resolver<'a, 'b> {
    fn new(api: &'a ModrinthApi, target: &'a Target<'b>) -> Self {
        Resolver { api, target, projects: HashMap::new(), versions: HashMap::new(), answers: HashMap::new() }
    }

    fn fits(&self, version: &Value) -> bool {
        version_fits(version, self.target.loader, self.target.game_version)
    }

    async fn project(&mut self, id: &str) -> Project {
        if let Some(project) = self.projects.get(id) {
            return project.clone();
        }
        let project = match self.api.project(id).await {
            Ok(raw) => Project::from_json(&raw, id),
            Err(_) => Project::unknown(id),
        };
        self.projects.insert(id.to_string(), project.clone());
        project
    }

    async fn version(&mut self, id: &str) -> Option<Value> {
        if let Some(version) = self.versions.get(id) {
            return version.clone();
        }
        let version = self.api.version(id).await.ok().filter(|v| text(v, "id") == id);
        self.versions.insert(id.to_string(), version.clone());
        version
    }

    async fn newest_compatible(&mut self, project_id: &str) -> Option<Value> {
        let versions =
            self.api.project_versions(project_id, self.target.loader, self.target.game_version).await.ok()?;
        newest(compatible(&versions, self.target.loader, self.target.game_version))
    }

    async fn installed_version(&mut self, item: &InstalledItem) -> Option<Value> {
        let id = item.version_id.clone().filter(|v| !v.is_empty())?;
        match &item.version {
            Some(version) => Some(version.clone()).filter(|v| text(v, "id") == id),
            None => self.version(&id).await,
        }
    }

    /// The original's `_candidate_action`.
    async fn action(&mut self, installed: Option<&InstalledItem>, version: &Value, exact: bool) -> Action {
        let Some(item) = installed else { return Action::Install };
        if !item.enabled {
            return Action::Replace;
        }
        let installed_id = item.version_id.clone().unwrap_or_default();
        if !installed_id.is_empty() && installed_id == text(version, "id") {
            return Action::Satisfied;
        }
        if installed_id.is_empty() || exact {
            return Action::Replace;
        }
        let Some(current) = self.installed_version(item).await else { return Action::Replace };
        if self.fits(&current) && !newer(version, &current) { Action::Satisfied } else { Action::Replace }
    }

    async fn candidate(
        &mut self,
        project: Project,
        version: Value,
        file: InstallFile,
        kind: DepKind,
        exact: bool,
    ) -> Result<Candidate, Box<Issue>> {
        let inventory = self.target.inventory;
        let found = inventory.find(&project.id, &project.slug, &project.title);
        let hinted = matches!(found, Found::Hinted(_));
        let installed = match found {
            // Several copies are here already: which to keep is the user's call — say which.
            Found::Ambiguous => {
                let files: Vec<String> = inventory
                    .copies(&project.id, &project.slug, &project.title)
                    .iter()
                    .map(|i| i.filename.clone())
                    .collect();
                return Err(Box::new(Issue {
                    code: "dependency_duplicate_copies",
                    blocking: kind != DepKind::Optional,
                    project_id: Some(project.id.clone()),
                    version_id: Some(text(&version, "id")),
                    file_name: Some(files.join(", ")),
                    project: Some(project),
                }));
            }
            Found::Owned(item) => Some(item.clone()),
            // A copy Modrinth does not name: its version is unknown, so it is replaced (the original's HINT) — the plan shows its file and the new version.
            Found::Hinted(item) => {
                let mut copy = item.clone();
                (copy.version_id, copy.version_number, copy.version) = (None, None, None);
                Some(copy)
            }
            Found::None => None,
        };
        let action = self.action(installed.as_ref(), &version, exact).await;
        Ok(Candidate { project, version, file, action, kind, installed, hinted })
    }

    async fn issue(
        &mut self,
        code: &'static str,
        dependency: &Value,
        blocking: bool,
        project_id: Option<String>,
        version_id: Option<String>,
    ) -> Issue {
        let or_dependency = |value: Option<String>, key: &str| {
            value.filter(|s| !s.is_empty()).or_else(|| Some(text(dependency, key)).filter(|s| !s.is_empty()))
        };
        let project_id = or_dependency(project_id, "project_id");
        let version_id = or_dependency(version_id, "version_id");
        let file_name = or_dependency(None, "file_name");
        let project = match &project_id {
            Some(id) => Some(self.project(id).await),
            None => None,
        };
        Issue { code, blocking, project, project_id, version_id, file_name }
    }

    async fn resolve(&mut self, dependency: &Value, kind: DepKind) -> Answer {
        let key = (
            text(dependency, "project_id"),
            text(dependency, "version_id"),
            text(dependency, "file_name"),
            kind == DepKind::Optional,
        );
        if let Some(answer) = self.answers.get(&key) {
            return answer.clone();
        }
        let answer = self.resolve_uncached(dependency, kind).await;
        self.answers.insert(key, answer.clone());
        answer
    }

    /// The original's `_resolve_dependency_candidate`.
    async fn resolve_uncached(&mut self, dependency: &Value, kind: DepKind) -> Answer {
        let blocking = kind != DepKind::Optional;
        let mut project_id = text(dependency, "project_id");
        let version_id = text(dependency, "version_id");
        let version = if !version_id.is_empty() {
            let Some(exact) = self.version(&version_id).await else {
                let issue =
                    self.issue("dependency_resolution_failed", dependency, blocking, None, None).await;
                return (None, vec![issue]);
            };
            let exact_project = text(&exact, "project_id");
            if exact_project.is_empty() || (!project_id.is_empty() && exact_project != project_id) {
                let id = if project_id.is_empty() { exact_project } else { project_id };
                let issue =
                    self.issue("dependency_project_mismatch", dependency, blocking, Some(id), None).await;
                return (None, vec![issue]);
            }
            project_id = exact_project;
            if !self.fits(&exact) {
                let issue =
                    self.issue("dependency_incompatible", dependency, blocking, Some(project_id), None).await;
                return (None, vec![issue]);
            }
            exact
        } else if !project_id.is_empty() {
            let inventory = self.target.inventory;
            let mut chosen = None;
            if let Found::Owned(item) = inventory.find(&project_id, "", "")
                && item.enabled
                && let Some(current) = self.installed_version(item).await
                && text(&current, "project_id") == project_id
                && self.fits(&current)
            {
                chosen = Some(current);
            }
            if chosen.is_none() {
                chosen = self.newest_compatible(&project_id).await;
            }
            let Some(version) = chosen else {
                let issue =
                    self.issue("dependency_resolution_failed", dependency, blocking, None, None).await;
                return (None, vec![issue]);
            };
            version
        } else {
            let issue = self.issue("required_file_only", dependency, blocking, None, None).await;
            return (None, vec![issue]);
        };
        let Some(file) = primary_file(&version) else {
            let issue = self
                .issue(
                    "dependency_no_file",
                    dependency,
                    blocking,
                    Some(project_id),
                    Some(text(&version, "id")),
                )
                .await;
            return (None, vec![issue]);
        };
        let project = self.project(&project_id).await;
        match self.candidate(project, version, file, kind, !version_id.is_empty()).await {
            Ok(candidate) => (Some(candidate), Vec::new()),
            Err(issue) => (None, vec![*issue]),
        }
    }

    /// The original's `_resolve_required_dependencies`: passes until the chosen versions settle.
    async fn dependencies(&mut self, main: Candidate, picked: &[Pick]) -> Plan {
        let picked_ids: HashSet<&str> = picked.iter().map(|p| p.project_id.as_str()).collect();
        let mut roots: Vec<Value> = main
            .version
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|d| d.is_object())
            .cloned()
            .collect();
        roots.extend(picked.iter().map(|p| json!({"project_id": p.project_id, "version_id": p.version_id, "dependency_type": "required"})));
        let implicit = self.target.loader == Some("fabric") && main.project.id != FABRIC_API;
        let mut previous: HashMap<String, Candidate> = HashMap::new();
        let mut seen: HashSet<Vec<(String, String)>> = HashSet::from([Vec::new()]);
        loop {
            let mut resolved: HashMap<String, Candidate> = HashMap::new();
            let mut order: Vec<String> = Vec::new();
            let mut pinned: HashMap<String, String> =
                HashMap::from([(main.project.id.clone(), main.version_id())]);
            let mut optional: HashMap<String, Candidate> = HashMap::new();
            let mut optional_order: Vec<String> = Vec::new();
            let mut optional_satisfied: Vec<Candidate> = Vec::new();
            let (mut optional_issues, mut embedded, mut issues) = (Vec::new(), Vec::new(), Vec::new());
            let mut incompatible: Vec<Value> = Vec::new();
            let mut traversed: HashSet<String> = HashSet::from([main.project.id.clone()]);
            let mut queue: VecDeque<Value> = roots.iter().cloned().collect();
            let mut implicit_checked = !implicit;
            while !queue.is_empty() || !implicit_checked {
                if queue.is_empty() {
                    implicit_checked = true;
                    if resolved.contains_key(FABRIC_API) {
                        break;
                    }
                    queue.push_back(json!({"project_id": FABRIC_API, "dependency_type": "required"}));
                }
                let Some(dependency) = queue.pop_front() else { break };
                let kind = match text(&dependency, "dependency_type").to_lowercase() {
                    k if k.is_empty() => "required".to_string(),
                    k => k,
                };
                match kind.as_str() {
                    "optional" => {
                        if picked_ids.contains(text(&dependency, "project_id").as_str()) {
                            continue;
                        }
                        let (candidate, errors) = self.resolve(&dependency, DepKind::Optional).await;
                        optional_issues.extend(errors);
                        let Some(candidate) = candidate else { continue };
                        if candidate.project.id == main.project.id
                            || picked_ids.contains(candidate.project.id.as_str())
                        {
                            continue;
                        }
                        if candidate.action == Action::Satisfied {
                            optional_satisfied.push(candidate);
                        } else {
                            let id = candidate.project.id.clone();
                            match optional.get(&id) {
                                Some(existing) if !newer(&candidate.version, &existing.version) => {}
                                Some(_) => {
                                    optional.insert(id, candidate);
                                }
                                None => {
                                    optional_order.push(id.clone());
                                    optional.insert(id, candidate);
                                }
                            }
                        }
                        continue;
                    }
                    "embedded" => {
                        embedded
                            .push(self.issue("embedded_dependency", &dependency, false, None, None).await);
                        continue;
                    }
                    "incompatible" => {
                        incompatible.push(dependency);
                        continue;
                    }
                    "required" => {}
                    _ => {
                        optional_issues.push(
                            self.issue("unsupported_dependency_type", &dependency, false, None, None).await,
                        );
                        continue;
                    }
                }
                let (candidate, errors) = self.resolve(&dependency, DepKind::Required).await;
                issues.extend(errors);
                let Some(candidate) = candidate else { continue };
                let id = candidate.project.id.clone();
                let exact = text(&dependency, "version_id");
                let pin = pinned.get(&id).cloned();
                if !exact.is_empty() && pin.as_ref().is_some_and(|p| *p != exact) {
                    issues.push(
                        self.issue("dependency_version_conflict", &dependency, true, Some(id), None).await,
                    );
                    continue;
                }
                if id == main.project.id {
                    continue;
                }
                if !exact.is_empty() {
                    pinned.insert(id.clone(), exact.clone());
                }
                let take = match resolved.get(&id) {
                    None => true,
                    Some(existing) => {
                        !exact.is_empty() || (pin.is_none() && newer(&candidate.version, &existing.version))
                    }
                };
                if take {
                    if !resolved.contains_key(&id) {
                        order.push(id.clone());
                    }
                    resolved.insert(id.clone(), candidate.clone());
                }
                if !traversed.insert(id.clone()) {
                    continue;
                }
                let chosen = previous.get(&id).cloned().unwrap_or(candidate);
                queue.extend(
                    chosen
                        .version
                        .get("dependencies")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|d| d.is_object())
                        .cloned(),
                );
            }
            let signature = |map: &HashMap<String, Candidate>| {
                let mut pairs: Vec<(String, String)> =
                    map.iter().map(|(k, c)| (k.clone(), c.version_id())).collect();
                pairs.sort();
                pairs
            };
            let (now, before) = (signature(&resolved), signature(&previous));
            if now != before {
                if seen.insert(now) {
                    previous = resolved;
                    continue;
                }
                issues.push(Issue {
                    code: "dependency_version_conflict",
                    blocking: true,
                    project: Some(main.project.clone()),
                    project_id: Some(main.project.id.clone()),
                    version_id: None,
                    file_name: None,
                });
            }
            let inventory = self.target.inventory;
            for dependency in incompatible {
                let id = text(&dependency, "project_id");
                if id.is_empty() {
                    continue;
                }
                let planned = id == main.project.id || resolved.contains_key(&id);
                if !planned && !matches!(inventory.find(&id, "", ""), Found::Owned(_) | Found::Hinted(_)) {
                    continue;
                }
                issues.push(self.issue("incompatible_installed", &dependency, true, Some(id), None).await);
            }
            let candidates: Vec<Candidate> =
                order.iter().filter_map(|id| resolved.get(id).cloned()).collect();
            let by = |action: Action| {
                candidates.iter().filter(|c| c.action == action).cloned().collect::<Vec<_>>()
            };
            let mut satisfied = by(Action::Satisfied);
            satisfied
                .extend(optional_satisfied.into_iter().filter(|c| !resolved.contains_key(&c.project.id)));
            return Plan {
                install: by(Action::Install),
                replace: by(Action::Replace),
                satisfied,
                optional: optional_order.iter().filter_map(|id| optional.get(id).cloned()).collect(),
                main: Some(main),
                embedded,
                blocking: issues,
                optional_issues,
            };
        }
    }
}
