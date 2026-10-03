//! CurseForge's own numbers and names; the provider contract's data is `launcher_shared::provider`.

use launcher_shared::ContentKind;
use launcher_shared::provider::{ProviderInfo, loaders_run_by};

pub const NAME: &str = "CurseForge";
/// The class of modpacks.
pub const MODPACKS_CLASS: u32 = 4471;

/// CurseForge's class of a kind of content.
pub fn class_id(kind: ContentKind) -> u32 {
    match kind {
        ContentKind::Mods => 6,
        ContentKind::ResourcePacks => 12,
        ContentKind::ShaderPacks => 6552,
    }
}

/// CurseForge's mod loader type of a loader (`launcher_shared::provider::loader_name`).
pub fn loader_type(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

/// The name CurseForge lists among a file's game versions for a loader.
pub fn loader_tag(loader: &str) -> Option<&'static str> {
    match loader {
        "forge" => Some("Forge"),
        "fabric" => Some("Fabric"),
        "quilt" => Some("Quilt"),
        "neoforge" => Some("NeoForge"),
        _ => None,
    }
}

/// CurseForge's mod loader types of the loaders a build of `loader` runs (Quilt runs Fabric's
/// mods too), its own first.
pub fn loader_types(loader: &str) -> Vec<u32> {
    loaders_run_by(loader).iter().filter_map(|l| loader_type(l)).collect()
}

/// The names CurseForge tags the files of those loaders with, the build's own first.
pub fn loader_tags(loader: &str) -> Vec<&'static str> {
    loaders_run_by(loader).iter().filter_map(|l| loader_tag(l)).collect()
}

/// What CurseForge offers the launcher: mods, resource packs and shader packs, and modpacks, all
/// with their updates.
pub fn provider_info() -> ProviderInfo {
    ProviderInfo {
        id: crate::ID.into(),
        name: NAME.into(),
        icon: "brand:curseforge".into(),
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
    fn kinds_and_loaders_have_curseforge_s_numbers() {
        assert_eq!(class_id(ContentKind::Mods), 6);
        assert_eq!(class_id(ContentKind::ResourcePacks), 12);
        assert_eq!(class_id(ContentKind::ShaderPacks), 6552);
        let loaders: Vec<_> = ["forge", "fabric", "quilt", "neoforge", "minecraft"]
            .into_iter()
            .map(|l| (loader_type(l), loader_tag(l)))
            .collect();
        assert_eq!(
            loaders,
            [
                (Some(1), Some("Forge")),
                (Some(4), Some("Fabric")),
                (Some(5), Some("Quilt")),
                (Some(6), Some("NeoForge")),
                (None, None),
            ]
        );
    }
}
