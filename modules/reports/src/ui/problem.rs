//! «Report a launcher problem»: from the Support window, or from a failed operation's toast with
//! its error. The report carries the user's words, the error and the launcher's own log — never
//! the game's files.

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::problem::{ProblemDraft, use_problem_reporter};
use ui_kit::{Button, Dialog, DialogFooter, Field, SettingRow, TextArea, TextInput, Variant, use_toasts};

use super::alert::{ReportState, failure_text};
use super::{api, state};

/// A report goes with the user's words, or with the error of a failure.
pub fn can_send(message: &str, error: Option<&ProblemDraft>) -> bool {
    !message.trim().is_empty() || error.is_some_and(|e| !e.title.trim().is_empty())
}

/// The window, opened by any `ProblemReporter` request; it tells the app reports can go.
#[component]
pub fn ProblemReportHost() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let reporter = use_problem_reporter();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let enabled = state::enabled();
    Effect::new(move |_| reporter.available.set(enabled.get() == Some(true)));
    let open = RwSignal::new(false);
    let error = RwSignal::new(None::<ProblemDraft>);
    let contact = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let report = RwSignal::new(ReportState::Idle);
    let tried = RwSignal::new(false);
    Effect::new(move |_| {
        let Some(draft) = reporter.request.get() else { return };
        reporter.request.set(None);
        error.set(Some(draft).filter(|d| !d.title.trim().is_empty()));
        message.set(String::new());
        report.set(ReportState::Idle);
        tried.set(false);
        open.set(true);
        spawn_local(async move {
            if let Ok(settings) = api::settings().await {
                let _ = contact.try_set(settings.contact);
            }
        });
    });
    let missing =
        Signal::derive(move || tried.get() && !error.with(|e| message.with(|m| can_send(m, e.as_ref()))));
    let send = move |_| {
        tried.set(true);
        let problem = error.get_untracked();
        if !can_send(&message.get_untracked(), problem.as_ref())
            || report.get_untracked() == ReportState::Sending
        {
            return;
        }
        report.set(ReportState::Sending);
        let (text, who) = (message.get_untracked(), contact.get_untracked());
        spawn_local(async move {
            match api::send_problem(&text, &who, problem.as_ref()).await {
                Ok(sent) => {
                    let _ = report.try_set(ReportState::Idle);
                    let _ = open.try_set(false);
                    toasts.show(
                        Level::Success,
                        i18n.tp("problem_report_sent", &[("report_id", sent.report_id)]),
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
    let submit_label = move || match report.get() {
        ReportState::Sending => i18n.t("error_report_sending"),
        ReportState::Failed => i18n.t("error_report_retry"),
        _ => i18n.t("send_error_report"),
    };
    // The error the report is about, as the user saw it, with its detail.
    let error_block = move || {
        error.get().map(|e| {
            view! {
                <div class="problem-report__error">
                    <div class="problem-report__error-title">{e.title}</div>
                    {e.detail.filter(|d| !d.trim().is_empty()).map(|d| view! { <code>{d}</code> })}
                </div>
            }
        })
    };
    view! {
        <Dialog open=open title=t("problem_report_title") icon="bug_report" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="send"
                    loading=Signal::derive(move || report.get() == ReportState::Sending)
                    on_click=send
                >
                    {submit_label}
                </Button>
            </DialogFooter>
            <p class="hint" style="margin:0 0 14px">{move || i18n.t("problem_report_hint")}</p>
            <div style="display:flex;flex-direction:column;gap:14px">
                {error_block}
                <Field
                    label=t("problem_report_message_label")
                    error=Signal::derive(move || missing.get().then(|| i18n.t("problem_report_message_required")))
                >
                    <TextArea value=message placeholder=t("problem_report_message_hint") invalid=missing rows=5 />
                </Field>
                <Field label=t("report_contact_label") hint=t("report_contact_hint")>
                    <TextInput value=contact />
                </Field>
            </div>
        </Dialog>
    }
}

/// The Support window's row: describe a problem with the launcher.
#[component]
pub fn SupportProblemRow() -> impl IntoView {
    let i18n = use_i18n();
    let reporter = use_problem_reporter();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let enabled = state::enabled();
    move || {
        (enabled.get() == Some(true)).then(|| {
            view! {
                <SettingRow title=t("support_problem_title") desc=t("support_problem_desc")>
                    <Button
                        variant=Variant::Secondary
                        icon="bug_report"
                        outlined=true
                        on_click=move |_| reporter.request.set(Some(ProblemDraft::default()))
                    >
                        {move || i18n.t("support_problem_open")}
                    </Button>
                </SettingRow>
            }
        })
    }
}
