//! Modrinth's own types and rules; the provider contract's data is `launcher_shared::provider`.

use launcher_shared::ContentKind;
use launcher_shared::provider::ProviderInfo;
use launcher_shared::url::url_escape;
use serde::{Deserialize, Serialize};

/// `installed`: the projects of `kind` installed in build `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildArgs {
    pub key: String,
    pub kind: ContentKind,
}

/// Modrinth's project type of a kind.
pub fn project_type(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mod",
        ContentKind::ResourcePacks => "resourcepack",
        ContentKind::ShaderPacks => "shader",
    }
}

/// A project's page on modrinth.com (`project_type` "mod" when unknown; `id` its slug or id).
pub fn modrinth_url(project_type: &str, id: &str) -> String {
    let kind = if project_type.is_empty() { "mod" } else { project_type };
    format!("https://modrinth.com/{}/{}", url_escape(kind), url_escape(id))
}

/// What Modrinth offers the launcher: every kind of content (updates checked for mods) and
/// modpacks.
pub fn provider_info() -> ProviderInfo {
    ProviderInfo {
        id: crate::ID.into(),
        name: "Modrinth".into(),
        icon: "brand:modrinth".into(),
        content: ContentKind::ALL.to_vec(),
        updates: ContentKind::ALL.to_vec(),
        modpacks: true,
        modpack_updates: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_page_is_on_modrinth() {
        assert_eq!(modrinth_url("mod", "sodium"), "https://modrinth.com/mod/sodium");
        assert_eq!(modrinth_url("shader", "AA nobb"), "https://modrinth.com/shader/AA%20nobb");
        assert_eq!(modrinth_url("", "x"), "https://modrinth.com/mod/x");
        assert_eq!(project_type(ContentKind::ResourcePacks), "resourcepack");
        assert_eq!(project_type(ContentKind::ShaderPacks), "shader");
    }
}
