//! The diagnostics module in the browser preview: with `?crash=1` every launch crashes at once
//! with the diagnosis a Fabric build missing Fabric API would have; the Diagnostics tab shows
//! where the build's logs are and its last diagnosis.

use std::collections::HashMap;

use launcher_shared::{AppError, AppResult, ErrorCode, GameEvent, GameState, Text, names};
use module_diagnostics::dto::{
    BUILD_DIAGNOSTICS, BuildArgs, BuildDiagnostics, Confidence, DIAGNOSIS_EVENT, Diagnosis, DiagnosisEvent,
    Finding, FixAction, FixKind, Safety, Severity,
};
use serde_json::{Value, json};
use ui_kit::ipc;

use crate::mock_builds::to_value;

pub struct MockDiagnostics {
    crashes: bool,
    /// The last diagnosis of each build that crashed.
    diagnoses: HashMap<String, Diagnosis>,
}

impl MockDiagnostics {
    pub fn new(crashes: bool) -> MockDiagnostics {
        MockDiagnostics { crashes, diagnoses: HashMap::new() }
    }

    pub fn crashes(&self) -> bool {
        self.crashes
    }

    /// A launch that crashes at once, announced as the module announces it.
    pub fn crash(&mut self, key: &str, name: &str) -> AppResult<Value> {
        for state in
            [GameState::Started { pid: 4242 }, GameState::Crashed { code: Some(1), early: true, log: None }]
        {
            ipc::emit_mock(
                names::GAME,
                to_value(GameEvent { build_key: key.into(), build_name: name.into(), state })?,
            );
        }
        let diagnosis = sample_diagnosis();
        self.diagnoses.insert(key.into(), diagnosis.clone());
        let event = DiagnosisEvent { build_key: key.into(), build_name: name.into(), diagnosis };
        ipc::emit_mock(DIAGNOSIS_EVENT, to_value(event)?);
        Ok(json!(4242))
    }

    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        match command {
            BUILD_DIAGNOSTICS => {
                let args: BuildArgs = serde_json::from_value(args.clone())
                    .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
                to_value(self.diagnostics(&args.key))
            }
            other => Err(AppError::new(ErrorCode::NotFound, format!("mock diagnostics has no {other}"))),
        }
    }

    /// Where a preview build's logs are, and its last crash's diagnosis.
    fn diagnostics(&self, key: &str) -> BuildDiagnostics {
        let game = format!("C:\\Users\\Steve\\AppData\\Roaming\\Launcher\\games\\{key}");
        BuildDiagnostics {
            logs_dir: format!("{game}\\logs"),
            crash_reports_dir: format!("{game}\\crash-reports"),
            latest_log: format!("{game}\\logs\\latest.log"),
            latest_crash: self
                .diagnoses
                .contains_key(key)
                .then(|| format!("{game}\\crash-reports\\crash-client.txt")),
            launch_log: format!("{game}\\logs\\launch.log"),
            last: self.diagnoses.get(key).cloned(),
            game_dir: game,
        }
    }
}

/// A crash as a Fabric build with a mod missing Fabric API and a damaged Minecraft would have.
fn sample_diagnosis() -> Diagnosis {
    let action = |id: &str, kind: FixKind, safety: Safety, title: &str| FixAction {
        id: id.into(),
        kind,
        safety,
        title: Text::key(title),
    };
    Diagnosis {
        engine_version: 1,
        created_at: 1_790_676_000,
        findings: vec![
            Finding {
                id: "mods.missing_dependency.fabric_api".into(),
                kind: "missing_mod_dependency".into(),
                severity: Severity::Warning,
                confidence: Confidence::Exact,
                priority: 100,
                title: Text::key("launch_diagnostic_missing_mod_dependency_title"),
                message: Text::key("launch_diagnostic_missing_mod_dependency")
                    .param("mod", "needs_api")
                    .param("dependency", "fabric-api"),
                evidence: vec![
                    "- Mod 'Needs API' (needs_api) 1.0 requires any version of fabric-api, which is missing!"
                        .into(),
                ],
                actions: vec![action(
                    "open_mod_manager",
                    FixKind::OpenModManager,
                    Safety::Safe,
                    "diagnostic_action_open_mod_manager",
                )],
                suppresses: vec!["mods.incompatible.generic".into()],
            },
            Finding {
                id: "runtime.minecraft.missing".into(),
                kind: "missing_minecraft".into(),
                severity: Severity::Error,
                confidence: Confidence::Exact,
                priority: 100,
                title: Text::key("launch_diagnostic_missing_minecraft_title"),
                message: Text::key("launch_diagnostic_missing_minecraft"),
                evidence: vec![
                    "Mod ID: 'minecraft', Requested by: 'fabricloader', Actual version: '[MISSING]'".into(),
                ],
                actions: vec![action(
                    "repair_minecraft",
                    FixKind::Repair,
                    Safety::Confirm,
                    "diagnostic_action_repair",
                )],
                suppresses: Vec::new(),
            },
        ],
        files: vec!["latest.log".into()],
    }
}
