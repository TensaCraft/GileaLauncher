//! A problem with the launcher the user may tell its developers about. A failed operation's toast
//! offers it (`report_action`); a module that can send reports answers (`ProblemReporter`).

use std::cell::Cell;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// What the user saw of the problem: the failure's text and, for an error, its code and detail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemDraft {
    pub title: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Whether a module can send reports now, and the problem the user asked to report.
#[derive(Clone, Copy)]
pub struct ProblemReporter {
    pub available: RwSignal<bool>,
    pub request: RwSignal<Option<ProblemDraft>>,
}

thread_local! {
    /// The app's reporter, for code that runs after an await (no reactive owner there).
    static CURRENT: Cell<Option<ProblemReporter>> = const { Cell::new(None) };
}

pub fn provide_problem_reporter() -> ProblemReporter {
    let reporter = ProblemReporter { available: RwSignal::new(false), request: RwSignal::new(None) };
    provide_context(reporter);
    CURRENT.with(|current| current.set(Some(reporter)));
    reporter
}

/// The app's reporter, when it has one.
pub fn current_reporter() -> Option<ProblemReporter> {
    CURRENT.with(Cell::get)
}

pub fn use_problem_reporter() -> ProblemReporter {
    expect_context::<ProblemReporter>()
}

const REPORT_PREFIX: &str = "report:";

/// The toast action that asks to report `draft`.
pub fn report_action(draft: &ProblemDraft) -> String {
    format!("{REPORT_PREFIX}{}", serde_json::to_string(draft).unwrap_or_default())
}

/// The problem a toast action asks to report; none for another action.
pub fn reported_problem(action: &str) -> Option<ProblemDraft> {
    action.strip_prefix(REPORT_PREFIX).and_then(|json| serde_json::from_str(json).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_action_carries_the_problem_to_report() {
        let draft = ProblemDraft {
            title: "Не вдалося встановити: Aero".into(),
            code: Some("download_failed".into()),
            detail: Some("HTTP 503 from piston-data.mojang.com".into()),
        };
        assert_eq!(reported_problem(&report_action(&draft)), Some(draft));
        assert_eq!(reported_problem("route:/builds"), None, "another action");
        assert_eq!(reported_problem("report:{"), None);
    }
}
