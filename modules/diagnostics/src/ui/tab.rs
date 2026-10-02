//! The Diagnostics tab (the original's Diagnostics view): the build's folders and logs at hand,
//! and what its last crash was.

use launcher_shared::{AppError, ErrorCode, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, EmptyState, Section, Skeleton, ipc, use_toasts};

use super::dialog::FindingView;
use super::state;
use crate::dto::{BUILD_DIAGNOSTICS, BuildArgs, BuildDiagnostics};

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

/// Local "dd.mm.yyyy hh:mm" of Unix seconds.
fn date_time(seconds: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64 * 1000.0));
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}

#[component]
pub fn DiagnosticsPanel(key: String) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |k: &'static str| Signal::derive(move || i18n.t(k));
    let info = RwSignal::new(None::<Result<BuildDiagnostics, AppError>>);
    // Read now, and again when this build crashes while the tab is open.
    let crashed = {
        let (key, crash) = (key.clone(), state::crash());
        Memo::new(move |_| {
            crash.with(|c| c.as_ref().filter(|c| c.build_key == key).map(|c| c.diagnosis.created_at))
        })
    };
    Effect::new(move |_| {
        if crashed.get().is_none() && info.get_untracked().is_some() {
            return;
        }
        let key = key.clone();
        spawn_local(async move {
            let answer =
                ipc::module_invoke::<_, BuildDiagnostics>(crate::ID, BUILD_DIAGNOSTICS, &BuildArgs { key })
                    .await;
            let _ = info.try_set(Some(answer));
        });
    });
    // A folder opens; a file is shown in its folder (a file path would be run, not opened).
    let open = move |path: String, folder: bool| {
        spawn_local(async move {
            let command = if folder { "open_path" } else { "reveal_path" };
            if let Err(e) = ipc::invoke::<_, ()>(command, &PathArgs { path: path.clone() }).await {
                let text = if matches!(e.code, ErrorCode::NotFound | ErrorCode::InvalidDirectoryPath) {
                    i18n.tp("diagnostic_path_missing", &[("path", path)])
                } else {
                    i18n.error(&e)
                };
                toasts.show(Level::Warning, text, None);
            }
        });
    };
    let buttons = move |d: BuildDiagnostics| {
        let entries: Vec<(&'static str, &'static str, Option<String>, bool)> = vec![
            ("folder_open", "open_version_folder", Some(d.game_dir.clone()), true),
            ("folder_open", "open_version_logs", Some(d.logs_dir.clone()), true),
            ("folder_open", "open_crash_reports", Some(d.crash_reports_dir.clone()), true),
            ("description", "open_latest_log", Some(d.latest_log.clone()), false),
            ("description", "open_latest_crash", d.latest_crash.clone(), false),
            ("description", "open_launch_diagnostics", Some(d.launch_log.clone()), false),
        ];
        view! {
            <div class="diagnostics__actions">
                {entries
                    .into_iter()
                    .map(|(icon, label, path, folder)| {
                        let missing = path.is_none();
                        view! {
                            <Button
                                icon=icon
                                disabled=missing
                                on_click=move |_| {
                                    if let Some(p) = path.clone() {
                                        open(p, folder);
                                    }
                                }
                            >
                                {move || i18n.t(label)}
                            </Button>
                        }
                    })
                    .collect_view()}
            </div>
        }
    };
    let last = move |d: &BuildDiagnostics| match d.last.clone() {
        None => view! { <EmptyState icon="verified" title=t("diagnostic_no_diagnosis") /> }.into_any(),
        Some(diagnosis) => {
            let when = date_time(diagnosis.created_at);
            view! {
                <div class="diagnostics__last">
                    <div class="create__count">{move || i18n.tp("diagnostic_last_title", &[("date", when.clone())])}</div>
                    {diagnosis.findings.into_iter().map(|f| view! { <FindingView finding=f /> }).collect_view()}
                </div>
            }
            .into_any()
        }
    };
    let body = move || match info.get() {
        None => view! { <div class="list-row create__skeleton"><Skeleton width=260 /></div> }.into_any(),
        Some(Err(e)) => {
            view! { <EmptyState icon="error_outline" title=t("unknown_error") desc=i18n.error(&e) /> }
                .into_any()
        }
        Some(Ok(d)) => {
            let (actions, past) = (buttons(d.clone()), last(&d));
            view! {
                <Section icon="troubleshoot" title=t("version_section_diagnostics") desc=t("version_section_diagnostics_desc")>
                    {actions}
                </Section>
                {past}
            }
            .into_any()
        }
    };
    view! { <div class="diagnostics">{body}</div> }
}
