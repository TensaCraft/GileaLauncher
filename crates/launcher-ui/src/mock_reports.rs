//! The reports module in the browser preview: reports "go" and get ids R-1, R-2…
//! (`?reports=fail` — the server is away; `?reports=crash` — the launcher crashed last time, so
//! its question shows at start; both: `?reports=crash,fail`).

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};

#[derive(Default)]
pub struct MockReports {
    fails: bool,
    /// The launcher's own crash waits to be reported.
    crashed: bool,
    contact: String,
    sent: u32,
}

impl MockReports {
    pub fn new(fails: bool, crashed: bool) -> MockReports {
        MockReports { fails, crashed, ..MockReports::default() }
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
            "send_problem" => {
                let error = args["error"]["title"].as_str().is_some_and(|t| !t.trim().is_empty());
                if text("message").is_empty() && !error {
                    return Err(AppError::new(ErrorCode::InvalidInput, "mock: a report needs a description"));
                }
                self.contact = text("contact");
                self.sent()
            }
            "last_crash" => Ok(if self.crashed {
                json!({"version": "0.0.8", "at": "2026-10-05T18:00:00Z",
                       "message": "index out of bounds: the len is 0 but the index is 0",
                       "location": "crates/launcher-core/src/builds/service.rs:120:9"})
            } else {
                Value::Null
            }),
            "send_crash" => {
                let sent = self.sent()?;
                self.crashed = false;
                Ok(sent)
            }
            "dismiss_crash" => {
                self.crashed = false;
                Ok(Value::Null)
            }
            "settings" => Ok(json!({"contact": self.contact})),
            "set_settings" => {
                self.contact = text("contact");
                Ok(Value::Null)
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock reports: no {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_s_crash_is_offered_until_sent_or_dismissed() {
        let mut reports = MockReports::new(false, true);
        assert!(reports.handle("last_crash", &Value::Null).unwrap().is_object());
        reports.handle("dismiss_crash", &Value::Null).unwrap();
        assert!(reports.handle("last_crash", &Value::Null).unwrap().is_null());
        assert!(reports.handle("send_problem", &json!({"message": " "})).is_err());
        let error = json!({"message": "", "error": {"title": "Не вдалося встановити Aero"}});
        assert_eq!(reports.handle("send_problem", &error).unwrap()["report_id"], "R-1");
    }
}
