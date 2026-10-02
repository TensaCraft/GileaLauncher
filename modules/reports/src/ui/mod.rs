//! The reports module's UI: «Send report» in an alert that can be reported, the build report
//! window from a build's menu, and the section «Звіти» with the contact.

mod alert;
mod api;
mod build_report;
mod settings;
mod state;

use leptos::prelude::*;
use ui_kit::module::{
    BuildActionRun, ModuleAlertAction, ModuleBuildAction, ModuleOverlay, ModuleSection, UiModule,
};

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

    fn build_actions(&self) -> Vec<ModuleBuildAction> {
        vec![ModuleBuildAction {
            module: crate::ID,
            id: "report",
            icon: "bug_report",
            label: "version_report_button",
            run: BuildActionRun::Open(state::open_build_report),
            // Only where reports can go (the build profile has somewhere to send them).
            applies: |_| state::is_enabled(),
        }]
    }

    fn overlays(&self) -> Vec<ModuleOverlay> {
        vec![ModuleOverlay {
            module: crate::ID,
            view: || view! { <build_report::BuildReportHost /> }.into_any(),
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
    use ui_kit::module::{BuildActionRun, UiModule};

    use launcher_shared::{AppError, ErrorCode};

    use super::alert::{Failure, ReportState, button_of, failure_of};
    use super::build_report::can_send;
    use super::*;

    #[test]
    fn the_module_adds_an_alert_action_a_menu_dialog_and_a_section() {
        let ui = ReportsModuleUi;
        assert_eq!(ui.alert_actions().iter().map(|a| a.module).collect::<Vec<_>>(), [crate::ID]);
        assert_eq!(ui.overlays().len(), 1, "the build report's window");
        let actions = ui.build_actions();
        assert_eq!(
            actions.iter().map(|a| (a.id, a.icon, a.label)).collect::<Vec<_>>(),
            [("report", "bug_report", "version_report_button")]
        );
        assert!(matches!(actions[0].run, BuildActionRun::Open(_)));
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
    fn a_build_report_needs_a_message_before_sending() {
        assert!(!can_send(""));
        assert!(!can_send(" \n "));
        assert!(can_send("It crashes on start"));
    }

    #[test]
    fn the_locales_name_the_section_reports() {
        let uk: serde_json::Value = serde_json::from_str(include_str!("../../locales/uk_UA.json")).unwrap();
        let en: serde_json::Value = serde_json::from_str(include_str!("../../locales/en_US.json")).unwrap();
        assert_eq!(uk["settings_tab_reports"], "Звіти");
        assert_eq!(en["settings_tab_reports"], "Reports");
        for key in ["reports_settings_desc", "reports_attachments_title", "reports_no_attachments"] {
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
