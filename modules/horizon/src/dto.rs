//! What the module's page sends its backend.

use serde::{Deserialize, Serialize};

/// `create`: a new Horizon build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateArgs {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// A picture on this computer for its card.
    #[serde(default)]
    pub image_path: Option<String>,
}
