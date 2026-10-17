//! A build made by the build wizard: what it runs, its settings and the content picked for it
//! (`build_create_planned`).

use serde::{Deserialize, Serialize};

use crate::{ContentKind, LoaderKind};

/// A build to create, with its settings and the content to install into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedBuild {
    pub name: String,
    /// Minecraft version.
    pub mc: String,
    pub loader: LoaderKind,
    /// The loader build; none for vanilla.
    #[serde(default)]
    pub loader_version: Option<String>,
    #[serde(default)]
    pub settings: PlannedSettings,
    #[serde(default)]
    pub picks: Vec<PlannedPick>,
}

/// The wizard's settings: `None` keeps the new build's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedSettings {
    #[serde(default)]
    pub max_ram_gb: Option<u64>,
    #[serde(default)]
    pub gpu_mode: Option<String>,
    /// Its own account (a profile key).
    #[serde(default)]
    pub profile: Option<String>,
    /// A picked icon file.
    #[serde(default)]
    pub image_path: Option<String>,
    #[serde(default)]
    pub description: String,
}

/// A project picked from a provider; its newest compatible version is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPick {
    pub provider: String,
    pub kind: ContentKind,
    pub project_id: String,
    pub slug: String,
    pub title: String,
    /// Its picture, for the wizard's summary.
    #[serde(default)]
    pub icon_url: Option<String>,
}
