//! What the launcher makes of a crashed game: findings with their certainty,
//! the log lines that show them and what can be done about them.

use serde::{Deserialize, Serialize};

use launcher_shared::Text;

/// The event a crash's diagnosis comes in (payload: [`DiagnosisEvent`]).
pub const DIAGNOSIS_EVENT: &str = "app://module/diagnostics/diagnosis";
/// The module's command answering [`BuildArgs`] with the build's [`BuildDiagnostics`].
pub const BUILD_DIAGNOSTICS: &str = "build_diagnostics";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildArgs {
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low = 1,
    Medium = 2,
    High = 3,
    Exact = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

/// What a finding offers to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixKind {
    OpenModManager,
    Repair,
    RepairSync,
    OpenDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    Safe,
    Confirm,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixAction {
    pub id: String,
    pub kind: FixKind,
    pub safety: Safety,
    pub title: Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub kind: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub priority: u32,
    pub title: Text,
    pub message: Text,
    /// Up to 4 lines of the log that show it, 300 characters each.
    pub evidence: Vec<String>,
    pub actions: Vec<FixAction>,
    /// Ids of findings this one explains away; an id also covers every id under it (`a.b` covers
    /// `a.b.c`).
    pub suppresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub engine_version: u32,
    /// Unix seconds.
    pub created_at: i64,
    /// The first is the main one.
    pub findings: Vec<Finding>,
    /// The files read.
    pub files: Vec<String>,
}

/// A build's diagnostics tab: where its files are, and its last crash's diagnosis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDiagnostics {
    pub game_dir: String,
    pub logs_dir: String,
    pub crash_reports_dir: String,
    pub latest_log: String,
    /// The newest crash report, if there is one.
    pub latest_crash: Option<String>,
    pub launch_log: String,
    pub last: Option<Diagnosis>,
}

/// A crashed game's diagnosis (`app://diagnosis`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisEvent {
    pub build_key: String,
    pub build_name: String,
    pub diagnosis: Diagnosis,
}
