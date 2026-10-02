//! The build report window: the user's description (required), the contact
//! (kept for next time) and the build's logs the report attaches.

use launcher_shared::{BuildDto, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, Field, Icon, TextArea, TextInput, Variant, use_toasts};

use super::alert::{ReportState, failure_text};
use super::{api, state};

/// A report goes only with a description.
pub fn can_send(message: &str) -> bool {
    !message.trim().is_empty()
}

#[component]
pub fn BuildReportHost() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    // Asks the backend now: the build menu's entry shows only when reports can go.
    let _ = state::enabled();
    let asked = state::asked();
    let open = RwSignal::new(false);
    let build = RwSignal::new(None::<BuildDto>);
    let contact = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let files = RwSignal::new(None::<Vec<String>>);
    let report = RwSignal::new(ReportState::Idle);
    let tried = RwSignal::new(false);
    let take = asked.clone();
    Effect::new(move |_| {
        let Some(asked_build) = take.get() else { return };
        take.set(None);
        let key = asked_build.key.clone();
        build.set(Some(asked_build));
        message.set(String::new());
        files.set(None);
        report.set(ReportState::Idle);
        tried.set(false);
        open.set(true);
        spawn_local(async move {
            if let Ok(settings) = api::settings().await {
                let _ = contact.try_set(settings.contact);
            }
            let _ = files.try_set(Some(api::attachments(&key).await.unwrap_or_default()));
        });
    });
    let title = Signal::derive(move || {
        let name = build.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default());
        i18n.tp("version_report_title", &[("version", name)])
    });
    let missing = Signal::derive(move || tried.get() && !message.with(|m| can_send(m)));
    let send = move |_| {
        tried.set(true);
        let Some(key) = build.with_untracked(|b| b.as_ref().map(|b| b.key.clone())) else { return };
        if !can_send(&message.get_untracked()) || report.get_untracked() == ReportState::Sending {
            return;
        }
        report.set(ReportState::Sending);
        let (text, who) = (message.get_untracked(), contact.get_untracked());
        spawn_local(async move {
            match api::send_build(&key, &text, &who).await {
                Ok(sent) => {
                    let _ = report.try_set(ReportState::Idle);
                    let _ = open.try_set(false);
                    toasts.show(
                        Level::Success,
                        i18n.tp("version_report_sent", &[("report_id", sent.report_id)]),
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
        _ => i18n.t("version_report_submit"),
    };
    view! {
        <Dialog open=open title=title icon="bug_report" wide=true>
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
            <p class="hint" style="margin:0 0 14px">{move || i18n.t("version_report_description")}</p>
            <div style="display:flex;flex-direction:column;gap:14px">
                <Field label=t("report_contact_label") hint=t("report_contact_hint")>
                    <TextInput value=contact />
                </Field>
                <Field
                    label=t("version_report_message_label")
                    error=Signal::derive(move || missing.get().then(|| i18n.t("version_report_message_required")))
                >
                    <TextArea value=message placeholder=t("version_report_message_hint") invalid=missing rows=5 />
                </Field>
                <div>
                    <div class="hint" style="margin-bottom:6px">{move || i18n.t("reports_attachments_title")}</div>
                    {move || match files.get() {
                        None => view! { <div class="hint">"…"</div> }.into_any(),
                        Some(list) if list.is_empty() => {
                            view! { <div class="hint">{i18n.t("reports_no_attachments")}</div> }.into_any()
                        }
                        // File names keep their case (tags would shout them).
                        Some(list) => view! {
                            <div class="hint" style="display:flex;align-items:center;gap:6px;color:var(--text-2)">
                                <Icon name="description" size=16 />
                                {list.join(" · ")}
                            </div>
                        }
                        .into_any(),
                    }}
                </div>
            </div>
        </Dialog>
    }
}
