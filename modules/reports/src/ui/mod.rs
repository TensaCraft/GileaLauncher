//! The reports module's UI: «Send report» in an alert that can be reported, «Report a launcher
//! problem» (from the Support window or a failed operation), the question after the launcher
//! crashed, and the section «Звіти» with the contact.

mod alert;
mod api;
mod crash_prompt;
mod problem;
mod settings;
mod state;

use leptos::prelude::*;
use ui_kit::module::{ModuleAlertAction, ModuleOverlay, ModuleSection, ModuleSupportAction, UiModule};

pub struct ReportsModuleUi;

impl UiModule for ReportsModuleUi {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../../locales/en_US.json")),
            _ => None,
        }
    }

    fn alert_actions(&self) -> Vec<ModuleAlertAction> {
        vec![ModuleAlertAction {
            module: crate::ID,
            view: |alert| view! { <alert::ReportButton alert=alert /> }.into_any(),
        }]
    }

    fn overlays(&self) -> Vec<ModuleOverlay> {
        vec![
            ModuleOverlay { module: crate::ID, view: || view! { <problem::ProblemReportHost /> }.into_any() },
            ModuleOverlay {
                module: crate::ID,
                view: || view! { <crash_prompt::CrashPromptHost /> }.into_any(),
            },
        ]
    }

    fn support_actions(&self) -> Vec<ModuleSupportAction> {
        vec![ModuleSupportAction {
            module: crate::ID,
            view: || view! { <problem::SupportProblemRow /> }.into_any(),
        }]
    }

    fn settings_sections(&self) -> Vec<ModuleSection> {
        vec![ModuleSection {
            module: crate::ID,
            id: "reports",
            icon: "bug_report",
            label: "settings_tab_reports",
            after: "backups",
            view: || view! { <settings::ReportsSettings /> }.into_any(),
        }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(ReportsModuleUi)
}

#[cfg(test)]
mod tests {
    use ui_kit::module::UiModule;
    use ui_kit::problem::ProblemDraft;

    use launcher_shared::{AppError, ErrorCode};

    use super::alert::{Failure, ReportState, button_of, failure_of};
    use super::problem::can_send;
    use super::*;

    #[test]
    fn the_module_adds_a_report_button_its_windows_a_support_row_and_a_section() {
        let ui = ReportsModuleUi;
        assert_eq!(ui.alert_actions().iter().map(|a| a.module).collect::<Vec<_>>(), [crate::ID]);
        assert_eq!(ui.overlays().len(), 2, "the problem's window and the question after a crash");
        assert_eq!(ui.support_actions().len(), 1);
        assert!(ui.build_actions().is_empty(), "a build's game logs are not the launcher's to send");
        let sections = ui.settings_sections();
        assert_eq!(
            sections.iter().map(|s| (s.id, s.label, s.after)).collect::<Vec<_>>(),
            [("reports", "settings_tab_reports", "backups")]
        );
    }

    #[test]
    fn the_report_button_goes_idle_sending_sent_or_retry() {
        assert_eq!(button_of(ReportState::Idle), ("send_error_report", true));
        assert_eq!(button_of(ReportState::Sending), ("error_report_sending", false));
        assert_eq!(button_of(ReportState::Sent), ("error_report_sent_button", false));
        assert_eq!(button_of(ReportState::Failed), ("error_report_retry", true));
    }

    #[test]
    fn a_problem_report_needs_words_or_an_error() {
        assert!(!can_send("", None));
        assert!(!can_send(" \n ", Some(&ProblemDraft::default())));
        assert!(can_send("It hangs", None));
        let error = ProblemDraft {
            title: "Не вдалося встановити Aero".into(), ..ProblemDraft::default()
        };
        assert!(can_send("", Some(&error)));
    }

    #[test]
    fn the_locales_name_the_section_reports() {
        let uk: serde_json::Value = serde_json::from_str(include_str!("../../locales/uk_UA.json")).unwrap();
        let en: serde_json::Value = serde_json::from_str(include_str!("../../locales/en_US.json")).unwrap();
        assert_eq!(uk["settings_tab_reports"], "Звіти");
        assert_eq!(en["settings_tab_reports"], "Reports");
        for key in [
            "reports_settings_desc",
            "problem_report_title",
            "problem_report_hint",
            "support_problem_title",
            "crash_prompt_title",
            "crash_prompt_text",
        ] {
            assert!(uk.get(key).is_some() && en.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn a_refused_report_says_why() {
        let refused = AppError::new(ErrorCode::Network, "x").with_param("reason", "HTTP 413: too big");
        assert_eq!(failure_of(&refused), Failure::Refused("HTTP 413: too big".into()));
        assert_eq!(failure_of(&AppError::new(ErrorCode::NotFound, "gone")), Failure::Gone);
        assert_eq!(
            failure_of(&AppError::new(ErrorCode::Network, "offline")),
            Failure::Other,
            "no answer at all"
        );
    }
}
