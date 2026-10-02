//! The dialog a crash opens (the original's "Crash after launch"), and a finding as the dialog and
//! the Diagnostics tab show it.

use launcher_shared::{AppError, BuildsSnapshot, ErrorCode, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::module::use_module_host;
use ui_kit::{Button, ConfirmDialog, Dialog, DialogFooter, DialogTone, Variant, ipc, use_toasts};

use super::state;
use crate::dto::{Diagnosis, DiagnosisEvent, Finding, FixKind};

/// The module's styles, with its window: the dialog and the Diagnostics tab use them.
const STYLES: &str = include_str!("../../styles/diagnostics.css");

/// What the dialog offers: the diagnostics always, then each kind the findings name, once, in
/// their order; a server sync waits for server builds.
pub fn offered(d: &Diagnosis) -> Vec<FixKind> {
    let mut kinds = vec![FixKind::OpenDiagnostics];
    for kind in d.findings.iter().flat_map(|f| f.actions.iter().map(|a| a.kind)) {
        if !kinds.contains(&kind) && kind != FixKind::RepairSync {
            kinds.push(kind);
        }
    }
    kinds
}

/// How many findings there are beside the main one.
pub fn more_count(d: &Diagnosis) -> usize {
    d.findings.len().saturating_sub(1)
}

/// One finding: its title (unless something above already says it), what it means and the log
/// lines that show it.
#[component]
pub fn FindingView(finding: Finding, #[prop(optional)] untitled: bool) -> impl IntoView {
    let i18n = use_i18n();
    let (title, message) = (i18n.text(&finding.title), i18n.text(&finding.message));
    let title = (!untitled).then(|| view! { <div class="finding__title">{title}</div> });
    let evidence = (!finding.evidence.is_empty()).then(|| {
        view! { <pre class="finding__evidence">{finding.evidence.join("\n")}</pre> }
    });
    view! {
        <div class="finding">
            {title}
            <p class="finding__message">{message}</p>
            {evidence}
        </div>
    }
}

#[derive(serde::Serialize)]
struct IdArgs {
    id: String,
}

/// The module's window over the app: its styles and the crash dialog; it hears of crashes.
#[component]
pub fn CrashHost() -> impl IntoView {
    state::listen();
    view! {
        <style>{STYLES}</style>
        <CrashDialog />
    }
}

/// Verifies the files of build `key`'s component: its mod loader, else its Minecraft.
async fn verify(key: &str) -> Result<(), AppError> {
    let snapshot = ipc::call::<BuildsSnapshot>("builds_list").await?;
    let build = snapshot.builds.into_iter().find(|b| b.key == key).ok_or_else(|| {
        AppError::new(ErrorCode::VersionNotFound, format!("no build {key}")).with_param("version", key)
    })?;
    let id = build.loader.or(build.version).unwrap_or_default();
    ipc::invoke::<_, serde_json::Value>("component_verify", &IdArgs { id }).await?;
    Ok(())
}

/// The dialog a crash opens (the original's "Crash after launch").
#[component]
fn CrashDialog() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let host = use_module_host();
    let crash: RwSignal<Option<DiagnosisEvent>> = state::crash().into();
    let open = RwSignal::new(false);
    let repair_open = RwSignal::new(false);
    Effect::new(move |_| open.set(crash.with(Option::is_some)));
    let close = move || crash.set(None);
    let event = move || crash.get();
    let go = move |tab: &'static str| {
        if let (Some(e), Some(host)) = (crash.get_untracked(), host) {
            host.open_content.run((e.build_key, tab.to_string()));
        }
        close();
    };
    let has = move |kind: FixKind| {
        crash.with(|c| c.as_ref().is_some_and(|c| offered(&c.diagnosis).contains(&kind)))
    };
    let repair = Callback::new(move |()| {
        repair_open.set(false);
        let Some(e) = crash.get_untracked() else { return };
        close();
        spawn_local(async move {
            match verify(&e.build_key).await {
                Ok(()) => {
                    if let Some(host) = host {
                        host.launch.run(e.build_key);
                    }
                }
                Err(err) => toasts.show(Level::Error, i18n.error(&err), None),
            }
        });
    });
    let title = Signal::derive(move || {
        event().and_then(|e| e.diagnosis.findings.first().map(|f| i18n.text(&f.title))).unwrap_or_default()
    });
    let subtitle = Signal::derive(move || event().map(|e| e.build_name).unwrap_or_default());
    let body = move || {
        event().map(|e| {
            let more = more_count(&e.diagnosis);
            let others: Vec<String> = e.diagnosis.findings.iter().skip(1).map(|f| i18n.text(&f.title)).collect();
            let main = e.diagnosis.findings.first().cloned();
            view! {
                {main.map(|f| view! { <FindingView finding=f untitled=true /> })}
                {(more > 0).then(|| view! {
                    <p class="finding__more">{i18n.tp("launch_diagnostic_multiple", &[("count", more.to_string())])}</p>
                    <ul class="finding__others">{others.into_iter().map(|t| view! { <li>{t}</li> }).collect_view()}</ul>
                })}
            }
        })
    };
    let repair_title = Signal::derive(move || {
        let name = event().map(|e| e.build_name).unwrap_or_default();
        i18n.tp("diagnostic_repair_confirm_title", &[("version", name)])
    });
    view! {
        <Dialog
            open=open
            title=title
            subtitle=subtitle
            icon="warning_amber"
            tone=DialogTone::Warning
            wide=true
            on_close=Callback::new(move |_| close())
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| close()>{move || i18n.t("close")}</Button>
                <Show when=move || has(FixKind::Repair)>
                    <Button icon="build" on_click=move |_| repair_open.set(true)>{move || i18n.t("diagnostic_action_repair")}</Button>
                </Show>
                <Show when=move || has(FixKind::OpenModManager)>
                    <Button icon="extension" on_click=move |_| go("mods")>{move || i18n.t("diagnostic_action_open_mod_manager")}</Button>
                </Show>
                <Button variant=Variant::Primary icon="troubleshoot" on_click=move |_| go("diagnostics")>
                    {move || i18n.t("open_crash_diagnostics")}
                </Button>
            </DialogFooter>
            {body}
        </Dialog>
        <ConfirmDialog
            open=repair_open
            title=repair_title
            message=Signal::derive(move || i18n.t("diagnostic_repair_confirm_message"))
            confirm_label=Signal::derive(move || i18n.t("diagnostic_action_repair"))
            cancel_label=Signal::derive(move || i18n.t("cancel"))
            on_confirm=repair
        />
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::Text;

    use crate::dto::{Confidence, Diagnosis, Finding, FixAction, FixKind, Safety, Severity};

    use super::*;

    fn finding_with(id: &str, kinds: Vec<FixKind>) -> Finding {
        Finding {
            id: id.into(),
            kind: "k".into(),
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 100,
            title: Text::key("t"),
            message: Text::key("m"),
            evidence: Vec::new(),
            actions: kinds
                .into_iter()
                .map(|kind| FixAction { id: "a".into(), kind, safety: Safety::Safe, title: Text::key("a") })
                .collect(),
            suppresses: Vec::new(),
        }
    }

    fn diagnosis(findings: Vec<Finding>) -> Diagnosis {
        Diagnosis { engine_version: 1, created_at: 0, findings, files: Vec::new() }
    }

    #[test]
    fn a_crash_offers_the_actions_its_findings_name_once_each() {
        let d = diagnosis(vec![
            finding_with("mods.missing_dependency.a", vec![FixKind::OpenModManager]),
            finding_with("mods.missing_dependency.b", vec![FixKind::OpenModManager]),
            finding_with("runtime.minecraft.missing", vec![FixKind::Repair]),
        ]);
        assert_eq!(offered(&d), vec![FixKind::OpenDiagnostics, FixKind::OpenModManager, FixKind::Repair]);
        assert_eq!(more_count(&d), 2, "the others beside the main one");
        assert_eq!(more_count(&diagnosis(vec![finding_with("x", Vec::new())])), 0);
    }
}
