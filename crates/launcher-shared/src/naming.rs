//! A new build's name, checked the same way by the backend, the app's dialogs and the modules':
//! trimmed, not empty, not taken by another build (ignoring case).

use crate::BuildDto;

/// `base`, or `base (2)`, `base (3)`… — the first name nobody uses (ignoring case).
pub fn unique_name(base: &str, taken: &[String]) -> String {
    let base = base.trim();
    let is_taken = |name: &str| taken.iter().any(|t| t.trim().to_lowercase() == name.to_lowercase());
    if !is_taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} ({n})")).find(|name| !is_taken(name)).expect("an unused name exists")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    Taken,
}

impl NameProblem {
    /// Its message's key; `version_exists` takes `{name}`.
    pub fn key(self) -> &'static str {
        match self {
            NameProblem::Empty => "empty_version_name",
            NameProblem::Taken => "version_exists",
        }
    }
}

/// The trimmed name of a new build; empty and taken names (ignoring case) are refused, as the
/// backend does.
pub fn check_new_name(raw: &str, builds: &[BuildDto]) -> Result<String, NameProblem> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    let lower = name.to_lowercase();
    if builds.iter().any(|b| b.name.trim().to_lowercase() == lower) {
        return Err(NameProblem::Taken);
    }
    Ok(name.to_string())
}

/// `title` ("Minecraft 1.21.1", a modpack's name) made unique among the build names.
pub fn default_build_name(title: &str, builds: &[BuildDto]) -> String {
    let taken: Vec<String> = builds.iter().map(|b| b.name.clone()).collect();
    unique_name(title, &taken)
}
