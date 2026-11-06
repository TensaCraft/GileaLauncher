//! Horizon's registry (github.com/horizon-rust/horizon-mods) as a content provider: its shader
//! packs and resource packs made for the Horizon here, searched, planned and downloaded into a
//! Horizon build's game folder, and known there by their SHA-256. The player picks a shader pack
//! in the game.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_core::core_app::CoreApp;
use launcher_core::feedback::OperationSpec;
use launcher_core::launch::engine::engine_of;
use launcher_core::launch::options::game_dir;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, ExpectedHash, HashKind, hash_file};
use launcher_core::providers::{ContentProvider, ProviderFuture};
use launcher_core::storage::versions::Build;
use launcher_shared::provider::{
    Action, Change, FileNote, InstallAnswer, InstallArgs, InstallOutcome, Overview, OverviewArgs, PlanDto,
    PlanItem, ProjectHit, SearchArgs, SearchPage, installed_key, installing_key,
};
use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode, Text};
use serde::Deserialize;
use serde_json::Value;

/// The index the registry's CI builds (schema 1, `docs/api/mod-index.md` in Horizon's repository).
pub const INDEX: &str = "https://horizon-rust.github.io/horizon-mods/index.json";
/// The newest index schema read; a newer one may mean something else.
const SCHEMA: u64 = 1;
/// The shader API and schema a stock nightly loads (it refuses API 0.2): what a Horizon that has
/// not said otherwise is taken to load.
const SHADER_API: &str = "0.1";
const SHADER_SCHEMA: u64 = 1;
/// The largest file the registry lists.
const MOST_BYTES: u64 = 64 * 1024 * 1024;
/// The registry's own repository, whose releases hold its files.
const REGISTRY_REPO: &str = "horizon-rust/horizon-mods";

/// The shader packs a Horizon loads: their shader APIs and the newest schema it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shaders {
    pub api: Vec<String>,
    pub schema: u64,
}

/// What a client's `--launch-api` reply says it loads (`content.shader`), unknown fields aside;
/// without a reply, or one that tells nothing, a stock nightly's.
pub fn accepted(reply: Option<&str>) -> Shaders {
    let reply: Value = reply.and_then(|r| serde_json::from_str(r).ok()).unwrap_or_default();
    let shader = &reply["content"]["shader"];
    Shaders {
        api: match shader["api"].as_array() {
            Some(api) => api.iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
            None => vec![SHADER_API.into()],
        },
        schema: shader["schema"].as_u64().unwrap_or(SHADER_SCHEMA),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    /// Newest first.
    #[serde(default)]
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    pub version: String,
    /// A shader pack's shader API (a resource pack's pack format).
    #[serde(default)]
    pub api: String,
    /// A shader pack's schema, when it names one.
    #[serde(default)]
    pub schema: Option<u64>,
    /// The Minecraft version it is for.
    #[serde(default)]
    pub game: String,
    pub file: RegistryFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryFile {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// A file taken down: one by its SHA-256, or every version of `id` without one.
#[derive(Debug, Clone, Deserialize)]
struct Revoked {
    id: String,
    #[serde(default)]
    sha256: Option<String>,
}

/// A project and the version of it a Horizon build takes.
#[derive(Debug, Clone)]
pub struct Offer {
    pub project: Project,
    pub version: Version,
}

/// The registry's kinds a tab of `kind` lists, each with its files' ending.
fn kinds(kind: ContentKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        ContentKind::ShaderPacks => &[("shader", "hshader"), ("shaderpack", "zip")],
        ContentKind::ResourcePacks => &[("resourcepack", "zip")],
        ContentKind::Mods => &[],
    }
}

/// The index's projects of `kind`'s kinds, every version; an entry this reader cannot read is
/// left out rather than the whole index.
pub fn projects(index: &Value, kind: ContentKind) -> Vec<Project> {
    index["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| serde_json::from_value::<Project>(p.clone()).ok())
        .filter(|p| kinds(kind).iter().any(|(k, _)| *k == p.kind))
        .collect()
}

/// `https://github.com/<owner>/<repo>/releases/download/<tag>/<name>` of the registry's own
/// repository or of the project's (its `source`): the only files the launcher fetches.
fn release_url(url: &str, source: Option<&str>) -> bool {
    let Some(path) = url.strip_prefix("https://github.com/") else { return false };
    let own = source.and_then(|s| s.strip_prefix("https://github.com/")).map(|s| s.trim_end_matches('/'));
    let named =
        |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    // A dot segment, written plainly or as `%2e`, would climb out of the release.
    let plain = |part: &str| !part.to_ascii_lowercase().replace("%2e", ".").trim_matches('.').is_empty();
    let known = |owner: &str, repo: &str| {
        let repo = format!("{owner}/{repo}");
        [Some(REGISTRY_REPO), own].into_iter().flatten().any(|r| r.eq_ignore_ascii_case(&repo))
    };
    let parts: Vec<&str> = path.split('/').collect();
    !path.contains(['?', '#', '\\'])
        && matches!(parts.as_slice(), [owner, repo, "releases", "download", tag, name]
            if named(owner) && named(repo) && known(owner, repo) && plain(tag) && plain(name))
}

/// A file the launcher may fetch: a plain name with `ending`, at most 64 MiB (the download is
/// held to its exact size), its lowercase hex SHA-256, from a release of the registry or of the
/// project (`source`). The project's minisign `key` and the file's `signature` are not checked:
/// they come in the same index as the SHA-256 the download is checked by, so they vouch for
/// nothing more.
fn fetchable(file: &RegistryFile, kind: ContentKind, ending: &str, source: Option<&str>) -> bool {
    (1..=MOST_BYTES).contains(&file.size)
        && file.sha256.len() == 64
        && file.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && file
            .name
            .to_ascii_lowercase()
            .strip_suffix(ending)
            .is_some_and(|stem| stem.len() > 1 && stem.ends_with('.'))
        && launcher_core::content::files::destination(kind, &file.name).is_ok()
        && release_url(&file.url, source)
}

/// What the registry offers a Horizon build's tab of `kind`, in the index's order: each project
/// with its newest version the game takes. Revoked files, files from elsewhere and shader packs
/// the Horizon here does not load (`shaders`) are left out.
pub fn offers(index: &Value, kind: ContentKind, shaders: &Shaders) -> AppResult<Vec<Offer>> {
    if index["schema"].as_u64().unwrap_or(SCHEMA) > SCHEMA {
        return Err(AppError::new(ErrorCode::Unsupported, "Horizon's registry has a newer index"));
    }
    let revoked: Vec<Revoked> = index["revoked"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect();
    let takes = |project: &Project, version: &Version| {
        let ending = kinds(kind).iter().find(|(k, _)| *k == project.kind).map_or("", |(_, e)| *e);
        let loads = shaders.api.contains(&version.api) && version.schema.is_none_or(|s| s <= shaders.schema);
        version.game == crate::MINECRAFT
            && (project.kind != "shader" || loads)
            && fetchable(&version.file, kind, ending, project.source.as_deref())
            && !revoked.iter().any(|r| match &r.sha256 {
                Some(sha256) => sha256.eq_ignore_ascii_case(&version.file.sha256),
                None => r.id == project.id,
            })
    };
    Ok(projects(index, kind)
        .into_iter()
        .filter_map(|project| {
            let version = project.versions.iter().find(|v| takes(&project, v))?.clone();
            Some(Offer { project, version })
        })
        .collect())
}

fn https(url: &Option<String>) -> Option<String> {
    url.clone().filter(|u| u.starts_with("https://"))
}

/// A page of `offers` whose name, id or summary holds the search.
pub fn page(offers: &[Offer], args: &SearchArgs) -> SearchPage {
    let query = args.query.trim().to_lowercase();
    let found: Vec<&Offer> = offers
        .iter()
        .filter(|o| {
            query.is_empty()
                || [&o.project.name, &o.project.id, &o.project.summary]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&query))
        })
        .collect();
    let hits = found
        .iter()
        .skip(args.offset as usize)
        .take(args.page_size() as usize)
        .map(|o| ProjectHit {
            project_id: o.project.id.clone(),
            slug: o.project.id.clone(),
            title: o.project.name.clone(),
            author: o.project.authors.join(", "),
            description: o.project.summary.clone(),
            downloads: o.project.downloads,
            icon_url: https(&o.project.icon),
            url: https(&o.project.source),
        })
        .collect();
    SearchPage { hits, total: found.len() as u32, offset: args.offset, limit: args.page_size() }
}

/// Where `offer` goes in a build's game folder `game`: its kind's folder, under its own name.
fn destination(game: &Path, kind: ContentKind, offer: &Offer) -> AppResult<PathBuf> {
    Ok(game.join(launcher_core::content::files::destination(kind, &offer.version.file.name)?))
}

/// What installing at `dest` does: nothing when the file there is the one wanted (`sha256`),
/// replaces another file of its name (the player's own), else installs.
fn action_of(dest: &Path, sha256: &str) -> Action {
    if !dest.is_file() {
        return Action::Install;
    }
    match hash_file(dest, HashKind::Sha256) {
        Ok(found) if found.eq_ignore_ascii_case(sha256) => Action::Satisfied,
        _ => Action::Replace,
    }
}

/// The plan of installing `offer`: the pack alone, which the player confirms first when it takes
/// the place of a file not the registry's.
fn plan_of(offer: &Offer, action: Action) -> PlanDto {
    let file = &offer.version.file;
    let main = PlanItem {
        project_id: offer.project.id.clone(),
        version_id: file.sha256.clone(),
        title: offer.project.name.clone(),
        version_number: offer.version.version.clone(),
        filename: file.name.clone(),
        action,
        current: (action == Action::Replace).then(|| file.name.clone()),
        url: https(&offer.project.source),
        unrecognized: action == Action::Replace,
    };
    PlanDto {
        main: Some(main),
        install: Vec::new(),
        replace: Vec::new(),
        satisfied: Vec::new(),
        optional: Vec::new(),
        optional_issues: Vec::new(),
        embedded: Vec::new(),
        blocking: Vec::new(),
    }
}

/// The files in `dir` that are one of `projects`' versions, known by size first and SHA-256 then.
fn notes(dir: &Path, projects: &[Project]) -> Vec<FileNote> {
    let mut files: Vec<(String, PathBuf, u64)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(fs::Metadata::is_file)?;
            Some((e.file_name().to_string_lossy().into_owned(), e.path(), meta.len()))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|(file, path, size)| {
            let sized = || projects.iter().flat_map(|p| p.versions.iter().map(move |v| (p, v)));
            if !sized().any(|(_, v)| v.file.size == size) {
                return None;
            }
            let sha256 = hash_file(&path, HashKind::Sha256).ok()?;
            let (project, version) =
                sized().find(|(_, v)| v.file.size == size && v.file.sha256.eq_ignore_ascii_case(&sha256))?;
            Some(FileNote {
                file,
                project_id: project.id.clone(),
                slug: project.id.clone(),
                title: project.name.clone(),
                version_number: version.version.clone(),
                update: None,
                url: https(&project.source),
                icon_url: https(&project.icon),
                // The registry's own file, however it came.
                installed: true,
            })
        })
        .collect()
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))
}

pub struct Registry {
    core: Arc<CoreApp>,
}

impl Registry {
    pub fn new(core: Arc<CoreApp>) -> Registry {
        Registry { core }
    }

    async fn index(&self) -> AppResult<Value> {
        self.core.meta.get_json(INDEX).await
    }

    /// The registry's offers of `kind` for the Horizon here: the shader packs its client's kept
    /// `--launch-api` reply says it loads (none yet before its first start: a stock nightly's).
    async fn offers(&self, kind: ContentKind) -> AppResult<Vec<Offer>> {
        let minecraft = self.core.paths.minecraft_dir.clone();
        let reply = blocking(move || {
            let client = super::install::present(&minecraft)?.client;
            fs::read_to_string(super::engine::kept_reply(&client)).ok()
        })
        .await?;
        offers(&self.index().await?, kind, &accepted(reply.as_deref()))
    }

    /// Build `key` and its game folder, when Horizon runs it.
    fn horizon_build(&self, key: &str) -> AppResult<Option<(Build, PathBuf)>> {
        let build = self.core.versions.get(key).ok_or_else(|| {
            AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
        })?;
        if engine_of(&build.options) != Some(crate::ENGINE) {
            return Ok(None);
        }
        let game = game_dir(&build, self.core.versions.minecraft_dir());
        Ok(Some((build, game)))
    }

    fn target(&self, key: &str) -> AppResult<(Build, PathBuf)> {
        self.horizon_build(key)?.ok_or_else(|| {
            AppError::new(ErrorCode::InvalidInput, format!("{key} is not a Horizon build"))
                .with_param("version", key)
        })
    }

    async fn offer(&self, kind: ContentKind, project_id: &str) -> AppResult<Offer> {
        self.offers(kind).await?.into_iter().find(|o| o.project.id == project_id).ok_or_else(|| {
            AppError::new(ErrorCode::NoCompatibleVersion, format!("the registry offers no {project_id}"))
                .with_param("name", project_id)
        })
    }

    async fn plan_for(&self, args: &InstallArgs) -> AppResult<(Build, PathBuf, Offer, PlanDto)> {
        let (build, game) = self.target(&args.key)?;
        let offer = self.offer(args.kind, &args.project_id).await?;
        let dest = destination(&game, args.kind, &offer)?;
        let sha256 = offer.version.file.sha256.clone();
        let plan = plan_of(&offer, blocking(move || action_of(&dest, &sha256)).await?);
        Ok((build, game, offer, plan))
    }

    async fn install_into(&self, args: &InstallArgs) -> AppResult<InstallAnswer> {
        let (build, game, offer, plan) = self.plan_for(args).await?;
        let Some(main) = plan.main.clone() else { return Ok(InstallAnswer::Replanned(Box::new(plan))) };
        let outcome = InstallOutcome {
            project_id: main.project_id.clone(),
            filename: main.filename.clone(),
            version_number: main.version_number.clone(),
        };
        if main.action == Action::Satisfied {
            return Ok(InstallAnswer::Installed(outcome));
        }
        let change = Change { project_id: main.project_id, version_id: main.version_id, action: main.action };
        if !args.approved.contains(&change) {
            return Ok(InstallAnswer::Replanned(Box::new(plan)));
        }
        let _lease = self
            .core
            .instances
            .try_acquire(&game, "content")
            .map_err(|e| e.with_param("version", build.name.clone()))?;
        let name = offer.project.name.clone();
        let op = self.core.feedback.begin(
            OperationSpec::new(Text::key(installing_key(args.kind)).param("name", &name), crate::ID)
                .on_build(&build.key),
        );
        let file = &offer.version.file;
        let task = DownloadTask::new(file.url.clone(), destination(&game, args.kind, &offer)?)
            .hash(ExpectedHash::sha256(&file.sha256))
            .size(file.size);
        let progress = |p: DownloadProgress| op.progress(p.bytes_done as f64, p.bytes_total.max(1) as f64);
        match self.core.downloader.download_one(task, &progress).await {
            Ok(()) => {
                op.finish();
                let done = if args.kind == ContentKind::ShaderPacks {
                    "horizon_shader_installed"
                } else {
                    installed_key(args.kind)
                };
                self.core.feedback.success(Text::key(done).param("name", &name));
                Ok(InstallAnswer::Installed(outcome))
            }
            Err(e) => {
                op.fail(Text::key("installation_failed"));
                Err(e)
            }
        }
    }

    async fn overview_of(&self, args: &OverviewArgs) -> AppResult<Overview> {
        let Some((_, game)) = self.horizon_build(&args.key)? else { return Ok(Overview::default()) };
        let projects = projects(&self.index().await?, args.kind);
        let dir = game.join(args.kind.folder());
        Ok(Overview { notes: blocking(move || notes(&dir, &projects)).await?, updates: None })
    }
}

impl ContentProvider for Registry {
    fn search(&self, args: SearchArgs) -> ProviderFuture<'_, SearchPage> {
        Box::pin(async move { Ok(page(&self.offers(args.kind).await?, &args)) })
    }

    fn plan(&self, args: InstallArgs) -> ProviderFuture<'_, PlanDto> {
        Box::pin(async move { Ok(self.plan_for(&args).await?.3) })
    }

    fn install(&self, args: InstallArgs) -> ProviderFuture<'_, InstallAnswer> {
        Box::pin(async move { self.install_into(&args).await })
    }

    fn overview(&self, args: OverviewArgs) -> ProviderFuture<'_, Overview> {
        Box::pin(async move { self.overview_of(&args).await })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use launcher_shared::ContentKind;
    use launcher_shared::provider::{Action, SearchArgs};
    use serde_json::{Value, json};

    use super::*;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn version(version: &str, api: &str, game: &str, name: &str, sha256: &str) -> Value {
        json!({
            "version": version, "published": "2026-11-01", "changelog": "", "api": api, "game": game,
            "file": {
                "name": name,
                "url": format!("https://github.com/horizon-rust/horizon-mods/releases/download/x-{version}/{name}"),
                "size": 4, "sha256": sha256
            }
        })
    }

    fn project(id: &str, kind: &str, versions: Vec<Value>) -> Value {
        json!({
            "id": id, "kind": kind, "name": id.to_uppercase(), "summary": format!("{id} summary"),
            "description": "", "authors": ["Ann", "Bo"], "license": "MIT",
            "source": format!("https://github.com/someone/{id}"), "icon": "https://img/x.png",
            "downloads": 7, "versions": versions, "stars": 3
        })
    }

    fn index(projects: Vec<Value>, revoked: Value) -> Value {
        json!({"schema": 1, "generated": "", "projects": projects, "revoked": revoked})
    }

    fn ids(offers: &[Offer]) -> Vec<(&str, &str)> {
        offers.iter().map(|o| (o.project.id.as_str(), o.version.version.as_str())).collect()
    }

    /// What a stock nightly loads.
    fn stock() -> Shaders {
        accepted(None)
    }

    #[test]
    fn shader_packs_are_offered_as_the_horizon_here_loads_them() {
        assert_eq!(accepted(None), Shaders { api: vec!["0.1".into()], schema: 1 }, "a stock nightly's");
        let reply = r#"{"launch_api":"1.4","options":[],"content":{"mod":{"api":["0.2","0.3"]},
            "shader":{"schema":2,"api":["0.1","0.2"],"later":true},"resourcepack":{"format":[97,1]}}}"#;
        let newer = accepted(Some(reply));
        assert_eq!(newer, Shaders { api: vec!["0.1".into(), "0.2".into()], schema: 2 });
        assert_eq!(accepted(Some(r#"{"launch_api":"1.3","options":[]}"#)), stock(), "no content told");
        assert_eq!(accepted(Some("not json")), stock());
        let mut later_schema = version("0.3.0", "0.2", "26.3", "daybreak-0.3.0.hshader", SHA_B);
        later_schema["schema"] = json!(3);
        let mut schema_two = version("0.2.0", "0.2", "26.3", "daybreak-0.2.0.hshader", SHA_B);
        schema_two["schema"] = json!(2);
        let index = index(
            vec![project(
                "daybreak",
                "shader",
                vec![
                    later_schema,
                    schema_two,
                    version("0.1.1", "0.1", "26.3", "daybreak-0.1.1.hshader", SHA_A),
                ],
            )],
            json!([]),
        );
        assert_eq!(ids(&offers(&index, ContentKind::ShaderPacks, &newer).unwrap()), [("daybreak", "0.2.0")]);
        assert_eq!(
            ids(&offers(&index, ContentKind::ShaderPacks, &stock()).unwrap()),
            [("daybreak", "0.1.1")]
        );
    }

    #[test]
    fn a_tab_lists_its_kinds_newest_version_this_game_takes() {
        let index = index(
            vec![
                project(
                    "daybreak",
                    "shader",
                    vec![
                        version("0.2.0", "0.2", "26.3", "daybreak-0.2.0.hshader", SHA_B),
                        version("0.1.1", "0.1", "26.3", "daybreak-0.1.1.hshader", SHA_A),
                    ],
                ),
                project("old", "shader", vec![version("1.0.0", "0.1", "26.2", "old-1.0.0.hshader", SHA_A)]),
                project("bsl", "shaderpack", vec![version("8.2", "", "26.3", "bsl-8.2.zip", SHA_A)]),
                project(
                    "panorama",
                    "resourcepack",
                    vec![version("1.0.0", "34", "26.3", "panorama.zip", SHA_A)],
                ),
                project("hud", "mod", vec![version("0.1.0", "0.1", "26.3", "hud-0.1.0.hmod", SHA_A)]),
                json!({"id": "broken", "kind": "shader"}),
            ],
            json!([]),
        );
        assert_eq!(
            ids(&offers(&index, ContentKind::ShaderPacks, &stock()).unwrap()),
            [("daybreak", "0.1.1"), ("bsl", "8.2")],
            "the shader API a stock nightly loads, for its game; a broken entry is skipped"
        );
        assert_eq!(
            ids(&offers(&index, ContentKind::ResourcePacks, &stock()).unwrap()),
            [("panorama", "1.0.0")]
        );
        assert!(offers(&index, ContentKind::Mods, &stock()).unwrap().is_empty(), "no Java mods");
        let newer = json!({"schema": 2, "projects": [], "revoked": []});
        assert_eq!(
            offers(&newer, ContentKind::ShaderPacks, &stock()).unwrap_err().code,
            ErrorCode::Unsupported
        );
    }

    #[test]
    fn revoked_files_are_never_offered() {
        let projects = || {
            vec![
                project(
                    "daybreak",
                    "shader",
                    vec![
                        version("0.1.2", "0.1", "26.3", "daybreak-0.1.2.hshader", SHA_B),
                        version("0.1.1", "0.1", "26.3", "daybreak-0.1.1.hshader", SHA_A),
                    ],
                ),
                project("montarc", "shader", vec![version("1.1.1", "0.1", "26.3", "montarc.hshader", SHA_A)]),
            ]
        };
        let one_file =
            index(projects(), json!([{"id": "daybreak", "sha256": SHA_B, "reason": "x", "date": ""}]));
        assert_eq!(
            ids(&offers(&one_file, ContentKind::ShaderPacks, &stock()).unwrap()),
            [("daybreak", "0.1.1"), ("montarc", "1.1.1")],
            "the file before the revoked one"
        );
        let every_version = index(projects(), json!([{"id": "daybreak", "reason": "x", "date": ""}]));
        assert_eq!(
            ids(&offers(&every_version, ContentKind::ShaderPacks, &stock()).unwrap()),
            [("montarc", "1.1.1")]
        );
    }

    #[test]
    fn files_come_only_from_github_releases_and_as_listed() {
        let with_file = |change: &dyn Fn(&mut Value)| {
            let mut v = version("0.1.1", "0.1", "26.3", "daybreak-0.1.1.hshader", SHA_A);
            change(&mut v["file"]);
            let index = index(vec![project("daybreak", "shader", vec![v])], json!([]));
            offers(&index, ContentKind::ShaderPacks, &stock()).unwrap().len()
        };
        assert_eq!(with_file(&|_| {}), 1);
        for own in [
            "https://github.com/someone/daybreak/releases/download/v1/p.hshader",
            "https://github.com/SomeOne/Daybreak/releases/download/v1/p.hshader",
        ] {
            assert_eq!(with_file(&|f| f["url"] = json!(own)), 1, "the project's own repository: {own}");
        }
        for url in [
            "https://github.com/someone/pack/releases/download/v1/p.hshader",
            "https://github.com/horizon-rust/horizon/releases/download/t/p.hshader",
            "http://github.com/horizon-rust/horizon-mods/releases/download/t/p.hshader",
            "https://github.com.evil.example/a/b/releases/download/t/p.hshader",
            "https://evil.example/horizon-rust/horizon-mods/releases/download/t/p.hshader",
            "https://github.com/a/b/raw/t/p.hshader",
            "https://github.com/a/b/releases/download/t/sub/p.hshader",
            "https://github.com/a/b/releases/download/%2e%2e/p.hshader",
            "https://github.com/a/b/releases/download/t/p.hshader?x=1",
            "https://github.com/a@b/c/releases/download/t/p.hshader",
            "https://github.com//b/releases/download/t/p.hshader",
        ] {
            assert_eq!(with_file(&|f| f["url"] = json!(url)), 0, "{url}");
        }
        assert_eq!(with_file(&|f| f["size"] = json!(64 * 1024 * 1024 + 1)), 0, "over 64 MiB");
        assert_eq!(with_file(&|f| f["size"] = json!(0)), 0, "an empty file");
        assert_eq!(with_file(&|f| f["sha256"] = json!(SHA_A.to_uppercase())), 0, "lowercase hex");
        assert_eq!(with_file(&|f| f["sha256"] = json!("abc")), 0);
        assert_eq!(with_file(&|f| f["name"] = json!("../evil.hshader")), 0, "a plain name");
        assert_eq!(with_file(&|f| f["name"] = json!("daybreak.exe")), 0, "the kind's ending");
    }

    #[test]
    fn a_search_finds_by_name_id_or_summary_a_page_at_a_time() {
        let shaders: Vec<Value> = ["daybreak", "montarc", "radiant"]
            .iter()
            .map(|id| {
                project(id, "shader", vec![version("1.0", "0.1", "26.3", &format!("{id}.hshader"), SHA_A)])
            })
            .collect();
        let all = offers(&index(shaders, json!([])), ContentKind::ShaderPacks, &stock()).unwrap();
        let search = |query: &str, offset: u32, limit: u32| SearchArgs {
            key: "h".into(),
            kind: ContentKind::ShaderPacks,
            query: query.into(),
            offset,
            target: None,
            limit: Some(limit),
        };
        let found = page(&all, &search(" MONT ", 0, 10));
        assert_eq!(found.hits.iter().map(|h| h.project_id.as_str()).collect::<Vec<_>>(), ["montarc"]);
        let hit = &found.hits[0];
        assert_eq!(
            (hit.title.as_str(), hit.author.as_str(), hit.description.as_str(), hit.downloads),
            ("MONTARC", "Ann, Bo", "montarc summary", 7)
        );
        assert_eq!(hit.url.as_deref(), Some("https://github.com/someone/montarc"));
        assert_eq!(page(&all, &search("radiant summary", 0, 10)).total, 1);
        let second = page(&all, &search("", 2, 10));
        assert_eq!((second.total, second.hits.len(), second.offset), (3, 1, 2));
    }

    #[test]
    fn a_pack_goes_into_its_kind_s_folder_and_is_asked_about_when_another_has_its_name() {
        let index = index(
            vec![project(
                "daybreak",
                "shader",
                vec![version("0.1.1", "0.1", "26.3", "daybreak.hshader", SHA_A)],
            )],
            json!([]),
        );
        let offer = offers(&index, ContentKind::ShaderPacks, &stock()).unwrap().remove(0);
        let game = tempfile::tempdir().unwrap();
        let dest = destination(game.path(), ContentKind::ShaderPacks, &offer).unwrap();
        assert_eq!(dest, game.path().join("shaderpacks").join("daybreak.hshader"));
        assert_eq!(plan_of(&offer, action_of(&dest, SHA_A)).main.unwrap().action, Action::Install);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"mine").unwrap();
        let sha = launcher_core::net::downloader::hash_file(&dest, HashKind::Sha256).unwrap();
        let main = plan_of(&offer, action_of(&dest, SHA_A)).main.unwrap();
        assert_eq!(
            (main.action, main.unrecognized, main.current.as_deref()),
            (Action::Replace, true, Some("daybreak.hshader")),
            "the player is asked before their file goes"
        );
        assert!(plan_of(&offer, action_of(&dest, SHA_A)).requires_confirmation());
        assert_eq!(plan_of(&offer, action_of(&dest, &sha)).main.unwrap().action, Action::Satisfied);
    }

    #[test]
    fn installed_packs_are_known_by_their_checksum() {
        let game = tempfile::tempdir().unwrap();
        let dir = game.path().join("shaderpacks");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("renamed.hshader"), b"ours").unwrap();
        fs::write(dir.join("other.hshader"), b"same").unwrap();
        fs::write(dir.join("big.hshader"), b"not ours at all").unwrap();
        let ours = launcher_core::net::downloader::hash_file(&dir.join("renamed.hshader"), HashKind::Sha256)
            .unwrap();
        let index = index(
            vec![project(
                "daybreak",
                "shader",
                vec![
                    version("0.2.0", "0.2", "26.3", "daybreak-0.2.0.hshader", SHA_B),
                    version("0.1.1", "0.1", "26.3", "daybreak-0.1.1.hshader", &ours),
                ],
            )],
            json!([]),
        );
        let notes = notes(&dir, &projects(&index, ContentKind::ShaderPacks));
        let seen: Vec<(&str, &str, &str, bool)> = notes
            .iter()
            .map(|n| (n.file.as_str(), n.project_id.as_str(), n.version_number.as_str(), n.installed))
            .collect();
        assert_eq!(seen, [("renamed.hshader", "daybreak", "0.1.1", true)], "an older version too");
        assert_eq!(notes[0].icon_url.as_deref(), Some("https://img/x.png"));
    }
}
