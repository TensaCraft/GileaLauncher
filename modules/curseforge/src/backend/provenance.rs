//! What the launcher installed from CurseForge into a build: one record per file, kept in
//! `.launcher/curseforge-content.json` — only what the installed content needs (ids, names, the
//! file's hash), never a copy of CurseForge's catalogue.

use std::collections::BTreeMap;
use std::path::Path;

use launcher_shared::ContentKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::resolver::{Installed, InstalledFile};

pub const PROVENANCE: &str = ".launcher/curseforge-content.json";
const SCHEMA: u32 = 1;

/// A file installed from CurseForge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub kind: ContentKind,
    pub project_id: u64,
    pub file_id: u64,
    pub title: String,
    /// The file's name on CurseForge (its display name).
    pub version: String,
    pub filename: String,
    /// The project's page.
    #[serde(default)]
    pub page: Option<String>,
    pub sha1: String,
    /// When CurseForge published the file.
    #[serde(default)]
    pub date: Option<String>,
    /// CurseForge's release type of the file (1 release, 2 beta, 3 alpha).
    #[serde(default)]
    pub release_type: Option<u64>,
}

/// The build's records by the file's place (`mods/x.jar`, as installed: enabled).
pub fn read(game: &Path) -> BTreeMap<String, Record> {
    std::fs::read(game.join(PROVENANCE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|doc| serde_json::from_value(doc["files"].clone()).ok())
        .unwrap_or_default()
}

/// The records as their file.
pub fn document(records: &BTreeMap<String, Record>) -> Value {
    json!({ "schema_version": SCHEMA, "files": records })
}

/// Where the recorded file lies now: its place, or the place switched off (`.disabled`).
pub fn on_disk(game: &Path, relative: &str) -> Option<String> {
    let disabled = format!("{relative}.disabled");
    [relative.to_string(), disabled].into_iter().find(|r| game.join(r).is_file())
}

/// The CurseForge projects of `kind` the build still has.
pub fn installed(game: &Path, kind: ContentKind, records: &BTreeMap<String, Record>) -> Installed {
    let mut found = Installed::default();
    for (relative, record) in records.iter().filter(|(_, r)| r.kind == kind) {
        if let Some(now) = on_disk(game, relative) {
            found.by_project.insert(
                record.project_id,
                InstalledFile {
                    file_id: record.file_id,
                    filename: record.filename.clone(),
                    version: record.version.clone(),
                    relative: now,
                    date: record.date.clone(),
                    release_type: record.release_type,
                },
            );
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(project_id: u64, filename: &str) -> Record {
        Record {
            kind: ContentKind::Mods,
            project_id,
            file_id: project_id * 10,
            title: format!("P{project_id}"),
            version: filename.into(),
            filename: filename.into(),
            page: None,
            sha1: "a".repeat(40),
            date: None,
            release_type: None,
        }
    }

    #[test]
    fn records_name_the_files_the_build_still_has() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("mods")).unwrap();
        std::fs::write(tmp.path().join("mods/a.jar"), b"a").unwrap();
        std::fs::write(tmp.path().join("mods/b.jar.disabled"), b"b").unwrap();
        let records: BTreeMap<String, Record> = [("a", 1), ("b", 2), ("gone", 3)]
            .into_iter()
            .map(|(name, id)| (format!("mods/{name}.jar"), record(id, &format!("{name}.jar"))))
            .collect();
        std::fs::create_dir_all(tmp.path().join(".launcher")).unwrap();
        std::fs::write(tmp.path().join(PROVENANCE), document(&records).to_string()).unwrap();
        let read_back = read(tmp.path());
        assert_eq!(read_back, records);
        let found = installed(tmp.path(), ContentKind::Mods, &read_back);
        let mut ids: Vec<u64> = found.by_project.keys().copied().collect();
        ids.sort();
        assert_eq!(ids, [1, 2], "a switched-off file is still there; a removed one is not");
        assert!(installed(tmp.path(), ContentKind::ShaderPacks, &read_back).by_project.is_empty());
        assert!(read(&tmp.path().join("nowhere")).is_empty());
    }
}
