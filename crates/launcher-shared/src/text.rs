use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A user-facing string: either a translation key with params or raw text (e.g. a file name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Text {
    Key {
        key: String,
        #[serde(default)]
        params: BTreeMap<String, String>,
    },
    Raw {
        text: String,
    },
}

impl Text {
    pub fn key(key: impl Into<String>) -> Self {
        Text::Key { key: key.into(), params: BTreeMap::new() }
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Text::Raw { text: text.into() }
    }

    /// Adds a placeholder value. No-op for `Raw`.
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        if let Text::Key { params, .. } = &mut self {
            params.insert(name.into(), value.into());
        }
        self
    }
}
