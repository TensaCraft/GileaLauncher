#[cfg(test)]
mod brand;
mod cmd;
#[cfg(test)]
mod downloads;
mod hooks;
mod mock;
mod profile;
mod release;
mod secrets;
mod sweep;
mod templates;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Launcher build tool")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Args)]
struct Target {
    /// Build profile from build-profiles/<name>.toml (default: standard)
    #[arg(long, conflicts_with = "modules")]
    profile: Option<String>,
    /// Comma-separated module list overriding the profile modules; "all" for every module but the postponed ones (diagnostics)
    /// under modules/, "none" (or "") for no modules
    #[arg(long)]
    modules: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the app with hot-reloaded UI
    Dev(Target),
    /// Build frontend and app
    Build {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        release: bool,
    },
    /// Run all tests
    Test,
    /// fmt + clippy (native and wasm) + tests
    Check,
    /// fmt + clippy (native and wasm), without the tests
    Lint,
    /// Generate app icons from a square PNG/SVG
    Icons {
        path: PathBuf,
        /// The macOS variant (the tile on Apple's grid, with room around it) for icon.icns
        #[arg(long)]
        macos: Option<PathBuf>,
    },
    /// Create a new optional module skeleton
    NewModule { id: String },
    /// List build profiles
    Profiles,
    /// Local GitHub Releases imitation for testing launcher updates
    MockReleases {
        #[command(subcommand)]
        action: MockAction,
    },
    /// Check a commit message file against the commit conventions
    CommitMsg { file: PathBuf },
    /// Run a git hook's checks
    Hook {
        #[command(subcommand)]
        hook: Hook,
    },
    /// Make git run the hooks in .githooks
    Hooks,
    /// Fail when the commits of a range (`a..b`) add a key-shaped string
    ScanKeys {
        #[arg(long)]
        range: String,
    },
    /// The release's version, tag and title, or the editions it publishes
    ReleaseMeta {
        /// version, tag, title, prerelease or json (the default)
        #[arg(long)]
        field: Option<String>,
        /// The editions published, as a JSON list of {profile, edition}
        #[arg(long)]
        editions: bool,
        /// Fail unless this tag is the version's (v<version>)
        #[arg(long)]
        check_tag: Option<String>,
        /// Fail when this tag already names another commit (a published release is never rebuilt)
        #[arg(long)]
        check_new_tag: Option<String>,
    },
    /// Release notes from the commits since the previous tag
    ReleaseNotes {
        #[arg(long)]
        tag: String,
        /// The tag the notes start from (default: the one before --tag; for a stable tag the stable one)
        #[arg(long)]
        previous: Option<String>,
        /// Write the notes here instead of printing them
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Build a profile for release and put its files, named for the updater, in dist/release
    Package {
        #[arg(long, default_value = "standard")]
        profile: String,
    },
}

#[derive(Subcommand)]
enum Hook {
    /// fmt, whitespace in the staged changes, clippy (LAUNCHER_SKIP_PRECOMMIT=1 skips them)
    PreCommit,
}

#[derive(Subcommand)]
enum MockAction {
    /// Serve .dev/mock-releases like api.github.com
    Serve {
        #[arg(long, default_value = "normal")]
        scenario: String,
        #[arg(long, default_value_t = mock_github::DEFAULT_PORT)]
        port: u16,
    },
    /// Build a launcher of this version (or take --file) and publish it as a release
    Publish {
        version: String,
        #[arg(long)]
        beta: bool,
        #[arg(long, default_value = "")]
        notes: String,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Remove all mock releases and the demo installation
    Reset,
    /// Build 0.1.0, publish 0.2.0, serve it and start the 0.1.0 copy
    Demo {
        #[arg(long, default_value = "normal")]
        scenario: String,
    },
}

fn resolve(target: &Target) -> Result<profile::BuildSpec> {
    let root = cmd::root();
    let base = profile::load_profile(&root, target.profile.as_deref().unwrap_or("standard"))?;
    match &target.modules {
        Some(list) => profile::with_modules(base, &root, list),
        None => Ok(base),
    }
}

fn main() -> Result<()> {
    let command = Cli::parse().command;
    if matches!(
        command,
        Cmd::Dev(_)
            | Cmd::Build { .. }
            | Cmd::Test
            | Cmd::Check
            | Cmd::Lint
            | Cmd::Package { .. }
            | Cmd::Hook { .. }
    ) {
        sweep::daily(&sweep::target_dir());
    }
    match command {
        Cmd::Dev(target) => cmd::dev(&resolve(&target)?),
        Cmd::Build { target, release } => cmd::build(&resolve(&target)?, release),
        Cmd::Test => cmd::test(),
        Cmd::Check => cmd::check(),
        Cmd::Lint => cmd::lint(),
        Cmd::Icons { path, macos } => cmd::icons(&path, macos.as_deref()),
        Cmd::NewModule { id } => cmd::new_module(&id),
        Cmd::Profiles => cmd::profiles(),
        Cmd::MockReleases { action } => match action {
            MockAction::Serve { scenario, port } => mock::serve(&scenario, port),
            MockAction::Publish { version, beta, notes, file } => {
                mock::publish(&version, beta, &notes, file.as_deref())
            }
            MockAction::Reset => mock::reset(),
            MockAction::Demo { scenario } => mock::demo(&scenario),
        },
        Cmd::CommitMsg { file } => hooks::commit_msg(&file),
        Cmd::Hook { hook: Hook::PreCommit } => hooks::pre_commit(),
        Cmd::Hooks => hooks::install(),
        Cmd::ScanKeys { range } => secrets::no_key_in(&["log", "-p", "--no-color", "-U0", &range]),
        Cmd::ReleaseMeta { field, editions, check_tag, check_new_tag } => release::meta(release::MetaQuery {
            field: field.as_deref(),
            editions,
            check_tag: check_tag.as_deref(),
            check_new_tag: check_new_tag.as_deref(),
        }),
        Cmd::ReleaseNotes { tag, previous, output } => {
            release::notes(&tag, previous.as_deref(), output.as_deref())
        }
        Cmd::Package { profile } => release::package(&profile),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    /// Each `cargo xtask …` call in `line`, as the words after `cargo xtask` (a `${{ … }}`
    /// expression counts as one word; the call ends at a shell operator).
    fn xtask_calls(line: &str) -> Vec<Vec<String>> {
        let mut calls = Vec::new();
        let mut rest = line;
        while let Some(at) = rest.find("cargo xtask ") {
            rest = &rest[at + "cargo xtask ".len()..];
            let end = rest.find(['>', '|', ';', '&', ')']).unwrap_or(rest.len());
            let mut call = rest[..end].to_string();
            while let (Some(open), Some(close)) = (call.find("${{"), call.find("}}")) {
                call.replace_range(open..close + 2, "X");
            }
            calls.push(call.split_whitespace().map(|w| w.trim_matches(['"', '\'']).to_string()).collect());
        }
        calls
    }

    #[test]
    fn workflows_call_existing_commands() {
        let dir = super::cmd::root().join(".github").join("workflows");
        let mut calls = 0;
        for name in ["ci.yml", "release.yml", "guards.yml"] {
            let text = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            for call in text.lines().flat_map(xtask_calls) {
                let argv = std::iter::once("xtask".to_string()).chain(call.iter().cloned());
                if let Err(e) = Cli::try_parse_from(argv) {
                    panic!("{name} calls `cargo xtask {}`, which xtask refuses:\n{e}", call.join(" "));
                }
                calls += 1;
            }
        }
        assert!(calls >= 6, "the workflows check, package and publish through xtask ({calls} calls)");
        assert_eq!(
            xtask_calls("x=$(cargo xtask release-meta --field tag) && y"),
            [["release-meta", "--field", "tag"]]
        );
    }
}
