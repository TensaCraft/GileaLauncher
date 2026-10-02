//! Releases: the version and tag, the editions published, notes from the commits, and the
//! packaged files named the way the launcher's updater looks for them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use launcher_shared::{ReleaseOs, release_file_names};

use crate::cmd::{cargo, root, run};
use crate::hooks::parse_subject;
use crate::profile::load_profile;

/// The launcher's version: `[workspace.package] version` of `Cargo.toml` in `root`.
pub fn workspace_version(root: &Path) -> Result<String> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let manifest: toml::Value = toml::from_str(&text).with_context(|| format!("in {}", path.display()))?;
    manifest
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("{} has no [workspace.package] version", path.display()))
}

/// The tag of version `version`.
pub fn tag_for(version: &str) -> String {
    format!("v{version}")
}

/// Refuses tag `tag` unless it is version `version`'s.
pub fn check_tag(tag: &str, version: &str) -> Result<()> {
    if tag != tag_for(version) {
        bail!(
            "tag '{tag}' is not the launcher's version {version} (its tag is {}); set [workspace.package] version in Cargo.toml",
            tag_for(version)
        );
    }
    Ok(())
}

/// Refuses to release tag `tag` from commit `head` when the tag already names another commit
/// (`tag_commit`): a published release is never rebuilt from other code.
pub fn check_new_tag(tag: &str, tag_commit: Option<&str>, head: &str) -> Result<()> {
    match tag_commit {
        Some(commit) if commit != head => bail!(
            "tag {tag} is already released from commit {commit}; raise [workspace.package] version in Cargo.toml for a new release"
        ),
        _ => Ok(()),
    }
}

/// A version with a suffix (`0.3.0-beta.1`) is a pre-release.
pub fn is_prerelease(version: &str) -> bool {
    version.contains('-')
}

/// An edition the release has files for, and the profile that builds it.
#[derive(Debug, PartialEq, Eq)]
pub struct Edition {
    pub profile: String,
    pub edition: String,
}

/// The editions published: the profiles in `root` that update from GitHub, one per edition.
pub fn release_editions(root: &Path) -> Result<Vec<Edition>> {
    let dir = root.join("build-profiles");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(".toml").map(str::to_string))
        .collect();
    names.sort();
    let mut editions: Vec<Edition> = Vec::new();
    for name in names {
        let spec = load_profile(root, &name)?;
        if spec.update_repo().is_empty() || !spec.update_api().is_empty() {
            continue;
        }
        if let Some(twin) = editions.iter().find(|e| e.edition == spec.edition()) {
            bail!("profiles '{}' and '{name}' both publish edition '{}'", twin.profile, spec.edition());
        }
        editions.push(Edition { profile: name, edition: spec.edition().to_string() });
    }
    Ok(editions)
}

/// Release notes of tag `tag` from commit subjects `subjects` (newest first, as `git log` lists
/// them), with the changes since `previous`.
pub fn release_notes(tag: &str, previous: Option<&str>, subjects: &[String]) -> String {
    let mut sections: Vec<(&str, Vec<String>)> = SECTIONS.iter().map(|s| (*s, Vec::new())).collect();
    for subject in subjects.iter().rev() {
        if let Some((section, text)) = note(subject) {
            let entries = &mut sections.iter_mut().find(|(s, _)| *s == section).expect("a known section").1;
            if !entries.contains(&text) {
                entries.push(text);
            }
        }
    }
    let mut out = format!("# {tag}\n\n");
    if let Some(previous) = previous {
        out += &format!("Changes since `{previous}`.\n\n");
    }
    let mut wrote = false;
    for (section, entries) in sections.iter().filter(|(_, e)| !e.is_empty()) {
        wrote = true;
        out += &format!("## {section}\n\n");
        out += &entries.iter().map(|e| format!("- {e}\n")).collect::<String>();
        out += "\n";
    }
    if !wrote {
        out += if previous.is_some() {
            "## Changes\n\n- Maintenance release.\n"
        } else {
            "## Changes\n\n- First release.\n"
        };
    }
    out.trim_end().to_string() + "\n"
}

/// Note sections, in order; commits of other kinds that players notice go to the last.
const SECTIONS: &[&str] = &["New Features", "Fixes", "Interface", "Performance", "Other Changes"];
/// Commit kinds players do not notice.
const UNNOTED: &[&str] = &["build", "chore", "ci", "docs", "refactor", "release", "test"];
/// Shortest summary worth a note.
const NOTE_MIN_LEN: usize = 10;

/// The section and text of commit subject `subject`'s note; none for what players do not notice.
fn note(subject: &str) -> Option<(&'static str, String)> {
    let subject = subject.trim();
    if ["Merge ", "Revert ", "fixup! ", "squash! "].iter().any(|p| subject.starts_with(p)) {
        return None;
    }
    let Some(parsed) = parse_subject(subject) else {
        return worth_noting(subject).then(|| ("Other Changes", sentence(subject)));
    };
    if UNNOTED.contains(&parsed.kind) || !worth_noting(parsed.summary) {
        return None;
    }
    let section = match parsed.kind {
        "feat" => "New Features",
        "fix" => "Fixes",
        "ui" => "Interface",
        "perf" => "Performance",
        _ => "Other Changes",
    };
    let text = match parsed.scope {
        Some(scope) => format!("**{}:** {}", title(scope), sentence(parsed.summary)),
        None => sentence(parsed.summary),
    };
    Some((section, text))
}

fn worth_noting(summary: &str) -> bool {
    let summary = summary.trim().trim_end_matches('.').to_lowercase();
    summary.chars().count() >= NOTE_MIN_LEN
        && !["bump version", "release v"].iter().any(|p| summary.starts_with(p))
}

/// `text` with a capital first letter and no closing full stop.
fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches('.');
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Scope `scope` as words: `home-cards` becomes `Home Cards`.
fn title(scope: &str) -> String {
    scope.split(['-', '_', '/', '.']).filter(|w| !w.is_empty()).map(sentence).collect::<Vec<_>>().join(" ")
}

/// The tag the notes of `tag` start from: the newest of `tags` (newest first) other than `tag`,
/// and for a stable `tag` the newest stable one.
pub fn previous_tag<'a>(tag: &str, tags: &'a [String]) -> Option<&'a str> {
    let stable = !tag.contains('-');
    tags.iter().map(String::as_str).find(|t| *t != tag && (!stable || !t.contains('-')))
}

/// The files `package` makes for edition `edition` of app `app` on `os` and `arch`.
pub fn packaged_names(app: &str, edition: &str, os: ReleaseOs, arch: &str) -> Vec<String> {
    let update_file = |appimage| release_file_names(app, edition, os, arch, appimage);
    match os {
        // The file without an architecture, and the installer for a first install.
        ReleaseOs::Windows => {
            vec![update_file(false).pop().unwrap_or_default(), format!("{app}-{edition}-Setup.exe")]
        }
        ReleaseOs::Linux => vec![update_file(true).remove(0), update_file(false).remove(0)],
        // One universal disk image for both architectures.
        ReleaseOs::MacOs => vec![update_file(false).pop().unwrap_or_default()],
    }
}

/// What `release-meta` is asked for.
pub struct MetaQuery<'a> {
    /// `version`, `tag`, `title`, `prerelease` or `json` (all of them).
    pub field: Option<&'a str>,
    /// The editions published, as a JSON list.
    pub editions: bool,
    /// Fail unless this tag is the version's.
    pub check_tag: Option<&'a str>,
    /// Fail when this tag already names a commit other than the current one.
    pub check_new_tag: Option<&'a str>,
}

/// `release-meta`: the release's version, tag, title and pre-release flag, the editions published,
/// or one of the checks.
pub fn meta(query: MetaQuery<'_>) -> Result<()> {
    let root = root();
    let version = workspace_version(&root)?;
    if let Some(tag) = query.check_tag {
        check_tag(tag, &version)?;
        println!("{tag} is version {version}");
        return Ok(());
    }
    if let Some(tag) = query.check_new_tag {
        let tag_commit =
            git(&["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}^{{commit}}")]).ok();
        check_new_tag(tag, tag_commit.as_deref(), &git(&["rev-parse", "HEAD"])?)?;
        println!("{tag} is free for this commit");
        return Ok(());
    }
    let editions = query.editions;
    if editions {
        let list: Vec<_> = release_editions(&root)?
            .into_iter()
            .map(|e| serde_json::json!({ "profile": e.profile, "edition": e.edition }))
            .collect();
        println!("{}", serde_json::Value::Array(list));
        return Ok(());
    }
    let title = format!("{} {version}", load_profile(&root, "standard")?.app_name());
    let (tag, prerelease) = (tag_for(&version), is_prerelease(&version));
    match query.field.unwrap_or("json") {
        "version" => println!("{version}"),
        "tag" => println!("{tag}"),
        "title" => println!("{title}"),
        "prerelease" => println!("{prerelease}"),
        "json" => println!(
            "{}",
            serde_json::json!({ "version": version, "tag": tag, "title": title, "prerelease": prerelease })
        ),
        other => bail!("unknown field '{other}' (version, tag, title, prerelease or json)"),
    }
    Ok(())
}

/// Output of `git args` in the repository.
fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(root()).output().context("failed to start git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `release-notes`: notes of tag `tag` (its commit, or the current one while it does not exist)
/// since `previous` (by default the tag before it), printed or written to `output`.
pub fn notes(tag: &str, previous: Option<&str>, output: Option<&Path>) -> Result<()> {
    let end = if git(&["rev-parse", "--verify", "--quiet", &format!("{tag}^{{commit}}")]).is_ok() {
        tag
    } else {
        "HEAD"
    };
    let tags: Vec<String> = git(&[
        "-c",
        "versionsort.suffix=-",
        "tag",
        "--merged",
        end,
        "--list",
        "v[0-9]*",
        "--sort=-v:refname",
    ])?
    .lines()
    .map(str::to_string)
    .collect();
    let previous = previous.or_else(|| previous_tag(tag, &tags));
    let range = previous.map_or(end.to_string(), |p| format!("{p}..{end}"));
    let subjects: Vec<String> = git(&["log", "--format=%s", &range])?.lines().map(str::to_string).collect();
    let text = release_notes(tag, previous, &subjects);
    match output {
        Some(path) => fs::write(path, &text).with_context(|| format!("cannot write {}", path.display()))?,
        None => print!("{text}"),
    }
    Ok(())
}

/// The system this runs on, as release files name it.
fn host_os() -> Result<ReleaseOs> {
    if cfg!(windows) {
        Ok(ReleaseOs::Windows)
    } else if cfg!(target_os = "linux") {
        Ok(ReleaseOs::Linux)
    } else if cfg!(target_os = "macos") {
        Ok(ReleaseOs::MacOs)
    } else {
        bail!("releases are made on Windows, Linux and macOS")
    }
}

/// The only file in `dir` whose name ends with `suffix`.
fn only_file(dir: &Path, suffix: &str) -> Result<PathBuf> {
    let found: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.to_string_lossy().ends_with(suffix))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => bail!("expected one *{suffix} in {}, found {}", dir.display(), found.len()),
    }
}

/// Cargo's output folder for the workspace at `root`, as cargo itself resolves it (with
/// `CARGO_TARGET_DIR` set to `target_dir` when given, removed when not).
fn target_dir_with(root: &Path, target_dir: Option<&Path>) -> Result<PathBuf> {
    let mut metadata = Command::new(cargo());
    metadata.current_dir(root).args(["metadata", "--format-version", "1", "--no-deps", "--offline"]);
    match target_dir {
        Some(dir) => metadata.env("CARGO_TARGET_DIR", dir),
        None => metadata.env_remove("CARGO_TARGET_DIR"),
    };
    let out = metadata.output().context("failed to start cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).context("cargo metadata's answer")?;
    json["target_directory"].as_str().map(PathBuf::from).context("cargo metadata names no target_directory")
}

/// Cargo's output folder for the workspace at `root`, as this environment has it.
fn target_dir(root: &Path) -> Result<PathBuf> {
    target_dir_with(root, std::env::var_os("CARGO_TARGET_DIR").as_deref().map(Path::new))
}

/// The app's executable in cargo's output folder `dir`: tauri names it after the app.
fn executable(dir: &Path, app: &str) -> Result<PathBuf> {
    let name = if cfg!(windows) { format!("{app}.exe") } else { app.to_string() };
    let path = dir.join(&name);
    if !path.is_file() {
        bail!("the build finished but {} is missing", path.display());
    }
    Ok(path)
}

/// `package`: builds profile `profile` for release and puts its files, named the way the updater
/// looks for them, in `dist/release`.
pub fn package(profile: &str) -> Result<()> {
    let root = root();
    let spec = load_profile(&root, profile)?;
    crate::cmd::refuse_unsafe_release(&spec, true)?;
    let (app, edition, os, arch) = (spec.app_name(), spec.edition(), host_os()?, std::env::consts::ARCH);
    run(&mut crate::cmd::trunk(&spec, false, true))?;

    // macOS: one disk image for both architectures.
    let target = (os == ReleaseOs::MacOs).then_some("universal-apple-darwin");
    let cargo_out = target_dir(&root)?;
    let out_dir = target.map_or(cargo_out.clone(), |t| cargo_out.join(t)).join("release");
    let bundle_dir = out_dir.join("bundle");
    if bundle_dir.exists() {
        fs::remove_dir_all(&bundle_dir).with_context(|| format!("cannot clear {}", bundle_dir.display()))?;
    }
    let bundle = match os {
        ReleaseOs::Windows => "nsis",
        ReleaseOs::Linux => "appimage",
        ReleaseOs::MacOs => "dmg",
    };
    let mut features = vec!["custom-protocol".to_string()];
    features.extend(spec.app_features());
    let config =
        serde_json::json!({ "productName": app, "mainBinaryName": app, "identifier": spec.identifier() });
    let mut build = Command::new(cargo());
    build
        .current_dir(root.join("crates").join("launcher-app"))
        .args(["tauri", "build", "--ci", "--bundles", bundle, "--features", &features.join(",")])
        .args(["--config", &config.to_string()]);
    if let Some(target) = target {
        build.args(["--target", target]);
    }
    build.args(["--", "--no-default-features"]);
    crate::cmd::clean_update_env(&mut build);
    build.envs(spec.env.iter().map(|(k, v)| (k, v)));
    run(&mut build)?;

    let names = packaged_names(app, edition, os, arch);
    let made = match os {
        ReleaseOs::Windows => vec![executable(&out_dir, app)?, only_file(&bundle_dir.join("nsis"), ".exe")?],
        ReleaseOs::Linux => {
            vec![only_file(&bundle_dir.join("appimage"), ".AppImage")?, executable(&out_dir, app)?]
        }
        ReleaseOs::MacOs => vec![only_file(&bundle_dir.join("dmg"), ".dmg")?],
    };
    let dist = root.join("dist").join("release");
    fs::create_dir_all(&dist).with_context(|| format!("cannot create {}", dist.display()))?;
    for (from, name) in made.iter().zip(&names) {
        let to = dist.join(name);
        fs::copy(from, &to).with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
        println!("{}", to.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_must_match_the_version() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.3.1-beta.2\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let version = workspace_version(dir.path()).unwrap();
        assert_eq!(version, "0.3.1-beta.2");
        assert_eq!(tag_for(&version), "v0.3.1-beta.2");
        check_tag("v0.3.1-beta.2", &version).unwrap();
        for wrong in ["0.3.1-beta.2", "v0.3.1", "v0.3.1-beta.3", ""] {
            assert!(check_tag(wrong, &version).is_err(), "{wrong:?} must be refused");
        }
    }

    #[test]
    fn a_published_tag_is_never_rebuilt_from_other_code() {
        check_new_tag("v0.2.0", None, "abc123").unwrap();
        check_new_tag("v0.2.0", Some("abc123"), "abc123").unwrap();
        let err = check_new_tag("v0.2.0", Some("def456"), "abc123").unwrap_err().to_string();
        assert!(err.contains("v0.2.0") && err.contains("version"), "{err}");
    }

    #[test]
    fn a_version_with_a_suffix_is_a_prerelease() {
        assert!(is_prerelease("0.3.0-beta.1"));
        assert!(is_prerelease("1.0.0-rc.2"));
        assert!(!is_prerelease("0.3.0"));
    }

    #[test]
    fn release_editions_come_from_the_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("build-profiles");
        fs::create_dir(&profiles).unwrap();
        let profile = |name: &str, edition: &str, repo: &str| {
            format!(
                "[profile]\nname = \"{name}\"\nmodules = []\n\n[branding]\napp_name = \"App\"\nidentifier = \"org.app\"\n\
                 edition = \"{edition}\"\nsupport_url = \"\"\nupdate_repo = \"{repo}\"\nms_client_id = \"x\"\n"
            )
        };
        fs::write(profiles.join("standard.toml"), profile("standard", "standard", "owner/app")).unwrap();
        fs::write(profiles.join("full.toml"), profile("full", "extra", "owner/app")).unwrap();
        fs::write(profiles.join("core.toml"), profile("core", "core", "")).unwrap();
        let local = profile("mock", "standard", "mock/app")
            + "update_api = \"http://127.0.0.1:1430\"
";
        fs::write(profiles.join("mock.toml"), local).unwrap();
        let editions = release_editions(dir.path()).unwrap();
        assert_eq!(
            editions,
            [
                Edition { profile: "full".into(), edition: "extra".into() },
                Edition { profile: "standard".into(), edition: "standard".into() },
            ],
            "neither the profiles without a repository nor the ones updating from this machine are released"
        );
        fs::write(profiles.join("twin.toml"), profile("twin", "standard", "owner/app")).unwrap();
        assert!(release_editions(dir.path()).is_err(), "two profiles cannot publish one edition's files");
    }

    #[test]
    fn release_notes_group_commits_by_kind() {
        let subjects: Vec<String> = [
            "fix(updater): each edition updates from its own files",
            "docs: players read how to install the launcher",
            "Merge branch 'feat/x'",
            "ui(home-cards): cards keep one height in every row",
            "feat: builds can be copied with their worlds",
            "fix: update",
            "perf(downloads): parallel downloads reuse connections",
            "a plain subject that follows no convention",
            "feat: builds can be copied with their worlds",
            "release: bump version to 0.2.0",
        ]
        .map(String::from)
        .to_vec();
        let notes = release_notes("v0.2.0", Some("v0.1.0"), &subjects);
        assert_eq!(
            notes,
            "# v0.2.0\n\nChanges since `v0.1.0`.\n\n\
             ## New Features\n\n- Builds can be copied with their worlds\n\n\
             ## Fixes\n\n- **Updater:** Each edition updates from its own files\n\n\
             ## Interface\n\n- **Home Cards:** Cards keep one height in every row\n\n\
             ## Performance\n\n- **Downloads:** Parallel downloads reuse connections\n\n\
             ## Other Changes\n\n- A plain subject that follows no convention\n"
        );
        let quiet = ["docs: players read how to install it".to_string()];
        assert_eq!(
            release_notes("v0.2.1", Some("v0.2.0"), &quiet),
            "# v0.2.1\n\nChanges since `v0.2.0`.\n\n## Changes\n\n- Maintenance release.\n"
        );
        assert_eq!(
            release_notes("v0.1.0", None, &["chore: first public release of the app".to_string()]),
            "# v0.1.0\n\n## Changes\n\n- First release.\n",
            "the first release has no earlier one to maintain"
        );

        let tags: Vec<String> =
            ["v0.3.0", "v0.3.0-beta.2", "v0.3.0-beta.1", "v0.2.0"].map(String::from).to_vec();
        assert_eq!(previous_tag("v0.3.0", &tags), Some("v0.2.0"), "a stable release covers its betas");
        assert_eq!(previous_tag("v0.3.0-beta.2", &tags[1..]), Some("v0.3.0-beta.1"));
        assert_eq!(previous_tag("v0.1.0", &[]), None);
    }

    #[test]
    fn packages_come_from_cargos_own_target_folder() {
        let root = root();
        let same = |a: &Path, b: &Path| fs::canonicalize(a).unwrap() == fs::canonicalize(b).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let found = target_dir_with(&root, Some(elsewhere.path())).unwrap();
        assert!(same(&found, elsewhere.path()), "CARGO_TARGET_DIR is followed: {}", found.display());
        let found = target_dir_with(&root, None).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        assert!(same(&found, &root.join("target")), "the workspace's own: {}", found.display());
    }

    #[test]
    fn packaged_names_are_the_updaters_names() {
        // What `package` makes, and which of the updater's platforms each update file serves.
        let table = [
            (ReleaseOs::Windows, vec!["App-extra.exe", "App-extra-Setup.exe"]),
            (ReleaseOs::Linux, vec!["App-extra-x86_64.AppImage", "App-extra-x86_64"]),
            (ReleaseOs::MacOs, vec!["App-extra-universal.dmg"]),
        ];
        for (os, names) in &table {
            assert_eq!(packaged_names("App", "extra", *os, "x86_64"), *names, "{os:?}");
        }
        let platforms = [
            (ReleaseOs::Windows, "x86_64", false),
            (ReleaseOs::Linux, "x86_64", true),
            (ReleaseOs::Linux, "x86_64", false),
            (ReleaseOs::MacOs, "aarch64", false),
            (ReleaseOs::MacOs, "x86_64", false),
        ];
        for (os, arch, appimage) in platforms {
            let packaged = &table.iter().find(|(o, _)| *o == os).unwrap().1;
            let wanted = release_file_names("App", "extra", os, arch, appimage);
            assert!(
                wanted.iter().any(|name| packaged.contains(&name.as_str())),
                "{os:?} {arch} appimage={appimage} looks for {wanted:?}, the release has {packaged:?}"
            );
        }
    }
}
