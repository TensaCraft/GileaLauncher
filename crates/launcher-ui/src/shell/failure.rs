//! A failed operation's toast: «Report» on it when reports can go and the error is the launcher's
//! to fix (not the user's network, account or computer).

use launcher_shared::{AppError, ErrorCode, Level};
use leptos::prelude::*;
use ui_kit::Toasts;
use ui_kit::i18n::I18nCtx;
use ui_kit::problem::{ProblemDraft, current_reporter, report_action};

/// The problem a report of `error`, told as `text`, is about.
pub fn draft_of(text: &str, error: &AppError) -> ProblemDraft {
    ProblemDraft {
        title: text.to_string(),
        code: serde_json::to_value(error.code).ok().and_then(|v| v.as_str().map(str::to_string)),
        detail: Some(error.detail.clone()).filter(|d| !d.trim().is_empty()),
    }
}

/// Whether a failure with `code` is offered to report while reports `available`.
pub fn offers_report(available: bool, code: ErrorCode) -> bool {
    available && code.reportable()
}

/// Shows `text`, the failure of an operation with `error`; with «Report» when that fits.
pub fn failure_toast(toasts: Toasts, i18n: I18nCtx, text: String, error: &AppError) {
    let available = current_reporter().is_some_and(|r| r.available.get_untracked());
    let action = offers_report(available, error.code)
        .then(|| (i18n.t("report_problem_action"), report_action(&draft_of(&text, error))));
    toasts.show_with_action(Level::Error, text, None, action);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_s_report_carries_its_text_code_and_detail() {
        let error = AppError::new(ErrorCode::DownloadFailed, "HTTP 503");
        let draft = draft_of("Не вдалося встановити Aero", &error);
        assert_eq!(draft.code.as_deref(), Some("download_failed"));
        assert_eq!(draft.detail.as_deref(), Some("HTTP 503"));
        assert_eq!(draft_of("x", &AppError::new(ErrorCode::Internal, " ")).detail, None);
    }

    #[test]
    fn report_is_offered_only_for_the_launcher_s_own_failures_while_reports_can_go() {
        assert!(offers_report(true, ErrorCode::DownloadFailed));
        assert!(!offers_report(true, ErrorCode::Network));
        assert!(!offers_report(false, ErrorCode::DownloadFailed), "nowhere to send it");
    }
}
