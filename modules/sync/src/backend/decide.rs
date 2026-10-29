//! What a pass gives each build, decided from what the builds hold now and what the snapshot saw
//! after the last pass. Pure: it reads and writes no file.

use std::collections::{BTreeMap, BTreeSet};

use super::paths::key;
use super::scan::Found;
use super::scan::is_managed;
use super::store::{Snapshot, Version};
use crate::dto::{Reason, SyncMode};

/// What a pass decides from.
pub struct Decision<'a> {
    pub mode: SyncMode,
    /// The starting build (the base in mode `Base`).
    pub start: &'a str,
    /// The builds taking part, the starting build first when it does: ties go to the earlier.
    pub members: &'a [String],
    pub snapshot: &'a Snapshot,
    /// What each member holds now, by build and `paths::key`.
    pub found: &'a BTreeMap<String, BTreeMap<String, Found>>,
    pub group_paths: &'a [String],
    /// What each member may not touch: the paths a module manages there, and those that could
    /// not be read. Such a file is neither held nor written nor removed in that member.
    pub untouchable: &'a BTreeMap<String, Vec<String>>,
}

/// A file a member gets from another member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub build: String,
    pub key: String,
    /// As the source spells it.
    pub path: String,
    pub from: String,
}

/// A file a member loses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    pub build: String,
    pub key: String,
    pub path: String,
}

/// A member's version a pass overwrites or removes, held until the pass ends to put it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub build: String,
    pub key: String,
    pub path: String,
    pub from: Option<String>,
    pub reason: Reason,
    /// The member's own: changed there since its last pass, or never synced there. Only such a
    /// version stays in history; any other is the group's earlier one.
    pub own: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    /// A build the pass needs does not take part: nothing is done.
    pub wait: bool,
    pub writes: Vec<Write>,
    pub removes: Vec<Remove>,
    pub history: Vec<Saved>,
    /// The agreed version of every file decided (`None`: removed everywhere).
    pub agreed: BTreeMap<String, Option<Version>>,
    /// The paths changed in two members since the last pass.
    pub conflicts: Vec<String>,
}

/// The newest of `files` (member, file), the earlier member on a tie.
fn newest<'a>(files: impl Iterator<Item = (&'a String, &'a Found)>) -> Option<(&'a String, &'a Found)> {
    files.fold(None, |best, (m, f)| match best {
        Some((_, b)) if b.modified_ms >= f.modified_ms => best,
        _ => Some((m, f)),
    })
}

/// Decides, for every file under the group's paths, the version every member gets, by the
/// spec's rules for the group's mode.
pub fn decide(input: &Decision<'_>) -> Outcome {
    let mut out = Outcome::default();
    let snapshot = input.snapshot;
    let first = snapshot.members.is_empty();
    let member = |b: &str| input.members.iter().any(|m| m == b);
    let joining = |m: &str| !first && !snapshot.members.contains(m);
    let known_path = |p: &str| snapshot.paths.iter().any(|s| key(s) == key(p));
    let new_paths = !first && input.group_paths.iter().any(|p| !known_path(p));
    let needs_start = match input.mode {
        SyncMode::Base => true,
        SyncMode::Latest => first || new_paths || input.members.iter().any(|m| joining(m)),
    };
    if needs_start && !member(input.start) {
        out.wait = true;
        return out;
    }
    let covering = |k: &str| {
        input.group_paths.iter().find(|p| {
            let p = key(p);
            k == p || k.starts_with(&format!("{p}/"))
        })
    };
    let mut keys: BTreeSet<String> = input.found.values().flat_map(|files| files.keys().cloned()).collect();
    keys.extend(snapshot.files.keys().cloned());
    for k in keys {
        // A file of a path the group no longer syncs is left alone.
        let Some(path) = covering(&k) else { continue };
        let fresh = first || !known_path(path);
        let record = snapshot.files.get(&k);
        let file_of = |m: &str| input.found.get(m).and_then(|files| files.get(&k));
        let held = |m: &str| record.and_then(|r| r.held.get(m));
        let own = |m: &str, f: &Found| held(m).is_none_or(|h| h.hash != f.hash);
        let spelling = input
            .members
            .iter()
            .find_map(|m| file_of(m).map(|f| f.path.clone()))
            .or_else(|| record.map(|r| r.path.clone()))
            .unwrap_or_else(|| k.clone());
        // The members that may touch this file: not those that manage it or could not read it.
        let free = |m: &String| !input.untouchable.get(m).is_some_and(|own| is_managed(&k, own));
        let active: Vec<&String> = input.members.iter().filter(|m| free(m)).collect();
        if input.mode == SyncMode::Base && !active.iter().any(|m| *m == input.start) {
            // The base cannot say what this file is: the others keep theirs.
            continue;
        }
        let holders = || active.iter().copied().filter_map(|m| file_of(m).map(|f| (m, f)));
        // The version every member gets (`None`: none), and the members whose own change lost.
        let (winner, losers): (Option<(&String, &Found)>, Vec<&String>) = match input.mode {
            SyncMode::Base => (
                active.iter().copied().find(|m| *m == input.start).and_then(|m| file_of(m).map(|f| (m, f))),
                Vec::new(),
            ),
            SyncMode::Latest if fresh => {
                let start = active
                    .iter()
                    .copied()
                    .find(|m| *m == input.start)
                    .and_then(|m| file_of(m).map(|f| (m, f)));
                (start.or_else(|| newest(holders())), Vec::new())
            }
            SyncMode::Latest => {
                let established = || active.iter().copied().filter(|m| !joining(m));
                let changed: Vec<(&String, &Found)> = established()
                    .filter_map(|m| {
                        file_of(m).filter(|f| held(m).is_none_or(|h| h.hash != f.hash)).map(|f| (m, f))
                    })
                    .collect();
                let removed = established().any(|m| held(m).is_some() && file_of(m).is_none());
                if let Some(best) = newest(changed.iter().copied()) {
                    let losers = changed.iter().filter(|(m, _)| *m != best.0).map(|(m, _)| *m).collect();
                    (Some(best), losers)
                } else if removed && record.is_some_and(|r| r.agreed.is_some()) {
                    (None, Vec::new())
                } else {
                    match record.and_then(|r| r.agreed.as_ref()) {
                        // The agreed version, from a member that holds it; when none does (those that
                        // do run), every member keeps what it has until one of them takes part.
                        Some(agreed) => match holders().find(|(_, f)| f.hash == agreed.hash) {
                            Some(holder) => (Some(holder), Vec::new()),
                            None => continue,
                        },
                        // A joining member brings its own file.
                        None => (newest(holders().filter(|(m, _)| joining(m))), Vec::new()),
                    }
                }
            }
        };
        if !losers.is_empty() {
            out.conflicts.push(spelling.clone());
        }
        let reason_for = |m: &String, removing: bool| match input.mode {
            SyncMode::Base => Reason::Base,
            SyncMode::Latest if losers.contains(&m) => Reason::Conflict,
            SyncMode::Latest if removing => Reason::Removed,
            SyncMode::Latest => Reason::Changed,
        };
        match winner {
            Some((source, file)) => {
                for m in active.iter().copied().filter(|m| *m != source) {
                    match file_of(m) {
                        Some(own) if own.hash == file.hash => {}
                        present => {
                            if let Some(present) = present {
                                out.history.push(Saved {
                                    build: m.clone(),
                                    key: k.clone(),
                                    path: present.path.clone(),
                                    from: Some(source.clone()),
                                    reason: reason_for(m, false),
                                    own: own(m, present),
                                });
                            }
                            out.writes.push(Write {
                                build: m.clone(),
                                key: k.clone(),
                                path: file.path.clone(),
                                from: source.clone(),
                            });
                        }
                    }
                }
                let version = Version {
                    hash: file.hash.clone(),
                    size: file.size,
                    modified_ms: file.modified_ms,
                    from: source.clone(),
                };
                out.agreed.insert(k.clone(), Some(version));
            }
            None => {
                // Gone from the base, or a file the group knew: removed everywhere (a joining member's own
                // file is not the group's yet).
                let wanted_gone = input.mode == SyncMode::Base || record.is_some();
                if wanted_gone {
                    for (m, file) in holders().filter(|(m, _)| input.mode == SyncMode::Base || !joining(m)) {
                        out.history.push(Saved {
                            build: m.clone(),
                            key: k.clone(),
                            path: file.path.clone(),
                            from: None,
                            reason: reason_for(m, true),
                            own: own(m, file),
                        });
                        out.removes.push(Remove {
                            build: m.clone(),
                            key: k.clone(),
                            path: file.path.clone(),
                        });
                    }
                }
                if record.is_some() || input.mode == SyncMode::Base {
                    out.agreed.insert(k.clone(), None);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::backend::store::{FileRecord, Seen, Snapshot, Version};
    use crate::dto::{Reason, SyncMode};

    const KEY: &str = "options.txt";

    /// The agreed hash, and what each build held after the last pass.
    type Record<'a> = (Option<&'a str>, &'a [(&'a str, &'a str)]);

    /// One case: the group, the snapshot of `KEY`, and what each member holds of it now.
    struct Case<'a> {
        mode: SyncMode,
        members: &'a [&'a str],
        /// The builds and paths of the snapshot (empty: never synced).
        before: &'a [&'a str],
        path_known: bool,
        /// The agreed hash and what each build held after the last pass.
        record: Option<Record<'a>>,
        /// What each member holds now: hash and time.
        now: &'a [(&'a str, Option<(&'a str, u64)>)],
        /// Members that may not touch `KEY` (a module manages it there, or it could not be read).
        untouchable: &'a [&'a str],
    }

    impl Case<'_> {
        fn latest() -> Case<'static> {
            Case {
                mode: SyncMode::Latest,
                members: &[],
                before: &[],
                path_known: true,
                record: None,
                now: &[],
                untouchable: &[],
            }
        }

        fn run(&self) -> Outcome {
            let mut snapshot = Snapshot {
                members: self.before.iter().map(|b| b.to_string()).collect::<BTreeSet<_>>(),
                ..Snapshot::default()
            };
            if self.path_known && !self.before.is_empty() {
                snapshot.paths.insert(KEY.to_string());
            }
            if let Some((agreed, held)) = self.record {
                let version =
                    agreed.map(|h| Version { hash: h.into(), size: 1, modified_ms: 1, from: "a".into() });
                let held = held
                    .iter()
                    .map(|(b, h)| (b.to_string(), Seen { hash: h.to_string(), size: 1, modified_ms: 1 }))
                    .collect();
                snapshot.files.insert(KEY.into(), FileRecord { path: KEY.into(), agreed: version, held });
            }
            let mut found: BTreeMap<String, BTreeMap<String, Found>> = BTreeMap::new();
            for (build, file) in self.now {
                let files = found.entry(build.to_string()).or_default();
                if let Some((hash, time)) = file {
                    let file =
                        Found { path: KEY.into(), size: 1, modified_ms: *time, hash: hash.to_string() };
                    files.insert(KEY.into(), file);
                }
            }
            let members: Vec<String> = self.members.iter().map(|m| m.to_string()).collect();
            let paths = vec![KEY.to_string()];
            let untouchable: BTreeMap<String, Vec<String>> =
                self.untouchable.iter().map(|m| (m.to_string(), vec![KEY.to_string()])).collect();
            decide(&Decision {
                mode: self.mode,
                start: "a",
                members: &members,
                snapshot: &snapshot,
                found: &found,
                group_paths: &paths,
                untouchable: &untouchable,
            })
        }
    }

    fn writes(o: &Outcome) -> Vec<(String, String)> {
        o.writes.iter().map(|w| (w.build.clone(), w.from.clone())).collect()
    }

    fn removes(o: &Outcome) -> Vec<String> {
        o.removes.iter().map(|r| r.build.clone()).collect()
    }

    fn saved(o: &Outcome) -> Vec<(String, Reason)> {
        o.history.iter().map(|h| (h.build.clone(), h.reason)).collect()
    }

    fn own(o: &Outcome) -> Vec<(String, bool)> {
        o.history.iter().map(|h| (h.build.clone(), h.own)).collect()
    }

    fn agreed(o: &Outcome) -> Option<String> {
        o.agreed.get(KEY).cloned().flatten().map(|v| v.hash)
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn reasons(list: &[(&str, Reason)]) -> Vec<(String, Reason)> {
        list.iter().map(|(a, r)| (a.to_string(), *r)).collect()
    }

    const ABC: &[&str] = &["a", "b", "c"];
    const HELD: &[(&str, &str)] = &[("a", "h0"), ("b", "h0"), ("c", "h0")];

    #[test]
    fn latest_a_change_in_one_spreads() {
        let o = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", Some(("h0", 1))), ("b", Some(("h1", 5))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("a", "b"), ("c", "b")]));
        assert_eq!(saved(&o), reasons(&[("a", Reason::Changed), ("c", Reason::Changed)]));
        assert_eq!(agreed(&o).as_deref(), Some("h1"));
        assert!(o.conflicts.is_empty() && !o.wait);
    }

    #[test]
    fn latest_a_file_added_in_one_spreads() {
        let o = Case {
            members: &["a", "b"],
            before: &["a", "b"],
            now: &[("a", None), ("b", Some(("h1", 5)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("a", "b")]));
        assert!(o.history.is_empty(), "nothing was there to keep");
        assert_eq!(agreed(&o).as_deref(), Some("h1"));
    }

    #[test]
    fn latest_changed_in_two_the_newer_wins_and_the_older_is_a_conflict() {
        let o = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", Some(("h1", 5))), ("b", Some(("h2", 9))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("a", "b"), ("c", "b")]));
        assert_eq!(saved(&o), reasons(&[("a", Reason::Conflict), ("c", Reason::Changed)]));
        assert_eq!(o.conflicts, vec![KEY.to_string()]);
        assert_eq!(agreed(&o).as_deref(), Some("h2"));
    }

    #[test]
    fn latest_removed_in_one_is_removed_everywhere() {
        let o = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", None), ("b", Some(("h0", 1))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert!(o.writes.is_empty());
        assert_eq!(removes(&o), vec!["b", "c"]);
        assert_eq!(saved(&o), reasons(&[("b", Reason::Removed), ("c", Reason::Removed)]));
        assert_eq!(o.agreed.get(KEY), Some(&None), "agreed: gone");
    }

    #[test]
    fn latest_a_change_beats_a_removal() {
        let o = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", None), ("b", Some(("h1", 5))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("a", "b"), ("c", "b")]));
        assert!(o.removes.is_empty());
        assert_eq!(saved(&o), reasons(&[("c", Reason::Changed)]));
        assert_eq!(agreed(&o).as_deref(), Some("h1"));
    }

    #[test]
    fn latest_the_first_sync_takes_the_starting_build_s() {
        let o = Case {
            members: ABC,
            now: &[("a", Some(("h1", 1))), ("b", Some(("h2", 9))), ("c", None)],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("b", "a"), ("c", "a")]), "even over a newer file");
        assert_eq!(saved(&o), reasons(&[("b", Reason::Changed)]));
        assert_eq!(agreed(&o).as_deref(), Some("h1"));
        let only_others =
            Case { members: ABC, now: &[("a", None), ("b", Some(("h2", 9))), ("c", None)], ..Case::latest() }
                .run();
        assert_eq!(
            writes(&only_others),
            pairs(&[("a", "b"), ("c", "b")]),
            "a file only another has is added"
        );
    }

    #[test]
    fn latest_a_joining_build_gets_the_agreed_files_and_brings_its_own() {
        let o = Case {
            members: &["a", "b", "d"],
            before: &["a", "b"],
            record: Some((Some("h0"), &[("a", "h0"), ("b", "h0")])),
            now: &[("a", Some(("h0", 1))), ("b", Some(("h0", 1))), ("d", Some(("h9", 99)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("d", "a")]));
        assert_eq!(saved(&o), reasons(&[("d", Reason::Changed)]));
        let its_own = Case {
            members: &["a", "b", "d"],
            before: &["a", "b"],
            now: &[("a", None), ("b", None), ("d", Some(("h5", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&its_own), pairs(&[("a", "d"), ("b", "d")]));
    }

    #[test]
    fn latest_a_joining_path_takes_the_starting_build_s() {
        let o = Case {
            members: &["a", "b"],
            before: &["a", "b"],
            path_known: false,
            now: &[("a", Some(("h1", 1))), ("b", Some(("h2", 9)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("b", "a")]));
        assert_eq!(saved(&o), reasons(&[("b", Reason::Changed)]));
    }

    #[test]
    fn latest_a_first_sync_without_its_starting_build_waits() {
        let o = Case { members: &["b", "c"], now: &[("b", Some(("h2", 9))), ("c", None)], ..Case::latest() }
            .run();
        assert!(o.wait && o.writes.is_empty() && o.removes.is_empty());
    }

    #[test]
    fn latest_a_file_no_member_holds_the_agreed_version_of_is_left_alone() {
        // A edited it while B and C ran; now A runs: B and C keep theirs until A takes part.
        let o = Case {
            members: &["b", "c"],
            before: ABC,
            record: Some((Some("h1"), &[("a", "h1"), ("b", "h0"), ("c", "h0")])),
            now: &[("b", Some(("h0", 1))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert!(o.writes.is_empty() && o.removes.is_empty() && o.history.is_empty(), "{o:?}");
        assert!(!o.agreed.contains_key(KEY), "the agreed version stays as it was");
    }

    #[test]
    fn a_file_a_member_may_not_touch_is_neither_its_nor_written_there() {
        // The server build manages it: it does not hold it here, which is no removal.
        let latest = Case {
            members: &["a", "s"],
            before: &["a", "s"],
            record: Some((Some("h0"), &[("a", "h0"), ("s", "h0")])),
            now: &[("a", Some(("h0", 1))), ("s", None)],
            untouchable: &["s"],
            ..Case::latest()
        }
        .run();
        assert!(latest.writes.is_empty() && latest.removes.is_empty(), "{latest:?}");
        // The base manages it: the others keep theirs.
        let base = Case {
            mode: SyncMode::Base,
            members: &["a", "b"],
            before: &["a", "b"],
            now: &[("a", None), ("b", Some(("h3", 5)))],
            untouchable: &["a"],
            ..Case::latest()
        }
        .run();
        assert!(base.writes.is_empty() && base.removes.is_empty(), "{base:?}");
    }

    #[test]
    fn base_an_edit_in_another_build_is_undone() {
        let o = Case {
            mode: SyncMode::Base,
            members: &["a", "b"],
            before: &["a", "b"],
            record: Some((Some("h0"), &[("a", "h0"), ("b", "h0")])),
            now: &[("a", Some(("h0", 1))), ("b", Some(("h1", 99)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("b", "a")]), "though newer");
        assert_eq!(saved(&o), reasons(&[("b", Reason::Base)]));
        assert_eq!(agreed(&o).as_deref(), Some("h0"));
    }

    #[test]
    fn base_the_base_s_edit_spreads() {
        let o = Case {
            mode: SyncMode::Base,
            members: &["a", "b"],
            before: &["a", "b"],
            record: Some((Some("h0"), &[("a", "h0"), ("b", "h0")])),
            now: &[("a", Some(("h1", 5))), ("b", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(writes(&o), pairs(&[("b", "a")]));
        assert_eq!(saved(&o), reasons(&[("b", Reason::Base)]));
    }

    #[test]
    fn base_a_file_only_another_build_has_is_removed() {
        let o = Case {
            mode: SyncMode::Base,
            members: &["a", "b"],
            before: &["a", "b"],
            now: &[("a", None), ("b", Some(("h3", 5)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(removes(&o), vec!["b"]);
        assert_eq!(saved(&o), reasons(&[("b", Reason::Base)]));
    }

    #[test]
    fn a_version_is_its_member_s_own_when_changed_there_or_never_synced() {
        let spread = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", Some(("h0", 1))), ("b", Some(("h1", 5))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(own(&spread), [("a".to_string(), false), ("c".to_string(), false)], "the group's");
        let conflict = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", Some(("h1", 5))), ("b", Some(("h2", 9))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(own(&conflict), [("a".to_string(), true), ("c".to_string(), false)], "the losing edit");
        let removed = Case {
            members: ABC,
            before: ABC,
            record: Some((Some("h0"), HELD)),
            now: &[("a", None), ("b", Some(("h0", 1))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert!(own(&removed).iter().all(|(_, own)| !own), "{removed:?}");
        let first = Case {
            members: ABC,
            now: &[("a", Some(("h1", 1))), ("b", Some(("h2", 9))), ("c", None)],
            ..Case::latest()
        }
        .run();
        assert_eq!(own(&first), [("b".to_string(), true)], "never synced there");
        let undone = Case {
            mode: SyncMode::Base,
            members: &["a", "b"],
            before: &["a", "b"],
            record: Some((Some("h0"), &[("a", "h0"), ("b", "h0")])),
            now: &[("a", Some(("h0", 1))), ("b", Some(("h1", 99)))],
            ..Case::latest()
        }
        .run();
        assert_eq!(own(&undone), [("b".to_string(), true)], "an edit the base undoes");
    }

    #[test]
    fn base_without_the_base_waits() {
        let o = Case {
            mode: SyncMode::Base,
            members: &["b", "c"],
            before: &["a", "b", "c"],
            now: &[("b", Some(("h1", 5))), ("c", Some(("h0", 1)))],
            ..Case::latest()
        }
        .run();
        assert!(o.wait && o.writes.is_empty() && o.removes.is_empty());
    }
}
