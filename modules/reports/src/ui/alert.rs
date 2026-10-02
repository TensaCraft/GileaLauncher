//! «Send report» in an alert the user can report: idle → sending → sent, or
//! «Retry sending» after a failure.

use launcher_shared::{Alert, AppError, ErrorCode, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Button, Variant, use_toasts};

use super::{api, state};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportState {
    Idle,
    Sending,
    Sent,
    Failed,
}

/// The button's label key and whether it can be pressed.
pub fn button_of(state: ReportState) -> (&'static str, bool) {
    match state {
        ReportState::Idle => ("send_error_report", true),
        ReportState::Sending => ("error_report_sending", false),
        ReportState::Sent => ("error_report_sent_button", false),
        ReportState::Failed => ("error_report_retry", true),
    }
}

/// Why a report did not go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The server answered and refused: its words.
    Refused(String),
    /// The alert's report is no longer kept.
    Gone,
    /// Anything else (no answer at all).
    Other,
}

pub fn failure_of(error: &AppError) -> Failure {
    match (error.code, error.params.get("reason")) {
        (_, Some(reason)) => Failure::Refused(reason.clone()),
        (ErrorCode::NotFound, None) => Failure::Gone,
        _ => Failure::Other,
    }
}

/// The toast of a report that did not go.
pub fn failure_text(i18n: I18nCtx, error: &AppError) -> String {
    let why = match failure_of(error) {
        Failure::Refused(reason) => reason,
        Failure::Gone => i18n.t("reports_alert_gone"),
        Failure::Other => i18n.error(error),
    };
    i18n.tp("error_report_failed", &[("error", why)])
}

#[component]
pub fn ReportButton(alert: Alert) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let enabled = state::enabled();
    let report = RwSignal::new(ReportState::Idle);
    let alert = StoredValue::new(alert);
    let send = move |_| {
        if !button_of(report.get_untracked()).1 {
            return;
        }
        report.set(ReportState::Sending);
        let alert = alert.get_value();
        let (title, message) = (i18n.text(&alert.title), i18n.text(&alert.message));
        spawn_local(async move {
            match api::send_alert(alert.id, &title, &message).await {
                Ok(sent) => {
                    let _ = report.try_set(ReportState::Sent);
                    toasts.show(
                        Level::Success,
                        i18n.tp("error_report_sent", &[("report_id", sent.report_id)]),
                        None,
                    );
                }
                Err(e) => {
                    let _ = report.try_set(ReportState::Failed);
                    toasts.show(Level::Error, failure_text(i18n, &e), None);
                }
            }
        });
    };
    view! {
        <Show when=move || enabled.get() == Some(true)>
            <Button
                variant=Variant::Secondary
                icon="bug_report"
                outlined=true
                loading=Signal::derive(move || report.get() == ReportState::Sending)
                disabled=Signal::derive(move || !button_of(report.get()).1)
                on_click=send
            >
                {move || i18n.t(button_of(report.get()).0)}
            </Button>
        </Show>
    }
}
