//! What a report must not carry: the user's home folder and access tokens.

use std::path::Path;

use regex::Regex;
use serde_json::Value;

/// Stands for the home folder in a report.
pub const HOME_MARK: &str = "<USER_HOME>";
const HIDDEN: &str = "${1}<redacted>";

pub struct Redactor {
    /// The home folder in its native and `/` forms, longest first, any case.
    home: Vec<Regex>,
    tokens: [Regex; 4],
}

impl Redactor {
    pub fn new(home: Option<&Path>) -> Redactor {
        let mut forms: Vec<String> = Vec::new();
        if let Some(home) = home.map(|h| h.to_string_lossy().trim_end_matches(['/', '\\']).to_string())
            // A root "home" would hide every path separator.
            && home.chars().any(|c| c != '/' && c != '\\')
        {
            forms.push(home.clone());
            forms.push(home.replace('\\', "/"));
            // As a debug string or JSON writes it (the launcher's log does): every `\` doubled.
            forms.push(home.replace('\\', "\\\\"));
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        forms.dedup();
        let home = forms
            .iter()
            .map(|form| {
                Regex::new(&format!("(?i){}", regex::escape(form))).expect("an escaped path is a regex")
            })
            .collect();
        let tokens = [
            Regex::new(r"(?i)(authorization\s*:\s*bearer\s+)([^\s]+)").expect("valid"),
            // The launch command's token; very old versions pass `--session token:<token>:<uuid>`.
            Regex::new(r"(?i)(--(?:accessToken|session)\s+)([^\s]+)").expect("valid"),
            // Minecraft 1.7–1.8 log "(Session ID is token:<token>:<uuid>)".
            Regex::new(r"(?i)(session id is token:)([^:\s)]+)").expect("valid"),
            Regex::new(
                r#"(?ix)(["']?(?:access[_-]?token|refresh[_-]?token|client[_-]?secret)["']?\s*[:=]\s*["']?)([^"',\s}]+)"#,
            )
            .expect("valid"),
        ];
        Redactor { home, tokens }
    }

    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for home in &self.home {
            out = home.replace_all(&out, HOME_MARK).into_owned();
        }
        for token in &self.tokens {
            out = token.replace_all(&out, HIDDEN).into_owned();
        }
        out
    }

    /// Every string inside `value` redacted; numbers and booleans kept; empty (`null`) fields
    /// dropped.
    pub fn value(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(fields) => Value::Object(
                fields.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, self.value(v))).collect(),
            ),
            other => other,
        }
    }
}
