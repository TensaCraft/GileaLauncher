//! The CurseForge API key stays out of the repository and out of the interface: only the
//! module's backend reads it, only the release workflow and `cargo xtask dev` pass it in.

use std::process::Command;

use anyhow::{Result, bail};

/// A bcrypt-shaped string as CurseForge keys are (`$2a$10$` and 53 characters).
fn key_shaped(text: &str) -> bool {
    let bytes = text.as_bytes();
    let body = |b: &u8| b.is_ascii_alphanumeric() || *b == b'.' || *b == b'/';
    bytes.windows(7).enumerate().any(|(at, w)| {
        w[0] == b'$'
            && w[1] == b'2'
            && matches!(w[2], b'a' | b'b' | b'y')
            && w[3] == b'$'
            && w[4].is_ascii_digit()
            && w[5].is_ascii_digit()
            && w[6] == b'$'
            && bytes.len() >= at + 7 + 53
            && bytes[at + 7..at + 7 + 53].iter().all(body)
    })
}

/// A key-shaped string among the lines a diff (`git diff`, `git log -p`) adds; file headers
/// (`+++`) are not lines of a file.
fn diff_adds_key(diff: &str) -> bool {
    diff.lines().filter(|line| line.starts_with('+') && !line.starts_with("+++")).any(key_shaped)
}

/// No key-shaped string in the changes `git <args>` shows (the staged ones before a commit, a
/// push's commits in CI: a key added and removed again still stays in the history).
pub fn no_key_in(args: &[&str]) -> Result<()> {
    let out = Command::new("git").args(args).current_dir(crate::cmd::root()).output()?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    if diff_adds_key(&String::from_utf8_lossy(&out.stdout)) {
        bail!(
            "a key-shaped string ($2a$...) is in the changes of `git {}`: keys belong in secrets only",
            args.join(" ")
        );
    }
    Ok(())
}

/// The files of the repository (tracked or new, not ignored) whose text has `needle`.
#[cfg(test)]
fn files_with(needle: &str) -> Vec<String> {
    let root = crate::cmd::root();
    crate::brand::repo_files(&root)
        .into_iter()
        .filter(|f| std::fs::read_to_string(root.join(f)).is_ok_and(|text| text.contains(needle)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::repo_files;

    #[test]
    fn a_key_is_recognised_by_its_shape() {
        let fake = format!("${}${}${}", "2a", "10", "a".repeat(53));
        assert!(key_shaped(&format!("key = \"{fake}\"")));
        assert!(!key_shaped(&format!("${}${}${}", "2a", "10", "a".repeat(20))), "too short");
        assert!(!key_shaped("price: $2 and $10"));
    }

    #[test]
    fn a_key_added_in_a_change_is_caught() {
        let key = format!("${}${}${}", "2a", "10", "b".repeat(53));
        let added = format!("diff --git a/x b/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+key = \"{key}\"\n");
        assert!(diff_adds_key(&added));
        let removed = format!("+++ b/x\n-key = \"{key}\"\n+nothing\n");
        assert!(!diff_adds_key(&removed), "taking a key out is no leak");
        assert!(no_key_in(&["log", "-1", "--format=%H"]).is_ok());
        assert!(no_key_in(&["no-such-command"]).is_err(), "a failed git is no clean answer");
    }

    #[test]
    fn no_api_key_is_in_the_repository() {
        let root = crate::cmd::root();
        let found: Vec<String> = repo_files(&root)
            .into_iter()
            .filter(|f| std::fs::read_to_string(root.join(f)).is_ok_and(|text| key_shaped(&text)))
            .collect();
        assert!(found.is_empty(), "key-shaped strings in {found:?}: keys belong in secrets only");
    }

    #[test]
    fn only_the_backend_and_the_build_name_the_key() {
        let allowed = |f: &str| {
            matches!(
                f,
                "modules/curseforge/src/backend/key.rs"
                    | "xtask/src/cmd.rs"
                    | "xtask/src/secrets.rs"
                    | ".github/workflows/release.yml"
                    | "README.md"
            ) || f.starts_with("docs/")
        };
        let stray: Vec<String> =
            files_with("CURSEFORGE_API_KEY").into_iter().filter(|f| !allowed(f)).collect();
        assert!(stray.is_empty(), "the key's variable named in {stray:?}");
        let header: Vec<String> = files_with("x-api-key")
            .into_iter()
            .filter(|f| {
                !f.starts_with("modules/curseforge/src/backend/")
                    && !f.starts_with("modules/curseforge/tests/")
                    && f != "xtask/src/secrets.rs"
                    && !f.starts_with("docs/")
            })
            .collect();
        assert!(header.is_empty(), "the key's header outside the CurseForge backend: {header:?}");
    }
}
