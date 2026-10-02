//! The reports module in the browser preview: reports "go" and get ids R-1, R-2…
//! (`?reports=fail` — the server is away; `?reports=alert` — Play raises a crash alert that can be
//! reported; both: `?reports=alert,fail`).

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};

#[derive(Default)]
pub struct MockReports {
    fails: bool,
    contact: String,
    sent: u32,
}

impl MockReports {
    pub fn new(fails: bool) -> MockReports {
        MockReports { fails, ..MockReports::default() }
    }

    fn sent(&mut self) -> AppResult<Value> {
        if self.fails {
            return Err(AppError::new(ErrorCode::Network, "mock: the reports server is away"));
        }
        self.sent += 1;
        Ok(json!({"report_id": format!("R-{}", self.sent)}))
    }

    /// Answers `module_invoke` for module `reports`.
    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        let text = |key: &str| args[key].as_str().unwrap_or_default().trim().to_string();
        match command {
            "status" => Ok(json!({"enabled": true})),
            "send_alert" => self.sent(),
            "send_build" => {
                self.contact = text("contact");
                self.sent()
            }
            "attachments" => Ok(json!(["latest.log", "launch.log"])),
            "settings" => Ok(json!({"contact": self.contact})),
            "set_settings" => {
                self.contact = text("contact");
                Ok(Value::Null)
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock reports: no {other}"))),
        }
    }
}
