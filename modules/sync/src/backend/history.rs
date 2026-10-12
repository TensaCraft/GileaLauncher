//! What sync passes overwrote or removed, kept with each build's backups: listed per group,
//! swept by age and size like the «Сховище» rule says.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use launcher_core::backups::{BuildBackups, stamped};
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::usage::rule_items;
use launcher_shared::StorageRule;
use serde::Deserialize;

use super::apply::Member;
use super::paths::normalize;
use crate::dto::{HistoryEntry, Reason};

/// One line of a pass folder's `change.json`.
#[derive(Deserialize)]
struct Change {
    group: String,
    path: String,
    from: Option<String>,
    at_ms: u64,
    reason: Reason,
}

/// The pass folders in `shelf`, by name.
fn passes(shelf: &BuildBackups) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(shelf.sync())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(name, _)| stamped(name))
        .collect();
    found.sort();
    found
}

/// The versions group `group_id`'s passes kept in its `members`, newest first.
pub fn entries(members: &[Member], group_id: &str) -> Vec<HistoryEntry> {
    let mut out = Vec::new();
    for member in members {
        for (name, folder) in passes(&member.shelf) {
            let Some(changes) = fs::read(folder.join("change.json"))
                .ok()
                .and_then(|raw| serde_json::from_slice::<Vec<Change>>(&raw).ok())
            else {
                continue;
            };
            for change in changes.into_iter().filter(|c| c.group == group_id) {
                let size = fs::metadata(folder.join("files").join(&change.path)).map_or(0, |m| m.len());
                out.push(HistoryEntry {
                    id: format!("{}/{name}/{}", member.key, change.path),
                    build: member.key.clone(),
                    group: change.group,
                    path: change.path,
                    from: change.from,
                    reason: change.reason,
                    at_ms: change.at_ms,
                    size,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.at_ms.cmp(&a.at_ms).then_with(|| a.build.cmp(&b.build)).then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// The kept file history entry `id` (`<build>/<pass folder>/<path>`) names, among `members`.
pub fn version_file(members: &[Member], id: &str) -> Option<PathBuf> {
    let (build, rest) = id.split_once('/')?;
    let (folder, path) = rest.split_once('/')?;
    if !stamped(folder) {
        return None;
    }
    let path = normalize(path).ok()?;
    let member = members.iter().find(|m| m.key == build)?;
    Some(member.shelf.sync().join(folder).join("files").join(path)).filter(|p| p.is_file())
}

/// Removes `shelf`'s sync history past the «Сховище» rule (14 days, 2 GB), unless its switch is
/// off. What it freed.
pub fn sweep(shelf: &BuildBackups, config: &ConfigStore, now: SystemTime) -> u64 {
    let rule = StorageRule::SyncHistory;
    if config.get_str(rule.config_key()).is_some_and(|v| v.eq_ignore_ascii_case("no")) {
        return 0;
    }
    let mut freed = 0;
    for folder in rule_items(rule, Path::new(""), shelf, now) {
        let size = launcher_core::storage::usage::dir_size(&folder);
        match fs::remove_dir_all(&folder) {
            Ok(()) => freed += size,
            Err(e) => tracing::warn!("Unable to remove {}: {e}", folder.display()),
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use launcher_core::backups::BuildBackups;

    use super::*;
    use crate::backend::apply::{Member, apply};
    use crate::backend::decide::{Outcome, Saved, Write};
    use crate::dto::{Reason, SyncGroup};

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn member(tmp: &Path, key: &str) -> Member {
        let game = tmp.join("games").join(key);
        fs::create_dir_all(&game).unwrap();
        Member { key: key.into(), game, shelf: BuildBackups::of(&tmp.join("backups"), key) }
    }

    fn overwrite(tmp: &Path, group: &str, path: &str, secs: u64) {
        let (a, b) = (member(tmp, "a"), member(tmp, "b"));
        put(&a.game, path, format!("new {secs}").as_bytes());
        put(&b.game, path, format!("old {secs}").as_bytes());
        let outcome = Outcome {
            writes: vec![Write { build: "b".into(), key: path.into(), path: path.into(), from: "a".into() }],
            history: vec![Saved {
                build: "b".into(),
                key: path.into(),
                path: path.into(),
                from: Some("a".into()),
                reason: Reason::Changed,
            }],
            ..Outcome::default()
        };
        let group =
            SyncGroup { id: group.into(), name: group.into(), start: "a".into(), ..SyncGroup::default() };
        apply(&[a, b], &outcome, &group, UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn entries_of_a_group_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        overwrite(tmp.path(), "g1", "options.txt", 1_791_800_000);
        overwrite(tmp.path(), "g2", "servers.dat", 1_791_800_100);
        overwrite(tmp.path(), "g1", "options.txt", 1_791_800_200);
        let members = [member(tmp.path(), "a"), member(tmp.path(), "b")];
        let found = entries(&members, "g1");
        assert_eq!(found.len(), 2, "the other group's change is not listed");
        assert!(found[0].at_ms > found[1].at_ms, "newest first");
        assert_eq!((found[0].build.as_str(), found[0].path.as_str()), ("b", "options.txt"));
        assert_eq!(found[0].reason, Reason::Changed);
        assert_eq!(found[0].size, "old 1791800200".len() as u64);
        assert!(
            found[0].id.starts_with("b/2026-") && found[0].id.ends_with("/options.txt"),
            "{}",
            found[0].id
        );
        assert_eq!(fs::read(version_file(&members, &found[1].id).unwrap()).unwrap(), b"old 1791800000");
    }
}
