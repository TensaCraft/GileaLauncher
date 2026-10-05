//! The content-provider contract's data (`ContentProvider`): what a provider offers —
//! every part optional — and what the app and a provider say to each other. Modrinth is the first
//! provider; CurseForge and Tensa follow.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{AppError, ContentKind, ErrorCode};

/// What a provider offers; everything is optional — the app asks for nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Its module's id.
    pub id: String,
    /// Its name as users know it ("Modrinth"); texts name the provider with it.
    pub name: String,
    /// Its Material icon.
    pub icon: String,
    /// Kinds it searches and installs into a build (a choice on those "Build content" tabs).
    #[serde(default)]
    pub content: Vec<ContentKind>,
    /// Kinds whose installed files it checks for newer versions.
    #[serde(default)]
    pub updates: Vec<ContentKind>,
    /// It searches modpacks and installs one as a new build.
    #[serde(default)]
    pub modpacks: bool,
    /// It finds the builds it installed from modpacks and updates them to another version.
    #[serde(default)]
    pub modpack_updates: bool,
}

/// What the app asks a provider for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Content(ContentKind),
    Updates(ContentKind),
    Modpacks,
    ModpackUpdates,
}

impl ProviderInfo {
    pub fn offers(&self, need: Need) -> bool {
        match need {
            Need::Content(kind) => self.content.contains(&kind),
            Need::Updates(kind) => self.updates.contains(&kind),
            Need::Modpacks => self.modpacks,
            Need::ModpackUpdates => self.modpack_updates,
        }
    }
}

/// The provider-named text of an error, where the shared one says too little (`{provider}` and the
/// error's params fill it): a lost connection (the shared text names no server), a file that
/// changed while the build's files were identified, or one the provider answered for
/// with another file's data (those carry the file's `name`).
pub fn provider_error_key(error: &AppError) -> Option<&'static str> {
    let named = error.params.contains_key("name");
    match error.code {
        ErrorCode::Io if named => Some("provider_file_changed"),
        ErrorCode::Network if named => Some("provider_file_mismatch"),
        ErrorCode::Network => Some("provider_unreachable"),
        _ => None,
    }
}

/// Results per page (the original's mods manager).
pub const SEARCH_LIMIT: u32 = 16;

/// `search`: one page of this build's kind of content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchArgs {
    pub key: String,
    pub kind: ContentKind,
    pub query: String,
    pub offset: u32,
}

/// `plan` / `install`: a project's install, with what the user saw and approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallArgs {
    pub key: String,
    pub kind: ContentKind,
    pub project_id: String,
    pub slug: String,
    pub title: String,
    /// The version of the project the user saw (`None`: its newest compatible one).
    #[serde(default)]
    pub version_id: Option<String>,
    /// Optional dependencies the user picked.
    #[serde(default)]
    pub optional: Vec<Pick>,
    /// The changes the user agreed to; an install that would make others answers with the new plan.
    #[serde(default)]
    pub approved: Vec<Change>,
    /// The project's own file only: its dependencies — required, optional or incompatible — are
    /// not looked at.
    #[serde(default)]
    pub alone: bool,
}

impl InstallArgs {
    /// An install of the project's newest compatible version, nothing picked or approved yet.
    pub fn new(
        key: impl Into<String>,
        kind: ContentKind,
        project_id: impl Into<String>,
        slug: impl Into<String>,
        title: impl Into<String>,
    ) -> InstallArgs {
        InstallArgs {
            key: key.into(),
            kind,
            project_id: project_id.into(),
            slug: slug.into(),
            title: title.into(),
            version_id: None,
            optional: Vec::new(),
            approved: Vec::new(),
            alone: false,
        }
    }

    /// This install (the one the dialog showed) of the project's own file, its dependencies left
    /// out: the plan's version, and its change alone approved.
    pub fn alone(&self, plan: &PlanDto) -> InstallArgs {
        let main = plan.main.as_ref();
        InstallArgs {
            version_id: main.map(|m| m.version_id.clone()),
            optional: Vec::new(),
            approved: main
                .filter(|m| m.action != Action::Satisfied)
                .map(|m| Change {
                    project_id: m.project_id.clone(),
                    version_id: m.version_id.clone(),
                    action: m.action,
                })
                .into_iter()
                .collect(),
            alone: true,
            ..self.clone()
        }
    }

    /// This install (the one the dialog showed) approved with `picks`: the plan's version, the
    /// earlier picks kept — a re-plan lists them as required, not as optional — and the plan's
    /// changes.
    pub fn approve(&self, plan: &PlanDto, picks: &[PlanItem]) -> InstallArgs {
        let mut optional = self.optional.clone();
        for item in picks {
            if !optional.iter().any(|p| p.project_id == item.project_id) {
                optional
                    .push(Pick { project_id: item.project_id.clone(), version_id: item.version_id.clone() });
            }
        }
        InstallArgs {
            version_id: plan.main.as_ref().map(|m| m.version_id.clone()),
            optional,
            approved: plan.changes(picks),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOutcome {
    pub project_id: String,
    pub filename: String,
    pub version_number: String,
}

/// `install`'s answer: done, or the plan as it is now when it differs from what the user approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "lowercase")]
pub enum InstallAnswer {
    Installed(InstallOutcome),
    Replanned(Box<PlanDto>),
}

/// What an install does with one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Install,
    Replace,
    Satisfied,
}

/// A project of a dependency plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    pub version_number: String,
    pub filename: String,
    pub action: Action,
    /// For a replacement: the installed version (its number, else its file name).
    pub current: Option<String>,
    /// Its page on the provider's site.
    pub url: Option<String>,
    /// It replaces a file the provider does not know as this project's (found by its name):
    /// the user confirms it first.
    #[serde(default)]
    pub unrecognized: bool,
}

/// What keeps a dependency out of a plan (the original's issue codes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssue {
    pub code: String,
    /// The project's title, else its id, the file or the version.
    pub name: Option<String>,
    pub file_name: Option<String>,
    pub url: Option<String>,
    pub blocking: bool,
    /// A file the provider may not hand out, downloaded by hand (`file_blocked`).
    #[serde(default)]
    pub held: Option<HeldFile>,
}

/// A file its provider may not hand to other apps (on CurseForge, its author's choice): the user
/// downloads it from its page, and the launcher finds it in their Downloads folder by its size
/// and SHA-1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldFile {
    /// Its project's name.
    pub title: String,
    pub file_name: String,
    /// Its page, where it is downloaded by hand.
    pub url: Option<String>,
    pub size: u64,
    pub sha1: String,
    /// The same mod on another provider, offered in its place (not the same bytes).
    #[serde(default)]
    pub alternative: Option<HeldAlternative>,
    /// The build's folder it goes in (`mods`, `resourcepacks`…); empty when not known.
    #[serde(default)]
    pub folder: String,
}

/// A held file's mod as another provider has it, for the same game and loader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldAlternative {
    /// The provider's name, as shown.
    pub provider: String,
    pub title: String,
    pub version: String,
    pub file_name: String,
    /// Its page there.
    pub url: Option<String>,
}

impl PlanDto {
    /// The files the plan names to download by hand (its `file_blocked` issues).
    pub fn held_files(&self) -> Vec<HeldFile> {
        self.blocking.iter().chain(&self.optional_issues).filter_map(|i| i.held.clone()).collect()
    }
}

/// The error of an install that needs `held` downloaded by hand first: its `files` list them.
pub fn held_error(held: &[HeldFile]) -> AppError {
    let names: Vec<&str> = held.iter().map(|h| h.title.as_str()).collect();
    AppError::new(
        ErrorCode::ProviderFilesHeld,
        format!("{} files are downloaded by hand: {}", held.len(), names.join(", ")),
    )
    .with_param("count", held.len().to_string())
    .with_param("names", names.join(", "))
    .with_param("files", serde_json::to_string(held).unwrap_or_default())
}

/// The files `error` says are downloaded by hand first (`held_error`); none for another error.
pub fn held_files(error: &AppError) -> Vec<HeldFile> {
    if error.code != ErrorCode::ProviderFilesHeld {
        return Vec::new();
    }
    error.params.get("files").and_then(|files| serde_json::from_str(files).ok()).unwrap_or_default()
}

/// What installing a project takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDto {
    pub main: Option<PlanItem>,
    pub install: Vec<PlanItem>,
    pub replace: Vec<PlanItem>,
    pub satisfied: Vec<PlanItem>,
    pub optional: Vec<PlanItem>,
    pub optional_issues: Vec<PlanIssue>,
    pub embedded: Vec<PlanIssue>,
    pub blocking: Vec<PlanIssue>,
}

/// One change an install makes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Change {
    pub project_id: String,
    pub version_id: String,
    pub action: Action,
}

/// An optional dependency the user picked: exactly this version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub project_id: String,
    pub version_id: String,
}

impl PlanDto {
    pub fn can_install(&self) -> bool {
        self.main.is_some() && self.blocking.is_empty()
    }

    /// The user sees the plan first when it brings more than the project, offers optional
    /// dependencies, or cannot go ahead.
    pub fn requires_confirmation(&self) -> bool {
        self.main.as_ref().is_some_and(|m| m.unrecognized)
            || !self.install.is_empty()
            || !self.replace.is_empty()
            || self.optional.iter().any(|i| i.action != Action::Satisfied)
            || !self.blocking.is_empty()
    }

    /// Its dependencies — to install, offered or in the way — leave something out when the project
    /// is installed alone; its own file to download by hand is needed all the same.
    pub fn offers_alone(&self) -> bool {
        let Some(main) = &self.main else { return false };
        let own = |issue: &PlanIssue| issue.held.as_ref().is_some_and(|h| h.file_name == main.filename);
        !self.install.is_empty()
            || !self.replace.is_empty()
            || !self.optional.is_empty()
            || !self.optional_issues.is_empty()
            || self.blocking.iter().any(|issue| !own(issue))
    }

    /// What installing the plan with the `selected` optional dependencies changes, in order (the
    /// original's `install_order_with_optional`, what is already there left out).
    pub fn changes(&self, selected: &[PlanItem]) -> Vec<Change> {
        let Some(main) = &self.main else { return Vec::new() };
        let mut taken: HashSet<&str> = self
            .replace
            .iter()
            .chain(&self.install)
            .chain(&self.satisfied)
            .map(|i| i.project_id.as_str())
            .collect();
        taken.insert(main.project_id.as_str());
        let mut picked = Vec::new();
        for item in selected.iter().filter(|i| i.action != Action::Satisfied) {
            if taken.insert(item.project_id.as_str()) {
                picked.push(item);
            }
        }
        self.replace
            .iter()
            .chain(&self.install)
            .chain(picked)
            .chain(std::iter::once(main))
            .filter(|i| i.action != Action::Satisfied)
            .map(|i| Change {
                project_id: i.project_id.clone(),
                version_id: i.version_id.clone(),
                action: i.action,
            })
            .collect()
    }
}

/// A plan item's line in the dialog: "name version", or "name: current -> new" for a replacement.
pub fn item_text(item: &PlanItem) -> (&'static str, Vec<(&'static str, String)>) {
    let version =
        if item.version_number.is_empty() { item.filename.clone() } else { item.version_number.clone() };
    if item.action == Action::Replace {
        let current = item.current.clone().unwrap_or_default();
        (
            "modrinth_dependency_replace_line",
            vec![("name", item.title.clone()), ("current", current), ("new", version)],
        )
    } else {
        ("modrinth_dependency_install_line", vec![("name", item.title.clone()), ("version", version)])
    }
}

/// An issue's line in the dialog: a translation key with its parameters, or `None` for the name
/// alone (an embedded dependency). `unknown` names what has no name.
pub fn issue_text(issue: &PlanIssue, unknown: &str) -> (Option<&'static str>, Vec<(&'static str, String)>) {
    let name = issue.name.clone().unwrap_or_else(|| unknown.to_string());
    let key = match issue.code.as_str() {
        "required_file_only" => {
            let file = issue.file_name.clone().unwrap_or_else(|| unknown.to_string());
            return (Some("modrinth_dependency_file_only_issue"), vec![("file", file)]);
        }
        "embedded_dependency" => return (None, vec![("name", name)]),
        "incompatible_installed" => "modrinth_dependency_incompatible_issue",
        "dependency_incompatible" => "modrinth_dependency_incompatible_build_issue",
        "dependency_project_mismatch" => "modrinth_dependency_project_mismatch_issue",
        "dependency_no_file" => "modrinth_dependency_no_file_issue",
        "file_blocked" => "dependency_file_blocked_issue",
        "dependency_version_conflict" => "modrinth_dependency_version_conflict",
        "dependency_duplicate_copies" => {
            let files = issue.file_name.clone().unwrap_or_else(|| unknown.to_string());
            return (Some("modrinth_dependency_duplicate_copies"), vec![("name", name), ("files", files)]);
        }
        _ => "modrinth_dependency_resolution_failed",
    };
    (Some(key), vec![("name", name)])
}

/// A search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    /// Its page on the provider's site.
    #[serde(default)]
    pub url: Option<String>,
}

/// A page of results; the pagination follows the original's `CatalogState`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<ProjectHit>,
    pub total: u32,
    pub offset: u32,
    pub limit: u32,
}

impl SearchPage {
    pub fn has_previous(&self) -> bool {
        self.offset > 0
    }

    pub fn has_next(&self) -> bool {
        self.offset.saturating_add(self.hits.len() as u32) < self.total
    }

    pub fn previous_offset(&self) -> u32 {
        self.offset.saturating_sub(self.limit)
    }

    pub fn next_offset(&self) -> u32 {
        self.offset.saturating_add(self.limit)
    }

    pub fn total_pages(&self) -> u32 {
        self.total.div_ceil(self.limit.max(1)).max(1)
    }

    pub fn current_page(&self) -> u32 {
        (self.offset / self.limit.max(1) + 1).min(self.total_pages())
    }

    /// More results than one page holds.
    pub fn paginated(&self) -> bool {
        self.total > self.limit
    }
}

/// `overview`'s arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewArgs {
    pub key: String,
    pub kind: ContentKind,
    /// Also ask the provider for newer versions (where it checks them).
    #[serde(default)]
    pub check_updates: bool,
}

/// A newer version the provider offers for an installed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewerVersion {
    pub version_id: String,
    pub version_number: String,
}

/// What the provider knows of one installed file of one of its projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNote {
    /// Its name on disk, as the Installed list names it (`ContentItem.file`).
    pub file: String,
    pub project_id: String,
    /// From the provenance; empty for a file only Modrinth named.
    pub slug: String,
    pub title: String,
    pub version_number: String,
    pub update: Option<NewerVersion>,
    /// Its project's page on the provider's site.
    #[serde(default)]
    pub url: Option<String>,
    /// Its project's icon (an https picture), when the provider knows it.
    #[serde(default)]
    pub icon_url: Option<String>,
    /// The launcher installed the file through this provider (on its own or with its modpack).
    #[serde(default)]
    pub installed: bool,
}

/// What a provider has said of a file of a build so far.
#[derive(Debug, Clone, Copy)]
pub enum Heard<'a> {
    /// It has not answered for the build yet.
    Waiting,
    /// It answered and does not know the file.
    Unknown,
    Knows(&'a FileNote),
}

/// The provider a file is shown with (`file_owner`) once no answer still to come can change it
/// (`heard`: in the app's order); `None` until then — a row never moves from one provider to
/// another while the providers answer one after another.
pub fn settled_owner<'a>(heard: &[(&'a str, Heard<'_>)]) -> Option<&'a str> {
    let mut answered_before = true;
    for (id, said) in heard {
        match said {
            Heard::Waiting => answered_before = false,
            // The provider that installed it, and none before it can say the same any more.
            Heard::Knows(note) if note.installed && answered_before => return Some(id),
            _ => {}
        }
    }
    if heard.iter().any(|(_, said)| matches!(said, Heard::Waiting)) {
        return None;
    }
    let notes: Vec<(&'a str, Option<&FileNote>)> = heard
        .iter()
        .map(|(id, said)| (*id, if let Heard::Knows(note) = said { Some(*note) } else { None }))
        .collect();
    file_owner(&notes)
}

/// The provider a file of a build is shown with when several know it (`notes`: each provider's
/// note of the file, in the app's order — Modrinth before CurseForge): the one the launcher
/// installed it through, else the first that knows it. One file, one provider.
pub fn file_owner<'a>(notes: &[(&'a str, Option<&FileNote>)]) -> Option<&'a str> {
    let known = || notes.iter().filter_map(|(id, note)| Some((*id, (*note)?)));
    known().find(|(_, note)| note.installed).or_else(|| known().next()).map(|(id, _)| id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatesStatus {
    Available,
    Unchecked,
    Current,
    NoEnabled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSummary {
    pub status: UpdatesStatus,
    /// Files with a newer version.
    pub available: usize,
    /// Checked files the provider said nothing of.
    pub unchecked: usize,
}

/// The provider's view of a build's installed files of one kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    pub notes: Vec<FileNote>,
    /// `None` when not checked (packs, or not asked).
    pub updates: Option<UpdateSummary>,
}

impl FileNote {
    /// The install that brings this file's newer version; `name` titles it when the provider's
    /// title is not known.
    pub fn update_args(&self, key: &str, kind: ContentKind, name: &str) -> Option<InstallArgs> {
        let newer = self.update.as_ref()?;
        let title = if self.title.is_empty() { name } else { &self.title };
        Some(InstallArgs {
            version_id: Some(newer.version_id.clone()),
            ..InstallArgs::new(key, kind, self.project_id.clone(), self.slug.clone(), title)
        })
    }
}

/// The status line's text key and parameters.
pub fn updates_text(
    summary: &UpdateSummary,
    kind: ContentKind,
) -> (&'static str, Vec<(&'static str, String)>) {
    let texts = update_texts(kind);
    match summary.status {
        UpdatesStatus::Available => {
            ("installed_updates_available", vec![("count", summary.available.to_string())])
        }
        UpdatesStatus::Unchecked => {
            ("installed_updates_unchecked", vec![("count", summary.unchecked.to_string())])
        }
        UpdatesStatus::Current => (texts.current, Vec::new()),
        UpdatesStatus::NoEnabled => (texts.none, Vec::new()),
        UpdatesStatus::Failed => ("installed_updates_failed", Vec::new()),
    }
}

/// Modpacks per page (the original's modpacks page).
pub const PACKS_LIMIT: u32 = 20;

/// `modpacks`: a page of the provider's modpacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacksArgs {
    pub query: String,
    pub offset: u32,
}

/// `modpack_versions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackArgs {
    pub project_id: String,
}

/// A modpack version to choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackVersion {
    pub id: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
}

impl PackVersion {
    /// "{version_number} ({game versions})", as the original's version list.
    pub fn label(&self) -> String {
        format!("{} ({})", self.version_number, self.game_versions.join(", "))
    }

    /// Runs on one of `games` with `loader`; an empty one of them allows any, and so does a version
    /// that names no loader (some older CurseForge packs).
    pub fn runs_on(&self, games: &[String], loader: &str) -> bool {
        (games.is_empty() || self.game_versions.iter().any(|g| games.contains(g)))
            && (loader.is_empty()
                || self.loaders.is_empty()
                || self.loaders.iter().any(|l| l.eq_ignore_ascii_case(loader)))
    }
}

/// The update a modpack build on version `current` is offered: the newest version listed before it
/// (newest first) for the build's Minecraft and loader. Another Minecraft is the user's own choice.
pub fn newer_pack_version(
    list: &[PackVersion],
    current: &str,
    games: &[String],
    loader: &str,
) -> Option<PackVersion> {
    let end = list.iter().position(|v| v.id == current).unwrap_or(list.len());
    list[..end].iter().find(|v| v.runs_on(games, loader)).cloned()
}

/// `install_modpack`: a new build from a modpack version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackInstallArgs {
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    /// Held files (by SHA-1) taken from their alternative instead.
    #[serde(default)]
    pub replace_held: Vec<String>,
    /// Goes on without the held files still missing: the answer names them.
    #[serde(default)]
    pub skip_held: bool,
}

/// A build a provider installed from a modpack, with the newest version when it is newer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModpackBuild {
    pub key: String,
    pub name: String,
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
    pub newest: Option<PackVersion>,
}

/// `update_modpack`: build `key` to modpack version `version_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackUpdateArgs {
    pub key: String,
    pub version_id: String,
    /// Held files (by SHA-1) taken from their alternative instead.
    #[serde(default)]
    pub replace_held: Vec<String>,
    /// Goes on without the held files still missing: the answer names them.
    #[serde(default)]
    pub skip_held: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackUpdated {
    pub key: String,
    pub version_number: String,
    /// Where the files the player had changed and the update replaced were saved (paths in the
    /// build, under `.launcher/pack-backups/`).
    #[serde(default)]
    pub backups: Vec<String>,
    /// Held files left out (`skip_held`): the player adds them by hand.
    #[serde(default)]
    pub skipped: Vec<HeldFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackInstalled {
    pub key: String,
    pub name: String,
    /// Held files left out (`skip_held`): the player adds them by hand.
    #[serde(default)]
    pub skipped: Vec<HeldFile>,
}

/// The build's mod loader as providers name it: the first of fabric, neoforge, forge and quilt in
/// its component (or, without one, its client).
pub fn loader_name(loader: Option<&str>, client: Option<&str>) -> Option<&'static str> {
    let source = loader.filter(|l| !l.trim().is_empty()).or(client).unwrap_or_default().to_lowercase();
    ["fabric", "neoforge", "forge", "quilt"].into_iter().find(|name| source.contains(name))
}

/// The loaders whose mods a build of `loader` runs (providers' names), its own first: Quilt runs
/// Fabric's too.
pub fn loaders_run_by(loader: &str) -> &'static [&'static str] {
    match loader {
        "quilt" => &["quilt", "fabric"],
        "fabric" => &["fabric"],
        "neoforge" => &["neoforge"],
        "forge" => &["forge"],
        _ => &[],
    }
}

pub fn loader_label(name: &str) -> &'static str {
    match name {
        "fabric" => "Fabric",
        "neoforge" => "NeoForge",
        "forge" => "Forge",
        "quilt" => "Quilt",
        _ => "Minecraft",
    }
}

/// The search's filter line: loader and Minecraft version for mods, the version otherwise.
pub fn filter_text(
    kind: ContentKind,
    loader: Option<&str>,
    client: Option<&str>,
    version: Option<&str>,
) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let version = version.map(str::trim).filter(|v| !v.is_empty())?.to_string();
    match (kind, loader_name(loader, client)) {
        (ContentKind::Mods, Some(name)) => {
            Some(("mods_filtered_by", vec![("loader", loader_label(name).to_string()), ("version", version)]))
        }
        _ => Some(("content_filtered_by_version", vec![("version", version)])),
    }
}

/// What a search that found nothing says.
pub fn not_found_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "no_mods_found",
        ContentKind::ResourcePacks => "no_resourcepacks_found",
        ContentKind::ShaderPacks => "no_shaderpacks_found",
    }
}

pub fn search_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "search_mods",
        ContentKind::ResourcePacks => "search_resourcepacks",
        ContentKind::ShaderPacks => "search_shaders",
    }
}

/// How updating content of one kind is told: mods speak of mods and of the backup made first,
/// packs of packs (they are not backed up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateTexts {
    /// The row's action and the confirmation's title.
    pub action: &'static str,
    /// The confirmation's question (`{name}`, `{current}`, `{new}`).
    pub confirm: &'static str,
    /// After it (`{name}`).
    pub done: &'static str,
    /// The status line before any check, while checking, when all is current and with nothing to check.
    pub idle: &'static str,
    pub checking: &'static str,
    pub current: &'static str,
    pub none: &'static str,
}

pub fn update_texts(kind: ContentKind) -> UpdateTexts {
    let packs = UpdateTexts {
        action: "update_resourcepack",
        confirm: "confirm_update_pack",
        done: "resourcepack_updated",
        idle: "installed_updates_idle_packs",
        checking: "installed_updates_checking_packs",
        current: "installed_updates_current_packs",
        none: "installed_updates_no_enabled_packs",
    };
    match kind {
        ContentKind::Mods => UpdateTexts {
            action: "update_mod",
            confirm: "confirm_update_mod",
            done: "mod_updated",
            idle: "installed_updates_idle",
            checking: "installed_updates_checking",
            current: "installed_updates_current",
            none: "installed_updates_no_enabled",
        },
        ContentKind::ResourcePacks => packs,
        ContentKind::ShaderPacks => {
            UpdateTexts { action: "update_shaderpack", done: "shaderpack_updated", ..packs }
        }
    }
}

pub fn installing_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "installing_mod",
        ContentKind::ResourcePacks => "installing_resourcepack",
        ContentKind::ShaderPacks => "installing_shaderpack",
    }
}

pub fn installed_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mod_installed",
        ContentKind::ResourcePacks => "resourcepack_installed",
        ContentKind::ShaderPacks => "shaderpack_installed",
    }
}

/// At most `max` characters, with "…" when cut.
pub fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>().trim_end())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_quilt_build_runs_fabric_mods_too() {
        // Quilt loads Fabric mods: a Quilt build is offered both, its own first.
        assert_eq!(loaders_run_by("quilt"), ["quilt", "fabric"]);
        assert_eq!(loaders_run_by("fabric"), ["fabric"]);
        assert_eq!(loaders_run_by("neoforge"), ["neoforge"]);
        assert_eq!(loaders_run_by("forge"), ["forge"]);
        assert!(loaders_run_by("minecraft").is_empty());
    }

    fn info() -> ProviderInfo {
        ProviderInfo {
            id: "shop".into(),
            name: "Shop".into(),
            icon: "search".into(),
            content: vec![ContentKind::Mods, ContentKind::ResourcePacks],
            updates: vec![ContentKind::Mods],
            modpacks: false,
            modpack_updates: false,
        }
    }

    #[test]
    fn a_provider_offers_only_what_it_says() {
        let shop = info();
        assert!(shop.offers(Need::Content(ContentKind::Mods)));
        assert!(!shop.offers(Need::Content(ContentKind::ShaderPacks)));
        assert!(shop.offers(Need::Updates(ContentKind::Mods)));
        assert!(!shop.offers(Need::Updates(ContentKind::ResourcePacks)));
        assert!(!shop.offers(Need::Modpacks));
        assert!(!shop.offers(Need::ModpackUpdates));
        let bare: ProviderInfo =
            serde_json::from_value(json!({"id": "t", "name": "T", "icon": "x"})).unwrap();
        assert!(
            !bare.offers(Need::Content(ContentKind::Mods)) && !bare.offers(Need::Modpacks),
            "everything is optional"
        );
    }

    fn pack(id: &str, games: &[&str], loaders: &[&str]) -> PackVersion {
        PackVersion {
            id: id.into(),
            version_number: id.into(),
            game_versions: games.iter().map(|g| (*g).into()).collect(),
            loaders: loaders.iter().map(|l| (*l).into()).collect(),
        }
    }

    #[test]
    fn a_build_is_offered_the_newest_version_for_its_minecraft_and_loader() {
        let list = [
            pack("v5", &["26.3"], &["fabric"]),
            pack("v4", &["1.21", "1.21.1"], &["fabric", "quilt"]),
            pack("v3", &["1.21.1"], &["neoforge"]),
            pack("v2", &["1.21.1"], &["fabric"]),
            pack("v1", &["1.21.1"], &["fabric"]),
        ];
        let games = ["1.21.1".to_string()];
        let id = |v: Option<PackVersion>| v.map(|v| v.id);
        assert_eq!(id(newer_pack_version(&list, "v1", &games, "Fabric")), Some("v4".into()));
        assert_eq!(id(newer_pack_version(&list, "v4", &games, "Fabric")), None, "only 26.3 is newer");
        assert_eq!(id(newer_pack_version(&list, "v2", &games, "NeoForge")), Some("v3".into()));
        // No loader or Minecraft known: any.
        assert_eq!(id(newer_pack_version(&list, "v4", &[], "")), Some("v5".into()));
        // The build's version is no longer listed: the newest that fits.
        assert_eq!(id(newer_pack_version(&list, "gone", &games, "fabric")), Some("v4".into()));
    }

    #[test]
    fn modpack_updates_are_their_own_offer() {
        let mut info = info();
        info.modpacks = true;
        assert!(!info.offers(Need::ModpackUpdates), "installing packs is not updating them");
        info.modpack_updates = true;
        assert!(info.offers(Need::ModpackUpdates));
    }

    #[test]
    fn provider_errors_name_the_provider() {
        let error = |code| AppError::new(code, "x");
        assert_eq!(provider_error_key(&error(ErrorCode::Network)), Some("provider_unreachable"));
        assert_eq!(
            provider_error_key(&error(ErrorCode::Network).with_param("name", "a.jar")),
            Some("provider_file_mismatch"),
            "not a connection problem"
        );
        assert_eq!(
            provider_error_key(&error(ErrorCode::Io).with_param("name", "a.jar")),
            Some("provider_file_changed")
        );
        assert_eq!(provider_error_key(&error(ErrorCode::Io)), None);
        assert_eq!(
            provider_error_key(&error(ErrorCode::NoCompatibleVersion)),
            None,
            "others keep their own text"
        );
    }

    #[test]
    fn a_hit_and_a_note_carry_their_page() {
        let hit: ProjectHit = serde_json::from_value(json!({"project_id": "p", "slug": "s", "title": "T",
            "author": "", "description": "", "downloads": 0, "icon_url": null,
            "url": "https://modrinth.com/mod/s"}))
        .unwrap();
        assert_eq!(hit.url.as_deref(), Some("https://modrinth.com/mod/s"));
        let note: FileNote = serde_json::from_value(json!({"file": "a.jar", "project_id": "p", "slug": "",
            "title": "", "version_number": "1", "update": null}))
        .unwrap();
        assert_eq!(note.url, None, "a page is optional");
    }

    #[test]
    fn a_pack_version_reads_like_the_original_s_list() {
        let version = PackVersion {
            id: "v".into(),
            version_number: "5.1".into(),
            game_versions: vec!["1.21.1".into(), "1.21".into()],
            loaders: vec!["fabric".into()],
        };
        assert_eq!(version.label(), "5.1 (1.21.1, 1.21)");
    }

    #[test]
    fn a_pack_version_that_names_no_loader_runs_on_any() {
        let version = |loaders: &[&str]| PackVersion {
            id: "v".into(),
            version_number: "1".into(),
            game_versions: vec!["1.12.2".into()],
            loaders: loaders.iter().map(|l| l.to_string()).collect(),
        };
        let games = ["1.12.2".to_string()];
        assert!(version(&[]).runs_on(&games, "Forge"), "an older pack lists no loader");
        assert!(version(&["forge"]).runs_on(&games, "Forge"));
        assert!(!version(&["fabric"]).runs_on(&games, "Forge"));
        assert!(!version(&[]).runs_on(&["1.20.1".to_string()], "Forge"));
    }

    fn note(slug: &str, title: &str, update: Option<&str>) -> FileNote {
        FileNote {
            file: "sodium.jar".into(),
            project_id: "AANobbMI".into(),
            slug: slug.into(),
            title: title.into(),
            version_number: "0.5".into(),
            update: update.map(|v| NewerVersion { version_id: v.into(), version_number: "0.6".into() }),
            url: None,
            icon_url: None,
            installed: false,
        }
    }

    #[test]
    fn a_note_updates_to_the_exact_newer_version() {
        assert_eq!(note("", "", None).update_args("aero", ContentKind::Mods, "Sodium"), None);
        let args = note("", "", Some("v6")).update_args("aero", ContentKind::Mods, "Sodium").unwrap();
        assert_eq!(
            (args.version_id.as_deref(), args.title.as_str(), args.key.as_str()),
            (Some("v6"), "Sodium", "aero")
        );
        let named =
            note("sodium", "Sodium Extra", Some("v6")).update_args("aero", ContentKind::Mods, "row").unwrap();
        assert_eq!((named.title.as_str(), named.slug.as_str()), ("Sodium Extra", "sodium"));
    }

    #[test]
    fn every_update_status_has_its_line() {
        let line = |status, available, unchecked| {
            updates_text(&UpdateSummary { status, available, unchecked }, ContentKind::Mods)
        };
        assert_eq!(
            line(UpdatesStatus::Available, 3, 1),
            ("installed_updates_available", vec![("count", "3".to_string())])
        );
        assert_eq!(
            line(UpdatesStatus::Unchecked, 0, 2),
            ("installed_updates_unchecked", vec![("count", "2".to_string())])
        );
        assert_eq!(line(UpdatesStatus::Current, 0, 0).0, "installed_updates_current");
        assert_eq!(line(UpdatesStatus::NoEnabled, 0, 0).0, "installed_updates_no_enabled");
        assert_eq!(line(UpdatesStatus::Failed, 0, 0).0, "installed_updates_failed");
        assert_eq!(serde_json::to_value(UpdatesStatus::NoEnabled).unwrap(), serde_json::json!("no_enabled"));
    }

    fn page(offset: u32, shown: usize, total: u32) -> SearchPage {
        let hit = ProjectHit {
            project_id: "p".into(),
            slug: "p".into(),
            title: "P".into(),
            author: String::new(),
            description: String::new(),
            downloads: 0,
            icon_url: None,
            url: None,
        };
        SearchPage { hits: vec![hit; shown], total, offset, limit: SEARCH_LIMIT }
    }

    #[test]
    fn pages_follow_the_originals_catalog_state() {
        let first = page(0, 16, 40);
        assert!(!first.has_previous() && first.has_next() && first.paginated());
        assert_eq!((first.current_page(), first.total_pages(), first.next_offset()), (1, 3, 16));
        let last = page(32, 8, 40);
        assert!(last.has_previous() && !last.has_next());
        assert_eq!((last.current_page(), last.previous_offset()), (3, 16));
        let alone = page(0, 3, 3);
        assert!(!alone.paginated() && !alone.has_next());
        assert_eq!(page(0, 0, 0).total_pages(), 1);
        assert_eq!(page(160, 0, 40).current_page(), 3, "never past the last page");
    }

    #[test]
    fn the_loader_comes_from_the_component_or_the_client() {
        assert_eq!(loader_name(Some("fabric-loader-0.16.9-1.21.1"), Some("Fabric")), Some("fabric"));
        assert_eq!(loader_name(Some("neoforge-21.1.252"), None), Some("neoforge"));
        assert_eq!(loader_name(Some("1.20.1-forge-47.3.0"), None), Some("forge"));
        assert_eq!(loader_name(Some("quilt-loader-0.26.0-1.21.1"), None), Some("quilt"));
        assert_eq!(loader_name(Some(" "), Some("Fabric")), Some("fabric"));
        assert_eq!(loader_name(None, Some("NeoForge")), Some("neoforge"));
        assert_eq!(loader_name(Some("1.21.1"), Some("Minecraft")), None);
        assert_eq!(loader_label("neoforge"), "NeoForge");
    }

    #[test]
    fn the_filter_names_what_narrows_the_search() {
        assert_eq!(
            filter_text(ContentKind::Mods, Some("fabric-loader-0.16.9-1.21.1"), None, Some("1.21.1")),
            Some((
                "mods_filtered_by",
                vec![("loader", "Fabric".to_string()), ("version", "1.21.1".to_string())]
            ))
        );
        assert_eq!(
            filter_text(ContentKind::ShaderPacks, Some("fabric-loader-0.16.9-1.21.1"), None, Some("1.21.1")),
            Some(("content_filtered_by_version", vec![("version", "1.21.1".to_string())]))
        );
        assert_eq!(filter_text(ContentKind::ResourcePacks, None, None, Some(" ")), None);
    }

    #[test]
    fn installing_alone_is_offered_while_dependencies_are_in_the_plan() {
        let bare = PlanDto { main: Some(item("main", Action::Install)), ..PlanDto::default() };
        assert!(!bare.offers_alone(), "nothing to leave out");
        assert!(PlanDto { install: vec![item("lib", Action::Install)], ..bare.clone() }.offers_alone());
        assert!(PlanDto { optional: vec![item("extra", Action::Install)], ..bare.clone() }.offers_alone());
        let stop = |held: Option<HeldFile>| PlanIssue {
            code: "file_blocked".into(),
            name: None,
            file_name: None,
            url: None,
            blocking: true,
            held,
        };
        let held = |file: &str| HeldFile {
            title: "T".into(),
            file_name: file.into(),
            url: None,
            size: 1,
            sha1: "0".repeat(40),
            alternative: None,
            folder: "mods".into(),
        };
        let stopped = PlanDto { blocking: vec![stop(Some(held("lib.jar")))], ..bare.clone() };
        assert!(stopped.offers_alone(), "a dependency that stops it");
        // The project's own file, downloaded by hand: installing it alone needs it all the same.
        let own = PlanDto { blocking: vec![stop(Some(held("main.jar")))], ..bare.clone() };
        assert!(!own.offers_alone());
        let none = PlanDto { install: vec![item("lib", Action::Install)], ..PlanDto::default() };
        assert!(!none.offers_alone(), "no project to install");
    }

    #[test]
    fn installing_alone_approves_the_project_s_file_only() {
        let first = InstallArgs::new("aero", ContentKind::Mods, "main", "main", "Main");
        let earlier = first.approve(
            &PlanDto { optional: vec![item("extra", Action::Install)], ..PlanDto::default() },
            &[item("extra", Action::Install)],
        );
        let plan = PlanDto {
            main: Some(item("main", Action::Replace)),
            install: vec![item("lib", Action::Install)],
            replace: vec![item("api", Action::Replace)],
            blocking: vec![PlanIssue {
                code: "dependency_no_file".into(),
                name: Some("Gone".into()),
                file_name: None,
                url: None,
                blocking: true,
                held: None,
            }],
            ..PlanDto::default()
        };
        let alone = earlier.alone(&plan);
        assert!(alone.alone && alone.optional.is_empty(), "no pick goes along");
        assert_eq!(alone.version_id.as_deref(), Some("main-2"));
        assert_eq!(
            alone.approved,
            [Change { project_id: "main".into(), version_id: "main-2".into(), action: Action::Replace }]
        );
        // What is there already changes nothing.
        let there = PlanDto { main: Some(item("main", Action::Satisfied)), ..plan };
        assert!(first.alone(&there).approved.is_empty());
    }

    #[test]
    fn approving_again_keeps_the_earlier_picks() {
        let first = InstallArgs::new("aero", ContentKind::Mods, "main", "main", "Main");
        let offered = PlanDto {
            main: Some(item("main", Action::Install)),
            optional: vec![item("extra", Action::Install)],
            ..PlanDto::default()
        };
        let picked = first.approve(&offered, &offered.optional);
        assert_eq!(picked.optional, [Pick { project_id: "extra".into(), version_id: "extra-2".into() }]);
        assert_eq!(picked.version_id.as_deref(), Some("main-2"));
        // The re-plan lists the pick as required, with a dependency of its own: approving that plan
        // keeps the pick, or the install would quietly go without it.
        let replanned = PlanDto {
            main: Some(item("main", Action::Install)),
            install: vec![item("extra", Action::Install), item("lib", Action::Install)],
            ..PlanDto::default()
        };
        let again = picked.approve(&replanned, &[]);
        assert_eq!(again.optional, picked.optional);
        assert_eq!(
            again.approved.iter().map(|c| c.project_id.as_str()).collect::<Vec<_>>(),
            ["extra", "lib", "main"]
        );
    }

    #[test]
    fn texts_per_kind() {
        assert_eq!(search_key(ContentKind::Mods), "search_mods");
        assert_eq!(installing_key(ContentKind::ShaderPacks), "installing_shaderpack");
        assert_eq!(installed_key(ContentKind::ResourcePacks), "resourcepack_installed");
    }

    #[test]
    fn counts_and_descriptions_are_short() {
        assert_eq!(clip("  abc  ", 5), "abc");
        assert_eq!(clip(&"я".repeat(200), 150), format!("{}…", "я".repeat(150)));
    }

    fn item(id: &str, action: Action) -> PlanItem {
        PlanItem {
            project_id: id.into(),
            version_id: format!("{id}-2"),
            title: id.to_uppercase(),
            version_number: "2.0".into(),
            filename: format!("{id}.jar"),
            action,
            current: (action == Action::Replace).then(|| "1.0".to_string()),
            url: None,
            unrecognized: false,
        }
    }

    #[test]
    fn a_file_is_shown_with_one_provider() {
        let note = |installed: bool| FileNote {
            file: "a.jar".into(),
            project_id: "p".into(),
            slug: String::new(),
            title: "A".into(),
            version_number: "1".into(),
            update: None,
            url: None,
            icon_url: None,
            installed,
        };
        let (found, put) = (note(false), note(true));
        assert_eq!(file_owner(&[("modrinth", Some(&found)), ("curseforge", Some(&found))]), Some("modrinth"));
        assert_eq!(
            file_owner(&[("modrinth", Some(&found)), ("curseforge", Some(&put))]),
            Some("curseforge"),
            "a CurseForge modpack's file stays CurseForge's"
        );
        assert_eq!(file_owner(&[("modrinth", None), ("curseforge", Some(&found))]), Some("curseforge"));
        assert_eq!(file_owner(&[("modrinth", None), ("curseforge", None)]), None);
    }

    #[test]
    fn a_row_waits_for_the_answers_that_could_change_its_provider() {
        let note = |installed: bool| FileNote {
            file: "a.jar".into(),
            project_id: "p".into(),
            slug: String::new(),
            title: "A".into(),
            version_number: "1".into(),
            update: None,
            url: None,
            icon_url: None,
            installed,
        };
        let (found, put) = (note(false), note(true));
        use Heard::*;
        let owner = |heard: &[(&'static str, Heard<'_>)]| settled_owner(heard);
        assert_eq!(
            owner(&[("modrinth", Waiting), ("curseforge", Knows(&found))]),
            None,
            "Modrinth may know it too"
        );
        assert_eq!(
            owner(&[("modrinth", Knows(&found)), ("curseforge", Waiting)]),
            None,
            "CurseForge may have put it"
        );
        assert_eq!(
            owner(&[("modrinth", Waiting), ("curseforge", Knows(&put))]),
            None,
            "Modrinth may have put it"
        );
        assert_eq!(
            owner(&[("modrinth", Knows(&put)), ("curseforge", Waiting)]),
            Some("modrinth"),
            "nothing can change it"
        );
        assert_eq!(owner(&[("modrinth", Unknown), ("curseforge", Knows(&put))]), Some("curseforge"));
        assert_eq!(owner(&[("modrinth", Knows(&found)), ("curseforge", Knows(&found))]), Some("modrinth"));
        assert_eq!(owner(&[("modrinth", Unknown), ("curseforge", Unknown)]), None);
    }

    #[test]
    fn held_files_travel_in_their_error() {
        let held = vec![HeldFile {
            title: "Held Mod".into(),
            file_name: "held.jar".into(),
            url: Some("https://www.curseforge.com/minecraft/mc-mods/held/files/7".into()),
            size: 5,
            sha1: "b".repeat(40),
            alternative: Some(HeldAlternative {
                provider: "Modrinth".into(),
                title: "Held Mod".into(),
                version: "2.0".into(),
                file_name: "held-2.jar".into(),
                url: Some("https://modrinth.com/mod/held".into()),
            }),
            folder: "mods".into(),
        }];
        let error = held_error(&held);
        assert_eq!(error.code, ErrorCode::ProviderFilesHeld);
        assert_eq!((error.params["count"].as_str(), error.params["names"].as_str()), ("1", "Held Mod"));
        assert_eq!(held_files(&error), held);
        let other = AppError { code: ErrorCode::NoFileFound, ..error };
        assert!(held_files(&other).is_empty());
    }

    #[test]
    fn args_cross_ipc_in_snake_case() {
        let args = InstallArgs::new("aero", ContentKind::ShaderPacks, "p", "s", "T");
        assert_eq!(
            serde_json::to_value(&args).unwrap(),
            serde_json::json!({"key": "aero", "kind": "shaderpacks", "project_id": "p", "slug": "s", "title": "T",
                               "version_id": null, "optional": [], "approved": [], "alone": false})
        );
        let old: InstallArgs = serde_json::from_value(
            serde_json::json!({"key": "aero", "kind": "mods", "project_id": "p", "slug": "s", "title": "T"}),
        )
        .unwrap();
        assert_eq!(old, InstallArgs::new("aero", ContentKind::Mods, "p", "s", "T"));
    }

    #[test]
    fn a_plan_s_changes_are_what_the_user_approves() {
        let plan = PlanDto {
            main: Some(item("main", Action::Install)),
            install: vec![item("lib", Action::Install)],
            replace: vec![item("old", Action::Replace)],
            satisfied: vec![item("fapi", Action::Satisfied)],
            optional: vec![item("extra", Action::Install), item("fapi", Action::Install)],
            ..PlanDto::default()
        };
        let ids = |changes: Vec<Change>| changes.into_iter().map(|c| c.project_id).collect::<Vec<_>>();
        assert_eq!(ids(plan.changes(&[])), ["old", "lib", "main"]);
        assert_eq!(
            ids(plan.changes(&plan.optional)),
            ["old", "lib", "extra", "main"],
            "fapi is already taken"
        );
        assert!(plan.requires_confirmation() && plan.can_install());
        let alone = PlanDto { main: Some(item("main", Action::Install)), ..PlanDto::default() };
        assert!(!alone.requires_confirmation());
        let blocked = PlanDto {
            blocking: vec![PlanIssue {
                code: "x".into(),
                name: None,
                file_name: None,
                url: None,
                blocking: true,
                held: None,
            }],
            ..alone.clone()
        };
        assert!(blocked.requires_confirmation() && !blocked.can_install());
        assert!(PlanDto::default().changes(&[]).is_empty());
    }

    #[test]
    fn plan_lines_read_like_the_original() {
        assert_eq!(
            item_text(&item("lib", Action::Install)),
            (
                "modrinth_dependency_install_line",
                vec![("name", "LIB".to_string()), ("version", "2.0".to_string())]
            )
        );
        assert_eq!(
            item_text(&item("old", Action::Replace)),
            (
                "modrinth_dependency_replace_line",
                vec![("name", "OLD".to_string()), ("current", "1.0".to_string()), ("new", "2.0".to_string())]
            )
        );
        let issue = |code: &str, name: Option<&str>, file: Option<&str>| PlanIssue {
            code: code.into(),
            name: name.map(str::to_string),
            file_name: file.map(str::to_string),
            url: None,
            blocking: true,
            held: None,
        };
        assert_eq!(
            issue_text(&issue("required_file_only", None, Some("a.jar")), "?"),
            (Some("modrinth_dependency_file_only_issue"), vec![("file", "a.jar".to_string())])
        );
        assert_eq!(
            issue_text(&issue("embedded_dependency", Some("Indium"), None), "?"),
            (None, vec![("name", "Indium".to_string())])
        );
        assert_eq!(
            issue_text(&issue("dependency_version_conflict", Some("Lib"), None), "?").0,
            Some("modrinth_dependency_version_conflict")
        );
        assert_eq!(
            issue_text(&issue("file_blocked", Some("Skin Layers"), None), "?"),
            (Some("dependency_file_blocked_issue"), vec![("name", "Skin Layers".to_string())])
        );
        assert_eq!(
            issue_text(&issue("anything_else", None, None), "?"),
            (Some("modrinth_dependency_resolution_failed"), vec![("name", "?".to_string())])
        );
    }

    #[test]
    fn install_answers_say_which_they_are() {
        let done = InstallAnswer::Installed(InstallOutcome {
            project_id: "p".into(),
            filename: "p.jar".into(),
            version_number: "1".into(),
        });
        assert_eq!(serde_json::to_value(&done).unwrap()["status"], "installed");
        let again = InstallAnswer::Replanned(Box::default());
        assert_eq!(serde_json::to_value(&again).unwrap()["status"], "replanned");
    }
}
