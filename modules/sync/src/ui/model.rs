//! What the sync views show, worked out without the browser: the editor's tree rows, the
//! summary, the warning about mixed versions, and a group's state.

use std::collections::{BTreeSet, HashMap, HashSet};

use launcher_shared::BuildDto;

use crate::dto::{GroupState, SyncGroup, SyncMode, TreeEntry};

/// A path over this is shown red in the editor: syncing it copies a lot.
pub const LARGE: u64 = 1024 * 1024 * 1024;

/// A row of the editor's tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    pub entry: TreeEntry,
    /// How deep it is (indent).
    pub depth: usize,
    /// It is chosen, or inside a chosen folder.
    pub checked: bool,
    /// Inside a chosen folder (its own box follows the folder's).
    pub covered: bool,
    /// Another group or a module holds it: it cannot be chosen.
    pub locked: bool,
    /// «є в N з M», when it is not in every build.
    pub present: Option<(u32, u32)>,
    pub large: bool,
}

/// `path` is `folder` or inside it.
fn inside(path: &str, folder: &str) -> bool {
    path == folder || path.starts_with(&format!("{folder}/"))
}

/// The rows to show: the root's, each opened folder's below it, those matching `filter` (by
/// name, any case) and the folders on their way.
pub fn tree_rows(
    loaded: &HashMap<String, Vec<TreeEntry>>,
    expanded: &HashSet<String>,
    checked: &BTreeSet<String>,
    filter: &str,
) -> Vec<TreeRow> {
    let filter = filter.trim().to_lowercase();
    let mut rows = Vec::new();
    push_rows(loaded, expanded, checked, &filter, "", 0, &mut rows);
    rows
}

/// A loaded row under `entry` (or `entry` itself) matches `filter`.
fn matches(loaded: &HashMap<String, Vec<TreeEntry>>, entry: &TreeEntry, filter: &str) -> bool {
    filter.is_empty()
        || entry.name.to_lowercase().contains(filter)
        || loaded.get(&entry.path).is_some_and(|children| children.iter().any(|c| matches(loaded, c, filter)))
}

fn push_rows(
    loaded: &HashMap<String, Vec<TreeEntry>>,
    expanded: &HashSet<String>,
    checked: &BTreeSet<String>,
    filter: &str,
    dir: &str,
    depth: usize,
    rows: &mut Vec<TreeRow>,
) {
    for entry in loaded.get(dir).into_iter().flatten() {
        if !matches(loaded, entry, filter) {
            continue;
        }
        let locked = entry.lock.is_some();
        let covered = checked.iter().any(|c| *c != entry.path && inside(&entry.path, c));
        rows.push(TreeRow {
            entry: entry.clone(),
            depth,
            checked: !locked && (covered || checked.contains(&entry.path)),
            covered,
            locked,
            present: (entry.present < entry.total).then_some((entry.present, entry.total)),
            large: entry.size > LARGE,
        });
        // A match below shows its folder opened.
        if entry.dir
            && (expanded.contains(&entry.path) || (!filter.is_empty() && loaded.contains_key(&entry.path)))
        {
            push_rows(loaded, expanded, checked, filter, &entry.path, depth + 1, rows);
        }
    }
}

/// What the editor's last step says, in parts its text fills in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub builds: usize,
    pub paths: usize,
    /// The first three paths, folders with `/`, «…» after them when there are more.
    pub shown: String,
    /// The starting (or base) build's name.
    pub start: String,
}

pub fn summary(builds: &[BuildDto], group: &SyncGroup) -> Summary {
    let names: Vec<String> = group.paths.iter().take(3).map(|p| shown_path(p)).collect();
    let more = if group.paths.len() > 3 { " …" } else { "" };
    let start =
        builds.iter().find(|b| b.key == group.start).map_or_else(|| group.start.clone(), |b| b.name.clone());
    Summary {
        builds: group.builds.len(),
        paths: group.paths.len(),
        shown: format!("{}{more}", names.join(", ")),
        start,
    }
}

/// The name of build `key`; `None` once the list is loaded and it is not there (deleted). Before
/// the list loads, its key.
pub fn build_name(builds: &[BuildDto], key: &str) -> Option<String> {
    match builds.iter().find(|b| b.key == key) {
        Some(build) => Some(build.name.clone()),
        None if builds.is_empty() => Some(key.to_string()),
        None => None,
    }
}

/// A path as the views write it: a folder with `/` after it (one with an extension is a file).
pub fn shown_path(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.contains('.') { path.to_string() } else { format!("{path}/") }
}

/// The loader a build runs, by its component id (`fabric-loader-…`, `neoforge-…`): vanilla when
/// it names none.
fn loader_kind(build: &BuildDto) -> &'static str {
    let id = build.loader.as_deref().unwrap_or_default().to_lowercase();
    ["neoforge", "forge", "fabric", "quilt"]
        .into_iter()
        .find(|kind| id.starts_with(kind))
        .unwrap_or("vanilla")
}

/// The group's builds run different Minecraft versions or loaders, and it syncs mods or configs:
/// a warning, not a refusal.
pub fn mismatch(builds: &[BuildDto], group: &SyncGroup) -> bool {
    let modding = group.paths.iter().any(|p| {
        let p = p.to_lowercase();
        inside(&p, "mods") || inside(&p, "config")
    });
    let kinds: BTreeSet<(Option<String>, &str)> = builds
        .iter()
        .filter(|b| group.builds.contains(&b.key))
        .map(|b| (b.version.clone(), loader_kind(b)))
        .collect();
    modding && kinds.len() > 1
}

pub fn state_key(state: GroupState) -> &'static str {
    match state {
        GroupState::New => "sync_state_new",
        GroupState::Synced => "sync_state_synced",
        GroupState::Waiting => "sync_state_waiting",
        GroupState::Failed => "sync_state_failed",
        GroupState::Idle => "sync_state_idle",
    }
}

pub fn mode_key(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Latest => "sync_mode_latest",
        SyncMode::Base => "sync_mode_base",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use launcher_shared::BuildDto;

    use super::*;
    use crate::dto::{GroupState, Lock, SyncGroup, SyncMode, TreeEntry};

    fn entry(path: &str, dir: bool, size: u64, present: u32, lock: Option<Lock>) -> TreeEntry {
        TreeEntry {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            dir,
            size,
            present,
            total: 3,
            lock,
            managed_in: Vec::new(),
        }
    }

    fn build(key: &str, name: &str, version: &str, loader: Option<&str>) -> BuildDto {
        BuildDto {
            key: key.into(),
            version_id: key.into(),
            name: name.into(),
            version: Some(version.into()),
            loader: Some(loader.unwrap_or(version).to_string()),
            client: None,
            loader_version: None,
            game_dir: String::new(),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
            loader_kept: None,
            last_loader: None,
            leftover_mods: 0,
        }
    }

    #[test]
    fn the_tree_counts_builds_and_locks_rows() {
        let mut loaded: HashMap<String, Vec<TreeEntry>> = HashMap::new();
        loaded.insert(
            String::new(),
            vec![
                entry("config", true, 10, 3, None),
                entry("resourcepacks", true, 2 * 1024 * 1024 * 1024, 2, None),
                entry("servers.dat", false, 5, 3, Some(Lock::Taken { group: "Інша".into() })),
            ],
        );
        loaded.insert("config".into(), vec![entry("config/jei", true, 4, 1, None)]);
        let expanded: HashSet<String> = ["config".to_string()].into();
        let checked: BTreeSet<String> = ["config".to_string()].into();
        let rows = tree_rows(&loaded, &expanded, &checked, "");
        let shown: Vec<(&str, usize)> = rows.iter().map(|r| (r.entry.path.as_str(), r.depth)).collect();
        assert_eq!(shown, vec![("config", 0), ("config/jei", 1), ("resourcepacks", 0), ("servers.dat", 0)]);
        assert!(rows[0].checked && rows[1].checked && rows[1].covered, "inside a checked folder");
        assert_eq!(rows[1].present, Some((1, 3)), "«є в 1 з 3»");
        assert_eq!(rows[0].present, None, "in every build: nothing said");
        assert!(rows[2].large && !rows[0].large);
        assert!(rows[3].locked && !rows[3].checked);
        let searched = tree_rows(&loaded, &expanded, &checked, "JE");
        let found: Vec<&str> = searched.iter().map(|r| r.entry.path.as_str()).collect();
        assert_eq!(found, vec!["config", "config/jei"], "a match keeps its folders");
    }

    #[test]
    fn a_deleted_build_is_told_apart_once_the_list_is_loaded() {
        let builds = vec![build("aero", "Aero", "1.21.1", None)];
        assert_eq!(build_name(&builds, "aero").as_deref(), Some("Aero"));
        assert_eq!(build_name(&builds, "gone"), None, "deleted");
        assert_eq!(build_name(&[], "aero").as_deref(), Some("aero"), "not loaded yet: its key");
    }

    #[test]
    fn the_summary_names_the_paths_and_the_start() {
        let builds = [build("aero", "Aeronautics", "1.21.1", None), build("lite", "Lite", "1.21.1", None)];
        let group = SyncGroup {
            builds: vec!["aero".into(), "lite".into()],
            paths: vec!["options.txt".into(), "servers.dat".into(), "resourcepacks".into(), "xaero".into()],
            start: "lite".into(),
            ..SyncGroup::default()
        };
        let summary = summary(&builds, &group);
        assert_eq!((summary.builds, summary.paths), (2, 4));
        assert_eq!(summary.shown, "options.txt, servers.dat, resourcepacks/ …");
        assert_eq!(summary.start, "Lite");
    }

    #[test]
    fn different_versions_with_mods_warn() {
        let builds =
            [build("a", "A", "1.21.1", Some("fabric-loader-0.16.9-1.21.1")), build("b", "B", "1.20.1", None)];
        let group = |paths: &[&str]| SyncGroup {
            builds: vec!["a".into(), "b".into()],
            paths: paths.iter().map(|p| p.to_string()).collect(),
            ..SyncGroup::default()
        };
        assert!(mismatch(&builds, &group(&["mods"])));
        assert!(mismatch(&builds, &group(&["config/jei"])));
        assert!(!mismatch(&builds, &group(&["options.txt"])), "no mods or configs");
        let same = [build("a", "A", "1.21.1", None), build("b", "B", "1.21.1", None)];
        assert!(!mismatch(&same, &group(&["mods"])));
        assert_eq!(state_key(GroupState::Waiting), "sync_state_waiting");
        assert_eq!(mode_key(SyncMode::Base), "sync_mode_base");
    }
}
