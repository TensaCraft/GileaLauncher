//! Commit conventions and the repository's git hooks (`.githooks`).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Commit kinds; release notes group the commits by them.
pub const KINDS: &[&str] =
    &["feat", "fix", "ui", "perf", "refactor", "docs", "test", "build", "ci", "chore", "release"];

/// A commit subject `kind(scope)!: summary`.
#[derive(Debug, PartialEq, Eq)]
pub struct Subject<'a> {
    pub kind: &'a str,
    pub scope: Option<&'a str>,
    pub breaking: bool,
    pub summary: &'a str,
}

/// `subject` split into its parts, when it has the `kind(scope)!: summary` shape.
pub fn parse_subject(subject: &str) -> Option<Subject<'_>> {
    let (head, summary) = subject.split_once(": ")?;
    let (head, breaking) = match head.strip_suffix('!') {
        Some(head) => (head, true),
        None => (head, false),
    };
    let (kind, scope) = match head.split_once('(') {
        Some((kind, rest)) => (kind, Some(rest.strip_suffix(')')?)),
        None => (head, None),
    };
    let word = |w: &str, extra: &[u8]| {
        !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    (word(kind, b"") && scope.is_none_or(|s| word(s, b"._/-")) && !summary.trim().is_empty())
        .then(|| Subject { kind, scope, breaking, summary: summary.trim() })
}

/// Shortest and longest summary.
const SUMMARY_LEN: (usize, usize) = (15, 120);
/// Summaries that tell a release note nothing.
const VAGUE: &[&str] = &["change", "changes", "fix", "misc", "update", "updates", "up", "wip"];

/// Git's own subjects, which keep their shape.
fn exempt(subject: &str) -> bool {
    ["Merge ", "Revert ", "fixup! ", "squash! "].iter().any(|p| subject.starts_with(p))
}

/// What is wrong with commit subject `subject`; nothing when it follows the conventions.
pub fn subject_errors(subject: &str) -> Vec<String> {
    let subject = subject.trim();
    if exempt(subject) {
        return Vec::new();
    }
    let Some(parsed) = parse_subject(subject) else {
        return vec!["use the Conventional Commits shape: kind(scope): English summary".into()];
    };
    let mut errors = Vec::new();
    if !KINDS.contains(&parsed.kind) {
        errors.push(format!("unknown kind '{}' (one of: {})", parsed.kind, KINDS.join(", ")));
    }
    if parsed.scope.is_some_and(|s| s.bytes().any(|b| b.is_ascii_uppercase())) {
        errors.push("the scope is lower case".into());
    }
    let len = parsed.summary.chars().count();
    if len < SUMMARY_LEN.0 || len > SUMMARY_LEN.1 {
        errors.push(format!("the summary has {}–{} characters, not {len}", SUMMARY_LEN.0, SUMMARY_LEN.1));
    }
    if VAGUE.contains(&parsed.summary.trim_end_matches('.').to_lowercase().as_str()) {
        errors.push("the summary is too vague for release notes".into());
    }
    if !subject.is_ascii() {
        errors.push("the subject is English ASCII text".into());
    }
    errors
}

/// The first line of commit message `message` that is neither blank nor a comment.
pub fn subject_of(message: &str) -> &str {
    message.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("")
}

/// `commit-msg` hook: refuses message file `path` unless its subject follows the conventions.
pub fn commit_msg(path: &Path) -> Result<()> {
    let message = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let subject = subject_of(&message);
    let errors = subject_errors(subject);
    if errors.is_empty() {
        return Ok(());
    }
    eprintln!("Invalid commit message:\n  {}", if subject.is_empty() { "<empty>" } else { subject });
    for error in &errors {
        eprintln!("- {error}");
    }
    eprintln!(
        "\nExamples:\n  fix(updater): each edition updates from its own files\n  ui(home): cards keep one height in every row\n  release: bump version to 0.2.0"
    );
    bail!("the commit message does not follow the conventions")
}

/// Setting that turns the pre-commit checks off for one commit.
pub const SKIP_PRECOMMIT: &str = "LAUNCHER_SKIP_PRECOMMIT";

/// What `pre-commit` runs, in order (program and arguments).
pub fn pre_commit_steps() -> Vec<Vec<String>> {
    let cargo = crate::cmd::cargo();
    [
        vec![cargo.as_str(), "fmt", "--all", "--", "--check"],
        vec!["git", "diff", "--check", "--cached"],
        vec![cargo.as_str(), "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    ]
    .into_iter()
    .map(|step| step.into_iter().map(str::to_string).collect())
    .collect()
}

/// The app's frontend is not built yet in the repository at `root` (a fresh clone): clippy of
/// the app needs it.
pub fn needs_frontend(root: &Path) -> bool {
    !root.join("crates").join("launcher-ui").join("dist").join("index.html").is_file()
}

/// `pre-commit` hook: formatting, whitespace in the staged changes and clippy.
pub fn pre_commit() -> Result<()> {
    if std::env::var(SKIP_PRECOMMIT).as_deref() == Ok("1") {
        println!("Skipping the pre-commit checks ({SKIP_PRECOMMIT}=1).");
        return Ok(());
    }
    crate::secrets::no_key_in(&["diff", "--cached", "--no-color", "-U0"])?;
    if needs_frontend(&crate::cmd::root()) {
        crate::cmd::frontend_for_tests()?;
    }
    for step in pre_commit_steps() {
        crate::cmd::run(
            std::process::Command::new(&step[0]).args(&step[1..]).current_dir(crate::cmd::root()),
        )?;
    }
    Ok(())
}

/// Makes git run the hooks in `.githooks`.
pub fn install() -> Result<()> {
    crate::cmd::run(
        std::process::Command::new("git")
            .args(["config", "core.hooksPath", ".githooks"])
            .current_dir(crate::cmd::root()),
    )?;
    println!("git now runs the hooks in .githooks");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_messages_follow_the_conventions() {
        for good in [
            "feat(updater): each edition updates from its own files",
            "fix: the window keeps its size after a restart",
            "ui(home): cards keep one height in every row",
            "release: bump version to 0.2.0",
            "refactor(core)!: builds are keyed by their folder",
        ] {
            assert_eq!(subject_errors(good), Vec::<String>::new(), "{good}");
        }
        for bad in [
            "",
            "added a thing to the launcher",
            "feature(ui): cards keep one height in every row",
            "fix: short",
            "fix: update",
            "fix(UI): cards keep one height in every row",
            "fix: картки мають однакову висоту в кожному ряду",
            "fix:cards keep one height in every row",
            &format!("fix: {}", "a".repeat(121)),
        ] {
            assert!(!subject_errors(bad).is_empty(), "{bad:?} must be refused");
        }
        let parsed = parse_subject("refactor(core)!: builds are keyed by their folder").unwrap();
        assert_eq!(
            parsed,
            Subject {
                kind: "refactor",
                scope: Some("core"),
                breaking: true,
                summary: "builds are keyed by their folder"
            }
        );
        assert_eq!(
            subject_of("\n# a comment\n  docs: players read how to install it\nbody"),
            "docs: players read how to install it"
        );
    }

    #[test]
    fn a_fresh_clone_builds_the_frontend_first() {
        let dir = tempfile::tempdir().unwrap();
        assert!(needs_frontend(dir.path()), "no dist yet");
        let dist = dir.path().join("crates").join("launcher-ui").join("dist");
        fs::create_dir_all(&dist).unwrap();
        fs::write(dist.join("index.html"), "<html></html>").unwrap();
        assert!(!needs_frontend(dir.path()));
    }

    #[test]
    fn merges_and_reverts_pass() {
        for subject in [
            "Merge branch 'feat/x' into main",
            "Revert \"fix: the window keeps its size after a restart\"",
            "fixup! fix: the window keeps its size after a restart",
            "squash! feat: something",
        ] {
            assert_eq!(subject_errors(subject), Vec::<String>::new(), "{subject}");
        }
    }
}
