//! Why a game crashed: its fresh logs, read within bounds, are weighed by
//! detectors; the most certain finding leads, findings another one explains away go, and when
//! nothing is recognised the diagnosis says so.

pub mod artifacts;
pub mod dependencies;
mod loader;
mod module;
pub mod rules;
mod watch;

pub use module::{build_diagnostics, module};
pub use watch::CrashWatcher;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::dto::{Confidence, Diagnosis, Finding, FixAction, FixKind, Safety, Severity};
use launcher_shared::Text;

pub const ENGINE_VERSION: u32 = 1;
/// Where a build's last diagnosis is kept, relative to its game folder.
pub const DIAGNOSIS_FILE: &str = ".launcher/diagnosis.json";

/// The diagnosis of the build's last crash, if one was kept.
pub fn last(game_dir: &std::path::Path) -> Option<Diagnosis> {
    serde_json::from_slice(&std::fs::read(game_dir.join(DIAGNOSIS_FILE)).ok()?).ok()
}

/// Where a build's diagnostics are, and its last crash's diagnosis.
pub fn of_build(game_dir: &std::path::Path) -> crate::dto::BuildDiagnostics {
    let text = |p: &std::path::Path| p.to_string_lossy().into_owned();
    let crash_reports = game_dir.join("crash-reports");
    let latest_crash = std::fs::read_dir(&crash_reports)
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        })
        .map(|p| text(&p));
    crate::dto::BuildDiagnostics {
        game_dir: text(game_dir),
        logs_dir: text(&game_dir.join("logs")),
        crash_reports_dir: text(&crash_reports),
        latest_log: text(&game_dir.join("logs").join("latest.log")),
        latest_crash,
        launch_log: text(&game_dir.join("logs").join(launcher_core::launch::process::LAUNCH_LOG)),
        last: last(game_dir),
    }
}

/// Keeps `diagnosis` beside the build, over the one before.
pub fn keep(game_dir: &std::path::Path, diagnosis: &Diagnosis) -> std::io::Result<()> {
    let path = game_dir.join(DIAGNOSIS_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let value = serde_json::to_value(diagnosis).map_err(std::io::Error::other)?;
    launcher_core::storage::json::write_json_file(&path, &value, 2)
}

/// What the detectors read: the logs joined (`raw`) and in lower case (`text`), the files they
/// came from, and whether the build is a server's (whose files the launcher syncs).
#[derive(Debug, Clone, Default)]
pub struct Case {
    pub text: String,
    pub raw: String,
    pub files: Vec<PathBuf>,
    pub managed: bool,
}

/// One way of recognising a crash.
pub trait Detector: Send + Sync {
    fn detect(&self, case: &Case) -> Vec<Finding>;
}

/// "Open the diagnostics": what every diagnosis can offer.
pub fn open_diagnostics() -> FixAction {
    FixAction {
        id: "open_diagnostics".into(),
        kind: FixKind::OpenDiagnostics,
        safety: Safety::Safe,
        title: Text::key("open_crash_diagnostics"),
    }
}

/// The finding when nothing is recognised.
pub fn unknown() -> Finding {
    Finding {
        id: "launch.unknown".into(),
        kind: "unknown".into(),
        severity: Severity::Warning,
        confidence: Confidence::Low,
        priority: 0,
        title: Text::key("launch_diagnostic_unknown_title"),
        message: Text::key("launch_diagnostic_unknown"),
        evidence: Vec::new(),
        actions: vec![open_diagnostics()],
        suppresses: Vec::new(),
    }
}

fn rank(f: &Finding) -> (Confidence, u32, usize) {
    (f.confidence, f.priority, f.evidence.len())
}

/// `rule` explains `id` away: the same id, or an id under it.
fn covers(rule: &str, id: &str) -> bool {
    id == rule || id.strip_prefix(rule).is_some_and(|rest| rest.starts_with('.'))
}

/// Every detector's findings, one per id (the strongest), less those another finding explains
/// away, the strongest first; nothing left is the unknown finding.
pub fn diagnose(case: &Case, detectors: &[Box<dyn Detector>], now: i64) -> Diagnosis {
    let mut by_id: HashMap<String, Finding> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for finding in detectors.iter().flat_map(|d| d.detect(case)) {
        match by_id.get(&finding.id) {
            Some(kept) if rank(kept) >= rank(&finding) => {}
            Some(_) => {
                by_id.insert(finding.id.clone(), finding);
            }
            None => {
                order.push(finding.id.clone());
                by_id.insert(finding.id.clone(), finding);
            }
        }
    }
    let all: Vec<Finding> = order.into_iter().filter_map(|id| by_id.remove(&id)).collect();
    let suppressed_by = |findings: &[&Finding], id: &str| {
        findings.iter().any(|f| f.id != id && f.suppresses.iter().any(|rule| covers(rule, id)))
    };
    // A finding explained away explains nothing away itself.
    let every: Vec<&Finding> = all.iter().collect();
    let standing: Vec<&Finding> = all.iter().filter(|f| !suppressed_by(&every, &f.id)).collect();
    let mut findings: Vec<Finding> =
        all.iter().filter(|f| !suppressed_by(&standing, &f.id)).cloned().collect();
    if findings.is_empty() {
        findings.push(unknown());
    }
    findings.sort_by_key(|f| std::cmp::Reverse(rank(f)));
    Diagnosis {
        engine_version: ENGINE_VERSION,
        created_at: now,
        findings,
        files: case.files.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use crate::dto::{Confidence, Finding, Severity};
    use launcher_shared::Text;

    use super::*;

    fn finding(id: &str, confidence: Confidence, priority: u32, suppresses: &[&str]) -> Finding {
        Finding {
            id: id.into(),
            kind: "k".into(),
            severity: Severity::Warning,
            confidence,
            priority,
            title: Text::key("t"),
            message: Text::key("m"),
            evidence: vec!["e".into()],
            actions: Vec::new(),
            suppresses: suppresses.iter().map(|s| s.to_string()).collect(),
        }
    }

    struct Gives(Vec<Finding>);

    impl Detector for Gives {
        fn detect(&self, _: &Case) -> Vec<Finding> {
            self.0.clone()
        }
    }

    fn case() -> Case {
        Case { text: String::new(), raw: String::new(), files: Vec::new(), managed: false }
    }

    #[test]
    fn nothing_found_is_the_unknown_finding() {
        let d = diagnose(&case(), &[], 7);
        assert_eq!(d.findings.len(), 1);
        let f = &d.findings[0];
        assert_eq!(
            (f.id.as_str(), f.kind.as_str(), f.confidence, f.priority),
            ("launch.unknown", "unknown", Confidence::Low, 0)
        );
        assert_eq!(
            (f.title.clone(), f.message.clone()),
            (Text::key("launch_diagnostic_unknown_title"), Text::key("launch_diagnostic_unknown"))
        );
        assert_eq!((d.engine_version, d.created_at), (1, 7));
    }

    #[test]
    fn the_most_certain_finding_leads_and_suppressed_ones_go() {
        let detectors: Vec<Box<dyn Detector>> = vec![
            Box::new(Gives(vec![finding("mods.incompatible.generic", Confidence::High, 80, &[])])),
            Box::new(Gives(vec![finding(
                "mods.missing_dependency.fabric_api",
                Confidence::Exact,
                100,
                &["mods.incompatible.generic"],
            )])),
            Box::new(Gives(vec![finding("graphics.initialization", Confidence::High, 60, &[])])),
            Box::new(Gives(vec![finding("files.locked", Confidence::Exact, 110, &[])])),
        ];
        let ids: Vec<String> = diagnose(&case(), &detectors, 0).findings.into_iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["files.locked", "mods.missing_dependency.fabric_api", "graphics.initialization"]
        );
    }

    #[test]
    fn a_prefix_suppresses_every_finding_under_it_and_repeats_keep_the_strongest() {
        let detectors: Vec<Box<dyn Detector>> = vec![
            Box::new(Gives(vec![
                finding("mods.missing_dependency.a", Confidence::Exact, 100, &[]),
                finding("mods.missing_dependency.b", Confidence::Exact, 100, &[]),
            ])),
            Box::new(Gives(vec![finding(
                "mods.create_configuration_payload",
                Confidence::Exact,
                125,
                &["mods.missing_dependency"],
            )])),
            Box::new(Gives(vec![
                finding("x", Confidence::Low, 1, &[]),
                finding("x", Confidence::High, 1, &[]),
            ])),
        ];
        let found = diagnose(&case(), &detectors, 0).findings;
        assert_eq!(
            found.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["mods.create_configuration_payload", "x"]
        );
        assert_eq!(found[1].confidence, Confidence::High);
    }
}
