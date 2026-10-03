//! What installing a CurseForge project takes: the file that fits the build, the projects its
//! file needs (however deep), the ones it offers, the ones it carries inside and the ones it
//! cannot live with.

use std::collections::{HashMap, HashSet, VecDeque};

use launcher_shared::provider::{Action, Pick, PlanDto, PlanIssue, PlanItem};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode};
use serde_json::{Value, json};

use super::api::CurseForgeApi;
use super::catalog::{InstallFile, blocked, file_fits, file_page, pick_file, text};
use super::held::{Finder, Held};
use crate::types::loader_types;

/// CurseForge's relation types.
const EMBEDDED: u64 = 1;
const OPTIONAL: u64 = 2;
const REQUIRED: u64 = 3;
const INCOMPATIBLE: u64 = 5;

/// A project's file in the build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledFile {
    pub file_id: u64,
    pub filename: String,
    /// Its version as CurseForge names it (the file's display name), else as the jar does.
    pub version: String,
    /// Where it lies in the build now (`mods/x.jar`, or `mods/x.jar.disabled` switched off).
    pub relative: String,
    /// When CurseForge published it, when known.
    pub date: Option<String>,
    /// CurseForge's release type (1 release, 2 beta, 3 alpha), when known.
    pub release_type: Option<u64>,
}

/// What the build has.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// CurseForge's projects, by project id: recorded at install or found by fingerprint.
    pub by_project: HashMap<u64, InstalledFile>,
    /// The enabled mod jars CurseForge does not know, by mod id (`mod_key`): a project whose slug
    /// is that id is taken for it.
    pub by_mod_id: HashMap<String, InstalledFile>,
}

impl Installed {
    /// The build's file of `project`, and whether it is surely the project's (not only by its
    /// mod id).
    fn of(&self, project: &Value) -> Option<(&InstalledFile, bool)> {
        let id = project["id"].as_u64().unwrap_or_default();
        self.by_project
            .get(&id)
            .map(|f| (f, true))
            .or_else(|| self.by_mod_id.get(&mod_key(&text(project, "slug"))).map(|f| (f, false)))
    }
}

/// A mod id (or a slug) as two names of one mod compare.
pub fn mod_key(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('-', "_")
}

/// The build the plan is for.
pub struct Target<'a> {
    pub kind: ContentKind,
    pub game_version: Option<&'a str>,
    /// The build's loader (`launcher_shared::provider::loader_name`); used for mods only.
    pub loader: Option<&'static str>,
    pub installed: &'a Installed,
    /// Where files CurseForge keeps from other apps are looked for; `None`: nowhere.
    pub finder: Option<&'a Finder>,
}

impl Target<'_> {
    /// The loader files must be for (mods only).
    fn mod_loader(&self) -> Option<&'static str> {
        (self.kind == ContentKind::Mods).then_some(self.loader).flatten()
    }

    /// The one loader type CurseForge may narrow the build's files to; a build running more than one
    /// (Quilt) gets every file of its version, narrowed here.
    fn loader_type(&self) -> Option<u32> {
        match self.mod_loader().map(loader_types).as_deref() {
            Some([one]) => Some(*one),
            _ => None,
        }
    }
}

/// A project and the file the plan takes for it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub project: Value,
    pub file: Value,
    pub action: Action,
    /// For a replacement: what is installed now.
    pub current: Option<String>,
    /// For a replacement: where the replaced file lies now.
    pub replaces: Option<String>,
    /// It replaces a file CurseForge does not know as this project's (found by its mod id): the
    /// user confirms it first.
    pub unrecognized: bool,
}

impl Candidate {
    fn new(project: Value, file: Value, action: Action) -> Candidate {
        Candidate { project, file, action, current: None, replaces: None, unrecognized: false }
    }

    pub fn project_id(&self) -> u64 {
        self.project["id"].as_u64().unwrap_or_default()
    }

    pub fn file_id(&self) -> u64 {
        self.file["id"].as_u64().unwrap_or_default()
    }

    pub fn item(&self) -> PlanItem {
        let url = text(&self.project["links"], "websiteUrl");
        PlanItem {
            project_id: self.project_id().to_string(),
            version_id: self.file_id().to_string(),
            title: text(&self.project, "name"),
            version_number: version_label(&text(&self.project, "name"), &text(&self.file, "displayName")),
            filename: text(&self.file, "fileName"),
            action: self.action,
            current: self.current.clone(),
            url: (!url.is_empty()).then_some(url),
            unrecognized: self.unrecognized,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub main: Candidate,
    pub install: Vec<Candidate>,
    pub replace: Vec<Candidate>,
    pub satisfied: Vec<Candidate>,
    pub optional: Vec<Candidate>,
    pub optional_issues: Vec<PlanIssue>,
    pub embedded: Vec<PlanIssue>,
    pub blocking: Vec<PlanIssue>,
    /// Files CurseForge keeps from other apps that were found elsewhere, by file id.
    pub found: HashMap<u64, InstallFile>,
}

impl Plan {
    pub fn to_dto(&self) -> PlanDto {
        let items = |list: &[Candidate]| list.iter().map(Candidate::item).collect();
        PlanDto {
            main: Some(self.main.item()),
            install: items(&self.install),
            replace: items(&self.replace),
            satisfied: items(&self.satisfied),
            optional: items(&self.optional),
            optional_issues: self.optional_issues.clone(),
            embedded: self.embedded.clone(),
            blocking: self.blocking.clone(),
        }
    }

    /// What the install changes, in order: replacements, new dependencies, then the project.
    pub fn pending(&self) -> Vec<&Candidate> {
        self.replace
            .iter()
            .chain(&self.install)
            .chain(std::iter::once(&self.main))
            .filter(|c| c.action != Action::Satisfied)
            .collect()
    }
}

fn issue(code: &str, project: Option<&Value>, id: u64, url: Option<String>, blocking: bool) -> PlanIssue {
    let name = project.map(|p| text(p, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string());
    PlanIssue { code: code.into(), name: Some(name), file_name: None, url, blocking, held: None }
}

/// The file's dependencies: (project id, relation type).
fn dependencies(file: &Value) -> Vec<(u64, u64)> {
    file.get("dependencies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| Some((d["modId"].as_u64()?, d["relationType"].as_u64()?)))
        .collect()
}

/// The file kept from other apps: an issue naming its page and the file to download there.
fn blocked_issue(project: &Value, file: &Value, blocking: bool) -> PlanIssue {
    let page = file_page(&text(&project["links"], "websiteUrl"), file["id"].as_u64().unwrap_or_default());
    PlanIssue {
        held: Held::of(project, file).map(|h| h.to_dto()),
        ..issue(
            "file_blocked",
            Some(project),
            project["id"].as_u64().unwrap_or_default(),
            Some(page),
            blocking,
        )
    }
}

/// The installed file of a project as a file of the plan.
fn installed_file(file: &InstalledFile) -> Value {
    json!({"id": file.file_id, "fileName": file.filename, "displayName": file.version})
}

/// What the build already has of `project`, as a plan's candidate.
fn kept(project: Value, now: &InstalledFile) -> Candidate {
    Candidate::new(project, installed_file(now), Action::Satisfied)
}

struct Resolver<'a, 'b> {
    api: &'a CurseForgeApi,
    target: &'a Target<'b>,
    projects: HashMap<u64, Value>,
    /// Held files found elsewhere, by file id.
    found: HashMap<u64, InstallFile>,
}

impl Resolver<'_, '_> {
    /// `file` is kept from other apps and found nowhere else (`Finder`); one found is noted for
    /// the install.
    async fn held(&mut self, project: &Value, file: &Value) -> bool {
        if !blocked(file) {
            return false;
        }
        let (Some(finder), Some(held)) = (self.target.finder, Held::of(project, file)) else { return true };
        match finder.find(std::slice::from_ref(&held)).await.remove(&held.sha1) {
            Some(found) => {
                self.found.insert(file["id"].as_u64().unwrap_or_default(), found);
                false
            }
            None => true,
        }
    }

    /// Fetches (once) the projects of `ids`.
    async fn know(&mut self, ids: &[u64]) -> AppResult<()> {
        let missing: Vec<u64> = ids.iter().copied().filter(|id| !self.projects.contains_key(id)).collect();
        for project in self.api.mods(&missing).await? {
            if let Some(id) = project["id"].as_u64() {
                self.projects.insert(id, project);
            }
        }
        Ok(())
    }

    fn project(&self, id: u64) -> Value {
        self.projects.get(&id).cloned().unwrap_or_else(|| json!({"id": id, "name": id.to_string()}))
    }

    /// The file of project `id` the build takes: `exact` when it fits, else the newest release.
    async fn file(&self, id: u64, exact: Option<u64>) -> AppResult<Option<Value>> {
        let target = self.target;
        let files = self.api.files(id, target.game_version, target.loader_type()).await?;
        let fits = |f: &&Value| file_fits(f, target.game_version, target.mod_loader());
        let chosen = exact
            .and_then(|wanted| files.iter().filter(fits).find(|f| f["id"].as_u64() == Some(wanted)))
            .or_else(|| pick_file(&files, target.game_version, target.mod_loader()));
        Ok(chosen.cloned())
    }
}

/// The project itself: its file, unless the build has it already, or has a newer one and the
/// user asked for no file in particular (never a downgrade); a jar known by its mod id only is
/// replaced once the user confirms.
fn main_candidate(project: &Value, file: Value, exact: Option<u64>, installed: &Installed) -> Candidate {
    let chosen = file["id"].as_u64().unwrap_or_default();
    let published = text(&file, "fileDate");
    match installed.of(project) {
        Some((now, true)) if now.file_id == chosen => kept(project.clone(), now),
        Some((now, true))
            if exact.is_none() && now.date.as_deref().is_some_and(|d| d >= published.as_str()) =>
        {
            kept(project.clone(), now)
        }
        Some((now, surely)) => Candidate {
            current: Some(version_label(&text(project, "name"), &now.version)),
            replaces: Some(now.relative.clone()),
            unrecognized: !surely,
            ..Candidate::new(project.clone(), file, Action::Replace)
        },
        None => Candidate::new(project.clone(), file, Action::Install),
    }
}

/// The plan of installing project `project_id` (its file `exact` when that still fits) into the
/// target, with the optional dependencies the user `picks`.
pub async fn plan(
    api: &CurseForgeApi,
    target: &Target<'_>,
    project_id: u64,
    exact: Option<u64>,
    picks: &[Pick],
) -> AppResult<Plan> {
    let installed = target.installed;
    let mut resolver = Resolver { api, target, projects: HashMap::new(), found: HashMap::new() };
    let project = api.mod_info(project_id).await?;
    resolver.projects.insert(project_id, project.clone());
    let file = resolver.file(project_id, exact).await?.ok_or_else(|| {
        AppError::new(ErrorCode::NoCompatibleVersion, format!("no file of {project_id} fits"))
            .with_param("name", text(&project, "name"))
    })?;
    let main = main_candidate(&project, file, exact, installed);
    let file = main.file.clone();
    let mut plan = Plan {
        main,
        install: Vec::new(),
        replace: Vec::new(),
        satisfied: Vec::new(),
        optional: Vec::new(),
        optional_issues: Vec::new(),
        embedded: Vec::new(),
        blocking: Vec::new(),
        found: HashMap::new(),
    };
    if plan.main.action != Action::Satisfied && resolver.held(&project, &file).await {
        plan.blocking.push(blocked_issue(&project, &file, true));
    }
    let picked: HashMap<u64, Option<u64>> =
        picks.iter().filter_map(|p| Some((p.project_id.parse().ok()?, p.version_id.parse().ok()))).collect();
    let mut seen: HashSet<u64> = HashSet::from([project_id]);
    let mut queue: VecDeque<(u64, Option<u64>)> = VecDeque::new();
    let own = dependencies(&file);
    resolver.know(&own.iter().map(|(id, _)| *id).collect::<Vec<_>>()).await?;
    for (id, relation) in own {
        let dependency = resolver.project(id);
        match relation {
            EMBEDDED => plan.embedded.push(issue("embedded_dependency", Some(&dependency), id, None, false)),
            OPTIONAL if picked.contains_key(&id) => queue.push_back((id, picked[&id])),
            OPTIONAL if seen.insert(id) => {
                if let Some((now, _)) = installed.of(&dependency) {
                    plan.optional.push(kept(dependency, now));
                    continue;
                }
                match resolver.file(id, None).await? {
                    Some(file) if resolver.held(&dependency, &file).await => {
                        plan.optional_issues.push(blocked_issue(&dependency, &file, false))
                    }
                    Some(file) => plan.optional.push(Candidate::new(dependency, file, Action::Install)),
                    None => plan.optional_issues.push(issue(
                        "dependency_no_file",
                        Some(&dependency),
                        id,
                        None,
                        false,
                    )),
                }
            }
            REQUIRED => queue.push_back((id, None)),
            INCOMPATIBLE if installed.of(&dependency).is_some() => {
                plan.blocking.push(issue("incompatible_installed", Some(&dependency), id, None, true))
            }
            _ => {}
        }
    }
    while let Some((id, exact)) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        resolver.know(&[id]).await?;
        let dependency = resolver.project(id);
        if let Some((now, _)) = installed.of(&dependency) {
            plan.satisfied.push(kept(dependency, now));
            continue;
        }
        let Some(file) = resolver.file(id, exact).await? else {
            plan.blocking.push(issue("dependency_no_file", Some(&dependency), id, None, true));
            continue;
        };
        if resolver.held(&dependency, &file).await {
            plan.blocking.push(blocked_issue(&dependency, &file, true));
            continue;
        }
        for (next, relation) in dependencies(&file) {
            match relation {
                REQUIRED if !seen.contains(&next) => queue.push_back((next, None)),
                INCOMPATIBLE => {
                    resolver.know(&[next]).await?;
                    let other = resolver.project(next);
                    if installed.of(&other).is_some() {
                        plan.blocking.push(issue("incompatible_installed", Some(&other), next, None, true));
                    }
                }
                _ => {}
            }
        }
        plan.install.push(Candidate::new(dependency, file, Action::Install));
    }
    plan.found = resolver.found;
    Ok(plan)
}

/// A file's version as a plan line shows it beside the project's name: CurseForge's display name
/// without the name it often starts with ("Create 6.0.10 for mc1.21.1" → "6.0.10 for mc1.21.1").
pub fn version_label(title: &str, display: &str) -> String {
    let (title, display) = (title.trim(), display.trim());
    let lead = display.get(..title.len()).filter(|lead| lead.eq_ignore_ascii_case(title));
    let rest = lead.map(|_| &display[title.len()..]);
    match rest {
        // The name, then a separator before the version (not a longer word).
        Some(rest) if rest.starts_with([' ', '-', ':', '_']) => {
            let version = rest.trim_start_matches([' ', '-', ':', '_']);
            if version.is_empty() { display.to_string() } else { version.to_string() }
        }
        _ => display.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_does_not_repeat_the_project_s_name() {
        assert_eq!(version_label("Create", "Create 6.0.10 for mc1.21.1"), "6.0.10 for mc1.21.1");
        assert_eq!(
            version_label("Just Enough Items (JEI)", "jei-1.21.1-neoforge-19.51.0.418"),
            "jei-1.21.1-neoforge-19.51.0.418"
        );
        assert_eq!(version_label("Sodium", "sodium - 0.8.13"), "0.8.13", "case and separators");
        assert_eq!(version_label("Create", "Create"), "Create", "nothing left: the name stays");
        assert_eq!(version_label("Create", "Created 1.0"), "Created 1.0", "a word that only starts the same");
    }
}
