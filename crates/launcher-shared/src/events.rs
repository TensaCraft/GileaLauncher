use serde::{Deserialize, Serialize};

use crate::Text;

/// Tauri event channel names (backend -> UI).
pub mod names {
    pub const OPS: &str = "app://ops";
    pub const ACTIVITY: &str = "app://activity";
    pub const TOAST: &str = "app://toast";
    pub const ALERT: &str = "app://alert";
    pub const SETTINGS: &str = "app://settings";
    pub const EXTERNAL_LAUNCH: &str = "app://external-launch";
    pub const UPDATE: &str = "app://update";
    pub const PROFILES: &str = "app://profiles";
    pub const AUTH: &str = "app://auth";
    pub const GAME: &str = "app://game";
    pub const BUILDS: &str = "app://builds";
    /// A close or a quit waits: work is under way (the payload: its titles). The window asks.
    pub const QUIT_HELD: &str = "app://quit-held";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationDto {
    pub id: u64,
    pub parent_id: Option<u64>,
    pub title: Text,
    pub kind: String,
    pub status: Option<Text>,
    pub progress: Option<f64>,
    pub total: Option<f64>,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpsSnapshot {
    pub busy: bool,
    pub operations: Vec<OperationDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityEvent {
    Begin,
    Update,
    Finish,
    Notify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub seq: u64,
    pub at_ms: u64,
    pub event: ActivityEvent,
    pub level: Level,
    pub message: Text,
    pub operation_id: Option<u64>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastAction {
    pub label: Text,
    /// UI-side action id, e.g. `route:/builds` or `restart`.
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    pub id: u64,
    pub level: Level,
    pub title: Text,
    pub message: Option<Text>,
    pub action: Option<ToastAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub id: u64,
    pub title: Text,
    pub message: Text,
    pub allow_report: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalLaunch {
    pub version_id: Option<String>,
}

/// What a started game is doing (`app://game`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GameState {
    Started {
        pid: u32,
    },
    /// Still running after the early-exit window; `close_launcher` asks the app to quit.
    Running {
        close_launcher: bool,
    },
    /// Exited normally, or was stopped by the launcher.
    Exited {
        code: Option<i32>,
    },
    /// Exited during startup, or later with a non-zero code; `log` is the file to read.
    Crashed {
        code: Option<i32>,
        early: bool,
        log: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEvent {
    pub build_key: String,
    pub build_name: String,
    pub state: GameState,
}
