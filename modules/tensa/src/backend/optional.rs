//! Which of the server's optional mods (its whitelist) a build has, and whether each is the
//! server's version: from the launcher's note, the files' hashes and the server's preserve rules.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use launcher_core::net::downloader::{ExpectedHash, HashKind, hash_file};
use launcher_core::storage::atomic::atomic_write;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};

use super::manifest::{PreserveKind, PreserveRule, preserved};
use super::plan::{PlannedFile, name_key};
use super::whitelist::Item;

/// The launcher's note of the optional mods it put in a build, by whitelist id.
pub const NOTE: &str = ".launcher/tensa-optional.json";
const DISABLED: &str = ".disabled";
/// The folder optional mods live in.
const MODS: &str = "mods";

/// An optional mod as the note keeps it: its file in `mods/` and what it was when noted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Noted {
    pub file: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified_ms: u64,
}

pub type Note = BTreeMap<String, Noted>;

/// The build's note; empty when there is none or it cannot be read.
pub fn read_note(root: &Path) -> Note {
    fs::read(root.join(NOTE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn write_note(root: &Path, note: &Note) -> AppResult<()> {
    let path = root.join(NOTE);
    let io = |e: std::io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let bytes = serde_json::to_vec_pretty(note).map_err(|e| AppError::internal(e.to_string()))?;
    atomic_write(&path, &bytes).map_err(io)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// One of its files is the server's version.
    Current,
    /// It is there, but not in the server's version.
    Outdated,
    Missing,
}

/// An item of the whitelist and the build's files that are it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub item: Item,
    /// Relative to the build's folder, as spelled on disk.
    pub files: Vec<String>,
    pub state: State,
    /// The file that is the server's version, when `Current`.
    pub current: Option<String>,
    /// Every file of it is turned off (`.disabled`).
    pub disabled: bool,
    /// Its files were known by the server's name pattern alone (an older version, put there by
    /// hand): the player confirms replacing them.
    pub by_name_only: bool,
    /// The file the launcher's note names (it put it there).
    pub noted: Option<String>,
    /// Its files known by the server's name pattern alone: perhaps another mod, so a sync never
    /// replaces or removes them.
    pub by_name: Vec<String>,
    /// The server's name pattern of its file (`own_glob`), when it has one.
    pub pattern: Option<PreserveRule>,
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn is_disabled(relative: &str) -> bool {
    relative.to_lowercase().ends_with(DISABLED)
}

/// `relative` without its `.disabled`.
fn enabled_name(relative: &str) -> &str {
    if is_disabled(relative) { &relative[..relative.len() - DISABLED.len()] } else { relative }
}

/// The jars of `mods/`, on or off, with their sizes, sorted.
fn candidates(root: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = fs::read_dir(root.join(MODS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            enabled_name(&name)
                .to_lowercase()
                .ends_with(".jar")
                .then(|| (format!("{MODS}/{name}"), meta.len()))
        })
        .collect();
    found.sort();
    found
}

/// The one glob of `rules` that matches the item's own file, when it names something (a glob
/// such as `mods/*.jar` would take every mod for this one).
fn own_glob<'a>(item: &Item, rules: &'a [PreserveRule]) -> Option<&'a PreserveRule> {
    let matching: Vec<&PreserveRule> =
        rules.iter().filter(|rule| preserved(&item.relative, std::slice::from_ref(*rule))).collect();
    let [rule] = matching.as_slice() else { return None };
    let name = rule.path.rsplit('/').next().unwrap_or_default();
    let named = name.split(['*', '?']).next().is_some_and(|prefix| prefix.chars().count() >= 3);
    (rule.kind == PreserveKind::Glob && named).then_some(*rule)
}

/// Which files of `root`'s `mods/` are each of `items`, and whether each is the server's version
/// (`hash` gives a file's sha256; it is asked only for files of an item's size).
pub fn recognize(
    root: &Path,
    items: &[Item],
    rules: &[PreserveRule],
    note: &Note,
    hash: &mut dyn FnMut(&str) -> Option<String>,
) -> Vec<Found> {
    let files = candidates(root);
    let mut hashes: HashMap<usize, Option<String>> = HashMap::new();
    let mut hash_of = |at: usize| hashes.entry(at).or_insert_with(|| hash(&files[at].0)).clone();
    let sized = |item: &Item, size: u64| item.size.is_none_or(|s| s == size);
    let mut owner: HashMap<usize, usize> = HashMap::new();
    let mut named: Vec<bool> = vec![false; items.len()];
    let mut by_note: HashMap<usize, usize> = HashMap::new();
    // The note (a noted file turned off or on since is still it), then the hashes, then the
    // server's name patterns.
    for (i, item) in items.iter().enumerate() {
        let Some(noted) = note.get(&item.id) else { continue };
        let key = name_key(enabled_name(&format!("{MODS}/{}", noted.file)));
        if let Some(at) = files.iter().position(|(relative, _)| name_key(enabled_name(relative)) == key)
            && !owner.contains_key(&at)
        {
            owner.insert(at, i);
            by_note.insert(i, at);
            named[i] = true;
        }
    }
    for (at, &(_, size)) in files.iter().enumerate() {
        if owner.contains_key(&at) || !items.iter().any(|item| sized(item, size)) {
            continue;
        }
        let Some(sha) = hash_of(at) else { continue };
        if let Some(i) = items.iter().position(|item| sized(item, size) && item.sha256 == sha) {
            owner.insert(at, i);
            named[i] = true;
        }
    }
    // A file two items' patterns take is neither's.
    let patterns: Vec<Option<&PreserveRule>> = items.iter().map(|item| own_glob(item, rules)).collect();
    let takers = |relative: &str| -> Vec<usize> {
        patterns
            .iter()
            .enumerate()
            .filter(|(_, rule)| {
                rule.is_some_and(|rule| preserved(enabled_name(relative), std::slice::from_ref(rule)))
            })
            .map(|(i, _)| i)
            .collect()
    };
    // A mod the server renumbered: the file the launcher noted, unchanged since, is the item whose
    // pattern alone takes it.
    for noted in note.iter().filter(|(id, _)| !items.iter().any(|item| &item.id == *id)).map(|(_, n)| n) {
        let key = name_key(enabled_name(&format!("{MODS}/{}", noted.file)));
        let Some(at) = files.iter().position(|(relative, _)| name_key(enabled_name(relative)) == key) else {
            continue;
        };
        if owner.contains_key(&at) || noted.sha256.is_empty() || hash_of(at).as_deref() != Some(&noted.sha256)
        {
            continue;
        }
        if let [i] = takers(&files[at].0).as_slice()
            && !by_note.contains_key(i)
        {
            owner.insert(at, *i);
            by_note.insert(*i, at);
            named[*i] = true;
        }
    }
    let mut by_pattern: Vec<usize> = Vec::new();
    for (at, (relative, _)) in files.iter().enumerate() {
        if owner.contains_key(&at) {
            continue;
        }
        if let [i] = takers(relative).as_slice() {
            owner.insert(at, *i);
            by_pattern.push(at);
        }
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mine: Vec<usize> = (0..files.len()).filter(|at| owner.get(at) == Some(&i)).collect();
            let current = mine
                .iter()
                .copied()
                .find(|&at| sized(item, files[at].1) && hash_of(at).as_deref() == Some(item.sha256.as_str()))
                .map(|at| files[at].0.clone());
            let state = match (&current, mine.is_empty()) {
                (_, true) => State::Missing,
                (Some(_), false) => State::Current,
                (None, false) => State::Outdated,
            };
            Found {
                item: item.clone(),
                files: mine.iter().map(|&at| files[at].0.clone()).collect(),
                state,
                current,
                disabled: !mine.is_empty() && mine.iter().all(|&at| is_disabled(&files[at].0)),
                by_name_only: !mine.is_empty() && !named[i],
                noted: by_note.get(&i).map(|&at| files[at].0.clone()),
                by_name: mine
                    .iter()
                    .filter(|at| by_pattern.contains(at))
                    .map(|&at| files[at].0.clone())
                    .collect(),
                pattern: patterns[i].cloned(),
            }
        })
        .collect()
}

/// A file's sha256: the noted one while its size and time stay, else read now.
pub fn hasher<'a>(root: &'a Path, note: &'a Note) -> impl FnMut(&str) -> Option<String> + 'a {
    move |relative: &str| {
        let path = root.join(relative);
        let meta = fs::metadata(&path).ok()?;
        let (size, modified) = (meta.len(), modified_ms(&meta));
        let file = relative.strip_prefix("mods/").unwrap_or(relative);
        let noted = note.values().find(|n| {
            n.file == file
                && n.size == size
                && n.modified_ms == modified
                && modified != 0
                && !n.sha256.is_empty()
        });
        match noted {
            Some(noted) => Some(noted.sha256.clone()),
            None => hash_file(&path, HashKind::Sha256).ok(),
        }
    }
}

/// What a server sync does for one optional mod.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionalItem {
    pub id: String,
    /// Its files known by the note or the hash: the sync may replace or remove them.
    pub named: Vec<String>,
    /// Its files known by the server's name pattern alone: kept as they are.
    pub by_name: Vec<String>,
    /// The server's name pattern of its file: a file of the server's own list it takes is this mod.
    pub pattern: Option<PreserveRule>,
    pub keep: Vec<String>,
    pub download: Option<PlannedFile>,
    pub replaced: Vec<String>,
}

/// What a server sync does for the optional mods (`items`), or, when the whitelist could not be
/// read, the files it keeps (`keep`); and the note it leaves (`None`: as it is).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Optional {
    pub items: Vec<OptionalItem>,
    pub keep: Vec<String>,
    pub note: Option<Note>,
}

/// The sync's part for the optional mods of `found` (`None`: the whitelist could not be read, so
/// what the note names stays and nothing changes). Only files known by the note or the hash are
/// replaced or removed; one found by the server's name pattern alone may be another mod.
pub fn for_sync(root: &Path, found: Option<&[Found]>, note: &Note) -> Optional {
    let Some(found) = found else {
        let keep = note
            .values()
            .map(|noted| format!("{MODS}/{}", noted.file))
            .filter(|relative| root.join(relative).is_file())
            .collect();
        return Optional { keep, ..Optional::default() };
    };
    let mut out = Optional::default();
    let mut next = note_after(found, root);
    for f in found {
        let named: Vec<String> = f.files.iter().filter(|file| !f.by_name.contains(file)).cloned().collect();
        let mut item = OptionalItem {
            id: f.item.id.clone(),
            named: named.clone(),
            by_name: f.by_name.clone(),
            pattern: f.pattern.clone(),
            keep: f.by_name.clone(),
            ..OptionalItem::default()
        };
        match f.state {
            State::Missing => {}
            State::Current => {
                // The server's version stays; another known copy of it would load the mod twice.
                for file in &named {
                    if Some(file) == f.current.as_ref() {
                        item.keep.push(file.clone());
                    } else {
                        item.replaced.push(file.clone());
                    }
                }
            }
            State::Outdated if named.is_empty() => {}
            State::Outdated => {
                let disabled = named.iter().all(|file| is_disabled(file));
                let dest =
                    if disabled { format!("{}{DISABLED}", f.item.relative) } else { f.item.relative.clone() };
                item.replaced =
                    named.iter().filter(|file| name_key(file) != name_key(&dest)).cloned().collect();
                item.download = Some(PlannedFile {
                    relative: dest.clone(),
                    url: f.item.url.clone(),
                    size: f.item.size,
                    hash: Some(ExpectedHash { kind: HashKind::Sha256, hex: f.item.sha256.clone() }),
                });
                let file = dest.strip_prefix("mods/").unwrap_or(&dest).to_string();
                let noted = Noted {
                    file,
                    sha256: f.item.sha256.clone(),
                    size: f.item.size.unwrap_or(0),
                    modified_ms: 0,
                };
                next.insert(f.item.id.clone(), noted);
            }
        }
        out.items.push(item);
    }
    out.note = Some(next);
    out
}

/// The note of `found`'s items that are the server's version, with their files' size and time
/// now: those the launcher put in and those put in by hand as the very same file, so a later
/// version replaces either.
pub fn note_after(found: &[Found], root: &Path) -> Note {
    found
        .iter()
        .filter_map(|f| {
            let relative = f.current.as_ref()?;
            let meta = fs::metadata(root.join(relative)).ok()?;
            let file = relative.strip_prefix("mods/").unwrap_or(relative).to_string();
            let noted = Noted {
                file,
                sha256: f.item.sha256.clone(),
                size: meta.len(),
                modified_ms: modified_ms(&meta),
            };
            Some((f.item.id.clone(), noted))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;

    use launcher_shared::ContentKind;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::backend::manifest::{PreserveKind, PreserveRule};
    use crate::backend::whitelist::Item;

    fn sha(body: &str) -> String {
        Sha256::digest(body.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn item(id: &str, name: &str, body: &str) -> Item {
        Item {
            id: id.into(),
            title: id.to_uppercase(),
            kind: ContentKind::Mods,
            name: name.into(),
            relative: format!("mods/{name}"),
            url: format!("https://cdn.example/{name}"),
            size: Some(body.len() as u64),
            sha256: sha(body),
            page: None,
        }
    }

    fn glob(path: &str) -> PreserveRule {
        PreserveRule { kind: PreserveKind::Glob, path: path.into() }
    }

    fn put(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn real_hash(root: &Path) -> impl FnMut(&str) -> Option<String> + '_ {
        move |relative: &str| fs::read_to_string(root.join(relative)).ok().map(|b| sha(&b))
    }

    fn goggles() -> Item {
        item("goggles", "CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar", "goggles 8.6.9")
    }

    #[test]
    fn a_noted_file_is_its_item_whatever_its_name() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/renamed.jar", "goggles 8.6.9");
        let mut note = Note::new();
        note.insert("goggles".into(), Noted { file: "renamed.jar".into(), ..Noted::default() });
        let found = recognize(dir.path(), &[goggles()], &[], &note, &mut real_hash(dir.path()));
        assert_eq!(found[0].files, ["mods/renamed.jar"]);
        assert_eq!((found[0].state, found[0].by_name_only), (State::Current, false));
    }

    #[test]
    fn a_mod_the_player_deleted_stays_deleted_at_the_next_sync() {
        let dir = tempfile::tempdir().unwrap();
        // Noted at an older version, its file since deleted from the Mods tab; the server now
        // has a newer one.
        let mut note = Note::new();
        let old = Noted { file: "CreateCyberGoggles-1.21.1-8.6.8-NeoForge.jar".into(), ..Noted::default() };
        note.insert("goggles".into(), old);
        let rules = [glob("mods/CreateCyberGoggles-*.jar")];
        let found = recognize(dir.path(), &[goggles()], &rules, &note, &mut real_hash(dir.path()));
        assert_eq!(found[0].state, State::Missing);
        let sync = for_sync(dir.path(), Some(&found), &note);
        assert!(sync.items.iter().all(|i| i.download.is_none()), "nothing is put back: {:?}", sync.items);
        assert_eq!(sync.note, Some(Note::new()), "and the note forgets it");
    }

    #[test]
    fn a_file_with_the_items_hash_is_current() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/goggles-any-name.jar", "goggles 8.6.9");
        let found = recognize(dir.path(), &[goggles()], &[], &Note::new(), &mut real_hash(dir.path()));
        assert_eq!(
            (found[0].files.clone(), found[0].state),
            (vec!["mods/goggles-any-name.jar".into()], State::Current)
        );
    }

    #[test]
    fn an_older_version_is_found_by_the_servers_glob_and_is_outdated() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/CreateCyberGoggles-1.21.1-8.6.8-NeoForge.jar", "goggles 8.6.8!");
        put(dir.path(), "mods/other.jar", "other");
        let rules = [glob("mods/CreateCyberGoggles-*.jar")];
        let found = recognize(dir.path(), &[goggles()], &rules, &Note::new(), &mut real_hash(dir.path()));
        assert_eq!(found[0].files, ["mods/CreateCyberGoggles-1.21.1-8.6.8-NeoForge.jar"]);
        assert_eq!((found[0].state, found[0].by_name_only), (State::Outdated, true));
    }

    #[test]
    fn a_file_two_items_globs_match_goes_to_neither() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/alpha-bet.jar", "which one");
        let alpha = item("alpha", "alpha-2.jar", "alpha 2");
        let alpbet = item("alpbet", "alpbet.jar", "alpbet 2");
        let rules = [glob("mods/alpha-*.jar"), glob("mods/alp*bet.jar")];
        let found = recognize(dir.path(), &[alpha, alpbet], &rules, &Note::new(), &mut real_hash(dir.path()));
        assert!(
            found.iter().all(|f| f.files.is_empty()),
            "both patterns take it: neither item does {found:?}"
        );
    }

    #[test]
    fn a_noted_file_turned_off_is_still_its_item() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/goggles-1.jar.disabled", "goggles 1");
        let mut note = Note::new();
        note.insert("goggles".into(), Noted { file: "goggles-1.jar".into(), ..Noted::default() });
        let found = recognize(dir.path(), &[goggles()], &[], &note, &mut real_hash(dir.path()));
        assert_eq!(found[0].files, ["mods/goggles-1.jar.disabled"]);
        assert_eq!(
            (found[0].state, found[0].disabled, found[0].by_name_only),
            (State::Outdated, true, false)
        );
        assert_eq!(found[0].noted.as_deref(), Some("mods/goggles-1.jar.disabled"));
    }

    #[test]
    fn a_glob_that_names_nothing_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/sodium.jar", "sodium");
        let rules = [glob("mods/*.jar")];
        let found = recognize(dir.path(), &[goggles()], &rules, &Note::new(), &mut real_hash(dir.path()));
        assert_eq!(found[0].state, State::Missing, "every mod is not the goggles");
    }

    #[test]
    fn an_item_two_globs_match_is_not_found_by_name() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/CreateCyberGoggles-old.jar", "old");
        let rules = [glob("mods/CreateCyberGoggles-*.jar"), glob("mods/Create*.jar")];
        let found = recognize(dir.path(), &[goggles()], &rules, &Note::new(), &mut real_hash(dir.path()));
        assert_eq!(found[0].state, State::Missing);
    }

    #[test]
    fn a_disabled_copy_is_installed_and_stays_disabled() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar.disabled", "goggles 8.6.9");
        let found = recognize(dir.path(), &[goggles()], &[], &Note::new(), &mut real_hash(dir.path()));
        assert_eq!((found[0].state, found[0].disabled), (State::Current, true));
        let old = tempfile::tempdir().unwrap();
        put(old.path(), "mods/CreateCyberGoggles-old.jar.disabled", "old");
        let rules = [glob("mods/CreateCyberGoggles-*.jar")];
        let found = recognize(old.path(), &[goggles()], &rules, &Note::new(), &mut real_hash(old.path()));
        assert_eq!(
            (found[0].state, found[0].disabled),
            (State::Outdated, true),
            "a glob sees through .disabled"
        );
    }

    #[test]
    fn a_file_two_items_could_claim_goes_to_the_one_that_named_it() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/connector-1.jar", "connector 1");
        let connector = item("connector", "connector-2.jar", "connector 2");
        let extras = item("extras", "connector-extras-2.jar", "connector 1");
        let rules = [glob("mods/connector-*.jar")];
        let found =
            recognize(dir.path(), &[connector, extras], &rules, &Note::new(), &mut real_hash(dir.path()));
        assert_eq!(
            found[0].state,
            State::Missing,
            "the glob is the connector's, but the hash is the extras'"
        );
        assert_eq!(
            (found[1].files.clone(), found[1].state),
            (vec!["mods/connector-1.jar".into()], State::Current)
        );
    }

    #[test]
    fn files_of_another_size_are_never_hashed() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..20 {
            put(dir.path(), &format!("mods/unrelated-{n}.jar"), &"x".repeat(100 + n));
        }
        let calls = Cell::new(0);
        let mut hash = |_: &str| {
            calls.set(calls.get() + 1);
            None
        };
        let found = recognize(dir.path(), &[goggles()], &[], &Note::new(), &mut hash);
        assert_eq!((found[0].state, calls.get()), (State::Missing, 0));
    }

    #[test]
    fn nothing_found_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let found = recognize(dir.path(), &[goggles()], &[], &Note::new(), &mut real_hash(dir.path()));
        assert_eq!((found[0].files.len(), found[0].state, found[0].disabled), (0, State::Missing, false));
    }

    #[cfg(windows)]
    #[test]
    fn a_name_in_another_case_is_still_the_item() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "mods/createcybergoggles-1.21.1-8.6.9-neoforge.jar", "goggles 8.6.9");
        let mut note = Note::new();
        note.insert(
            "goggles".into(),
            Noted { file: "CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar".into(), ..Noted::default() },
        );
        let found = recognize(dir.path(), &[goggles()], &[], &note, &mut |_| None);
        assert_eq!(found[0].files, ["mods/createcybergoggles-1.21.1-8.6.9-neoforge.jar"]);
    }

    #[test]
    fn the_note_round_trips_and_holds_the_current_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_note(dir.path()).is_empty());
        put(dir.path(), "mods/CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar", "goggles 8.6.9");
        let old = item("old", "old-2.jar", "old 2");
        put(dir.path(), "mods/old-1.jar", "old 1");
        let barrel = item("barrel", "barrel-1.jar", "barrel");
        put(dir.path(), "mods/barrel-1.jar", "barrel");
        let rules = [glob("mods/old-*.jar")];
        let mut noted = Note::new();
        for (id, file) in [("goggles", "CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar"), ("old", "old-1.jar")]
        {
            noted.insert(id.into(), Noted { file: file.into(), ..Noted::default() });
        }
        let found =
            recognize(dir.path(), &[goggles(), old, barrel], &rules, &noted, &mut real_hash(dir.path()));
        let note = note_after(&found, dir.path());
        assert_eq!(
            note.keys().collect::<Vec<_>>(),
            ["barrel", "goggles"],
            "what is the server's version, put in by the launcher or as the very same file by another: not the old one"
        );
        assert_eq!(note["goggles"].sha256, sha("goggles 8.6.9"));
        assert_eq!(note["goggles"].size, 13);
        write_note(dir.path(), &note).unwrap();
        assert_eq!(read_note(dir.path()), note);
        assert!(dir.path().join(NOTE).is_file());
        let mut hash = hasher(dir.path(), &note);
        assert_eq!(hash("mods/CreateCyberGoggles-1.21.1-8.6.9-NeoForge.jar"), Some(sha("goggles 8.6.9")));
    }
}
