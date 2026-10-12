//! A group's paths: written one way (`normalize`), compared one way (`key`, case-blind on
//! Windows), and held by one group per build (`taken`).

use launcher_shared::{AppError, AppResult, ErrorCode};

use super::blacklist::blocked;
use crate::dto::SyncGroup;

fn invalid(raw: &str, why: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("{raw:?}: {why}")).with_param("path", raw)
}

/// `raw` as a group keeps it: relative to a build's folder, `/`-separated, without empty, `.` or
/// `..` parts; refused when it leaves the folder or is never synced (`blocked`).
pub fn normalize(raw: &str) -> AppResult<String> {
    let path = raw.trim().replace('\\', "/");
    let segments: Vec<&str> = path.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(invalid(raw, "no path"));
    }
    if segments.iter().any(|s| *s == "." || *s == ".." || s.contains(':')) {
        return Err(invalid(raw, "outside the build's folder"));
    }
    let path = segments.join("/");
    if blocked(&path) {
        return Err(invalid(raw, "never synced"));
    }
    Ok(path)
}

/// What `path` is compared as: case-blind on Windows, as its files are.
pub fn key(path: &str) -> String {
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

/// `a` and `b` are the same path or one is inside the other.
pub fn overlaps(a: &str, b: &str) -> bool {
    let (a, b) = (key(a), key(b));
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// The name of another group (not `editing`) that holds a path overlapping `path` for `build`.
pub fn taken(groups: &[SyncGroup], editing: Option<&str>, build: &str, path: &str) -> Option<String> {
    groups
        .iter()
        .filter(|g| Some(g.id.as_str()) != editing && g.builds.iter().any(|b| b == build))
        .find(|g| g.paths.iter().any(|held| overlaps(held, path)))
        .map(|g| g.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{SyncGroup, SyncMode};

    #[test]
    fn normalize_refuses_escapes_and_blocked() {
        assert_eq!(normalize(r" config\jei\ ").unwrap(), "config/jei");
        assert_eq!(normalize("/xaero/").unwrap(), "xaero");
        assert_eq!(normalize("config//jei").unwrap(), "config/jei");
        for bad in ["", "/", "..", "a/../b", "./a", "C:/Windows", "saves/World", ".launcher/x", "a.tmp"] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn overlaps_inside_around_and_equal() {
        assert!(overlaps("config", "config/jei/x.json"), "inside");
        assert!(overlaps("config/jei/x.json", "config"), "around");
        assert!(overlaps("options.txt", "options.txt"), "equal");
        assert!(!overlaps("config", "configs"), "a name that only starts the same");
        assert!(!overlaps("xaero", "config/xaero"));
        assert_eq!(overlaps("Config/JEI", "config/jei"), cfg!(windows), "case counts only off Windows");
    }

    #[test]
    fn a_path_another_group_holds_for_the_build_is_taken() {
        let group = |id: &str, name: &str, builds: &[&str], paths: &[&str]| SyncGroup {
            id: id.into(),
            name: name.into(),
            builds: builds.iter().map(|b| b.to_string()).collect(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            mode: SyncMode::Latest,
            start: builds[0].into(),
        };
        let groups = vec![
            group("g1", "Мінікарта", &["aero", "lite"], &["xaero"]),
            group("g2", "Налаштування", &["aero", "zeta"], &["options.txt", "config/jei"]),
        ];
        assert_eq!(taken(&groups, None, "aero", "xaero/minimap.txt").as_deref(), Some("Мінікарта"));
        assert_eq!(taken(&groups, None, "zeta", "config").as_deref(), Some("Налаштування"), "around");
        assert_eq!(taken(&groups, None, "zeta", "xaero"), None, "not that build's group");
        assert_eq!(taken(&groups, Some("g1"), "aero", "xaero"), None, "the group being edited");
    }
}
