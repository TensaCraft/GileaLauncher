//! Which builds are the server's: a managed build follows the server at every
//! launch; its catalog id is kept apart from the component it runs.

use std::path::Path;

use launcher_core::storage::journal::SyncJournal;
use launcher_core::storage::versions::Build;
use serde_json::{Value, json};

/// The client a server build names (the original launcher's, so its builds are recognised).
pub const CLIENT: &str = "TensaCraft";
/// The journal of a server build's file changes.
pub const JOURNAL: &str = ".launcher-tensa-sync.json";
const PACK_OPTION_KEYS: [&str; 2] = ["tensacraftPackId", "tensacraft_pack_id"];

/// A string or a number, trimmed; nothing when blank.
fn text(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// The catalog id of the server build `build` follows: its remote id, else the id in its
/// options, else its own id.
pub fn pack_id(build: &Build) -> Option<String> {
    text(build.remote_pack_id.as_ref())
        .or_else(|| PACK_OPTION_KEYS.iter().find_map(|key| text(build.options.get(*key))))
        .or_else(|| Some(build.id.trim().to_string()).filter(|id| !id.is_empty()))
}

/// `build` follows the server: not when it opted out (`managedByApi: false`, a copy set to
/// `syncMode: manual`); yes when it names the server's client, a remote id or a pack id, or when
/// its last file change (in `game_dir`) was the server's.
pub fn is_managed(build: &Build, game_dir: &Path) -> bool {
    if build.options.get("managedByApi") == Some(&Value::Bool(false)) {
        return false;
    }
    let manual = build.options.get("syncMode").and_then(Value::as_str);
    if manual.is_some_and(|mode| mode.trim().eq_ignore_ascii_case("manual")) {
        return false;
    }
    if build.client.as_deref().is_some_and(|client| client.to_lowercase().contains("tensa"))
        || text(build.remote_pack_id.as_ref()).is_some()
        || PACK_OPTION_KEYS.iter().any(|key| text(build.options.get(*key)).is_some())
    {
        return true;
    }
    let journal = SyncJournal::new(game_dir, JOURNAL).read();
    let operation = journal.as_ref().and_then(|j| j.get("operation")).and_then(Value::as_str);
    operation.is_some_and(|op| op.trim().to_lowercase().starts_with("tensacraft_"))
}

/// Names `build` a server build following `pack_id`; whether anything changed.
pub fn mark(build: &mut Build, pack_id: &str) -> bool {
    let id = pack_id.trim();
    let mut changed = false;
    if !build.client.as_deref().is_some_and(|client| client.trim().eq_ignore_ascii_case(CLIENT)) {
        build.client = Some(CLIENT.to_string());
        changed = true;
    }
    if !id.is_empty() && build.remote_pack_id != Some(json!(id)) {
        build.remote_pack_id = Some(json!(id));
        changed = true;
    }
    if build.options.get("managedByApi") != Some(&json!(true)) {
        build.options.insert("managedByApi".into(), json!(true));
        changed = true;
    }
    if !id.is_empty() && build.options.get("tensacraftPackId") != Some(&json!(id)) {
        build.options.insert("tensacraftPackId".into(), json!(id));
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Build {
        Build::new("Aero")
    }

    #[test]
    fn the_pack_id_comes_from_the_remote_id_options_or_id() {
        let mut b = build();
        b.remote_pack_id = Some(json!(" aero "));
        b.options.insert("tensacraftPackId".into(), json!("other"));
        assert_eq!(pack_id(&b).as_deref(), Some("aero"));
        b.remote_pack_id = Some(json!(7));
        assert_eq!(pack_id(&b).as_deref(), Some("7"));
        b.remote_pack_id = None;
        assert_eq!(pack_id(&b).as_deref(), Some("other"));
        b.options.remove("tensacraftPackId");
        b.options.insert("tensacraft_pack_id".into(), json!("snake"));
        assert_eq!(pack_id(&b).as_deref(), Some("snake"));
        b.options.remove("tensacraft_pack_id");
        assert_eq!(pack_id(&b), Some(b.id.clone()), "its own id at last");
    }

    #[test]
    fn managed_builds_are_told_by_client_id_option_or_journal() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = build();
        b.client = Some("Fabric".into());
        assert!(!is_managed(&b, dir.path()));
        b.client = Some("tensacraft".into());
        assert!(is_managed(&b, dir.path()));
        b.client = Some("Fabric".into());
        b.remote_pack_id = Some(json!("aero"));
        assert!(is_managed(&b, dir.path()));
        b.remote_pack_id = None;
        b.options.insert("tensacraftPackId".into(), json!("aero"));
        assert!(is_managed(&b, dir.path()));
        b.options.remove("tensacraftPackId");
        std::fs::write(dir.path().join(JOURNAL), r#"{"status":"complete","operation":"tensacraft_sync"}"#)
            .unwrap();
        assert!(is_managed(&b, dir.path()), "its last file change was the server's");
        std::fs::write(
            dir.path().join(JOURNAL),
            r#"{"status":"complete","operation":"modrinth-content-install"}"#,
        )
        .unwrap();
        assert!(!is_managed(&b, dir.path()));
    }

    #[test]
    fn a_copy_set_to_manual_is_not_managed() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = build();
        b.client = Some(CLIENT.into());
        b.options.insert("syncMode".into(), json!(" Manual "));
        assert!(!is_managed(&b, dir.path()));
        b.options.remove("syncMode");
        b.options.insert("managedByApi".into(), json!(false));
        assert!(!is_managed(&b, dir.path()));
    }

    #[test]
    fn mark_names_the_server_build() {
        let mut b = build();
        b.client = Some("Fabric".into());
        assert!(mark(&mut b, " aero "));
        assert_eq!(b.client.as_deref(), Some(CLIENT));
        assert_eq!(b.remote_pack_id, Some(json!("aero")));
        assert_eq!(b.options.get("managedByApi"), Some(&json!(true)));
        assert_eq!(b.options.get("tensacraftPackId"), Some(&json!("aero")));
        assert!(!mark(&mut b, "aero"), "nothing left to change");
        b.client = Some("TENSACRAFT".into());
        assert!(!mark(&mut b, ""), "any case of the client is the client; no id keeps the old one");
        assert_eq!(b.remote_pack_id, Some(json!("aero")));
    }

    #[test]
    fn a_blank_pack_id_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = build();
        b.options.insert("tensacraftPackId".into(), json!(""));
        assert!(!is_managed(&b, dir.path()), "a blank pack id is none");
    }
}
