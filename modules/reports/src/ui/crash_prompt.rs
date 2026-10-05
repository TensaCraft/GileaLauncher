//! After the launcher crashed: at the next start, the user is asked whether to report it. Nothing
//! goes unless they press «Send»; «Don't send» forgets the crash.

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, DialogTone, Field, TextArea, Variant, use_toasts};

use super::alert::{ReportState, failure_text};
use super::{api, state};

#[component]
pub fn CrashPromptHost() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let enabled = state::enabled();
    let open = RwSignal::new(false);
    let crash = RwSignal::new(None::<api::Crash>);
    let comment = RwSignal::new(String::new());
    let report = RwSignal::new(ReportState::Idle);
    let asked = StoredValue::new(false);
    // Once reports are known to have somewhere to go, ask once for a crash waiting.
    Effect::new(move |_| {
        if enabled.get() != Some(true) || asked.get_value() {
            return;
        }
        asked.set_value(true);
        spawn_local(async move {
            if let Ok(Some(waiting)) = api::last_crash().await {
                let _ = crash.try_set(Some(waiting));
                let _ = open.try_set(true);
            }
        });
    });
    let dismiss = Callback::new(move |_| {
        open.set(false);
        spawn_local(async move {
            let _ = api::dismiss_crash().await;
        });
    });
    let send = move |_| {
        if report.get_untracked() == ReportState::Sending {
            return;
        }
        report.set(ReportState::Sending);
        let words = comment.get_untracked();
        spawn_local(async move {
            let contact = api::settings().await.map(|s| s.contact).unwrap_or_default();
            match api::send_crash(&words, &contact).await {
                Ok(sent) => {
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
    let text = move || {
        let version = crash.with(|c| c.as_ref().map(|c| c.version.clone()).unwrap_or_default());
        i18n.tp("crash_prompt_text", &[("version", version)])
    };
    let submit_label = move || match report.get() {
        ReportState::Sending => i18n.t("error_report_sending"),
        ReportState::Failed => i18n.t("error_report_retry"),
        _ => i18n.t("send_error_report"),
    };
    view! {
        <Dialog open=open title=t("crash_prompt_title") icon="report" tone=DialogTone::Warning on_close=dismiss>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| dismiss.run(())>{move || i18n.t("crash_prompt_skip")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="send"
                    loading=Signal::derive(move || report.get() == ReportState::Sending)
                    on_click=send
                >
                    {submit_label}
                </Button>
            </DialogFooter>
            <p style="margin:0 0 14px">{text}</p>
            <Field label=t("crash_prompt_comment_label")>
                <TextArea value=comment placeholder=t("crash_prompt_comment_hint") rows=3 />
            </Field>
        </Dialog>
    }
}
