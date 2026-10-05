//! Builds in the UI: pure helpers here, IPC actions, the Play flow and the
//! build dialogs in the submodules.

pub mod actions;
pub mod catalog;
pub mod dialogs;
pub mod gpu;
pub mod launch;
pub mod ram;

pub use launcher_shared::naming::{NameProblem, check_new_name, default_build_name, unique_name};
use launcher_shared::{BuildDto, LoaderKind, MemoryInfo};
use ui_kit::TagTone;

/// Catalog rows shown per "Load more".
pub const CATALOG_PAGE: usize = 80;

/// "{client} {version}" of a build, skipping missing parts.
pub fn build_subtitle(build: &BuildDto) -> String {
    [build.client.as_deref(), build.version.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The loader a build runs (its component, else its client's name; plain Minecraft else), as a
/// card's tag: its name and colour.
pub fn loader_tag(build: &BuildDto) -> (String, TagTone) {
    let kind = build
        .loader
        .as_deref()
        .and_then(LoaderKind::of_component)
        .or_else(|| build.client.as_deref().and_then(LoaderKind::from_client))
        .unwrap_or(LoaderKind::Minecraft);
    let tone = match kind {
        LoaderKind::Minecraft => TagTone::Vanilla,
        LoaderKind::Fabric => TagTone::Fabric,
        LoaderKind::Quilt => TagTone::Quilt,
        LoaderKind::Forge => TagTone::Forge,
        LoaderKind::NeoForge => TagTone::NeoForge,
    };
    (kind.display_name().to_string(), tone)
}

/// A card's line under the name: the version, after the client when the client is not just the
/// loader its tag names.
pub fn card_subtitle(build: &BuildDto) -> String {
    let tagged = loader_tag(build).0;
    match build.client.as_deref().map(str::trim) {
        Some(client) if !client.is_empty() && !client.eq_ignore_ascii_case(&tagged) => build_subtitle(build),
        _ => build.version.clone().unwrap_or_default(),
    }
}

/// The placeholder icon of a build without a picture.
pub fn loader_icon(loader: Option<&str>) -> &'static str {
    let loader = loader.unwrap_or_default().to_lowercase();
    if loader.contains("neoforge") {
        "construction"
    } else if loader.contains("forge") {
        "build"
    } else if loader.contains("fabric") {
        "extension"
    } else if loader.contains("quilt") {
        "grid_view"
    } else if loader.contains("minecraft") || loader.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        "layers"
    } else {
        "videogame_asset"
    }
}

/// The build an external `--launch-version=<id>` means: by key, then by version id.
pub fn find_build<'a>(builds: &'a [BuildDto], id: &str) -> Option<&'a BuildDto> {
    builds.iter().find(|b| b.key == id).or_else(|| builds.iter().find(|b| b.version_id == id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStep {
    /// A launch of this build is still being started.
    Ignore,
    ConfirmDuplicate,
    PickProfile,
    NeedProfile,
    Launch,
}

/// The build's own account while the launcher still has it: Play starts with it and asks none.
pub fn own_profile(build: &BuildDto, profiles: &[launcher_shared::ProfileDto]) -> Option<String> {
    build.profile.clone().filter(|key| profiles.iter().any(|p| &p.key == key))
}

/// What Play does next.
pub fn next_launch_step(
    launching: bool,
    running: bool,
    allow_duplicate: bool,
    ask_profile: bool,
    profiles: usize,
) -> LaunchStep {
    if launching {
        LaunchStep::Ignore
    } else if running && !allow_duplicate {
        LaunchStep::ConfirmDuplicate
    } else if ask_profile && profiles == 0 {
        LaunchStep::NeedProfile
    } else if ask_profile {
        LaunchStep::PickProfile
    } else {
        LaunchStep::Launch
    }
}

/// How many catalog rows `pages` pages show.
pub fn catalog_shown(total: usize, pages: usize) -> usize {
    total.min(pages.max(1) * CATALOG_PAGE)
}

/// `YYYY-MM-DD` of a manifest `releaseTime`.
pub fn release_date(raw: Option<&str>) -> Option<String> {
    let date = raw?.get(..10)?;
    let ok = date.len() == 10
        && date.char_indices().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    ok.then(|| date.to_string())
}

pub use ui_kit::LatestRequest;

/// Min, max and recommended GiB for the memory slider; the range never collapses.
pub fn ram_slider(info: &MemoryInfo) -> (f64, f64, f64) {
    let min = info.min_heap_gb.max(1);
    let max = info.max_heap_gb.max(min + 1);
    let recommended = info.recommended_heap_gb.clamp(min, max);
    (min as f64, max as f64, recommended as f64)
}

/// `raw` for a URL query: unreserved characters as they are, the rest as `%XX` of their UTF-8.
pub fn query_escape(raw: &str) -> String {
    launcher_shared::url::url_escape(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(key: &str, version_id: &str, name: &str) -> BuildDto {
        BuildDto {
            key: key.into(),
            version_id: version_id.into(),
            name: name.into(),
            version: Some("1.21.1".into()),
            loader: Some("1.21.1".into()),
            client: Some("Minecraft".into()),
            loader_version: None,
            game_dir: format!("G/games/{key}"),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
        }
    }

    fn profile(key: &str) -> launcher_shared::ProfileDto {
        launcher_shared::ProfileDto {
            key: key.into(),
            name: key.into(),
            id: String::new(),
            kind: launcher_shared::AccountKind::Offline,
            is_default: false,
            reauth_required: false,
            reauth_reason: None,
        }
    }

    #[test]
    fn a_build_with_its_own_account_starts_with_it_and_asks_none() {
        let profiles = [profile("Alex"), profile("Steve")];
        let mut aero = build("aero", "aero", "Aero");
        assert_eq!(own_profile(&aero, &profiles), None);
        aero.profile = Some("Alex".into());
        assert_eq!(own_profile(&aero, &profiles).as_deref(), Some("Alex"));
        aero.profile = Some("Gone".into());
        assert_eq!(own_profile(&aero, &profiles), None, "a deleted account: Play asks as usual");
    }

    #[test]
    fn subtitles_and_icons_follow_the_original() {
        assert_eq!(build_subtitle(&build("a", "a", "A")), "Minecraft 1.21.1");
        let mut bare = build("b", "b", "B");
        bare.client = None;
        bare.version = None;
        assert_eq!(build_subtitle(&bare), "");
        assert_eq!(loader_icon(Some("Minecraft")), "layers");
        assert_eq!(loader_icon(Some("neoforge-21.1.77")), "construction");
        assert_eq!(loader_icon(Some("forge")), "build");
        assert_eq!(loader_icon(Some("fabric-loader")), "extension");
        assert_eq!(loader_icon(Some("quilt")), "grid_view");
        assert_eq!(loader_icon(None), "videogame_asset");
    }

    #[test]
    fn a_card_tags_its_loader_and_does_not_name_it_twice() {
        let mut neo = build("a", "a", "A");
        neo.loader = Some("neoforge-21.1.252".into());
        neo.client = Some("TensaCraft".into());
        assert_eq!(loader_tag(&neo), ("NeoForge".to_string(), TagTone::NeoForge));
        assert_eq!(card_subtitle(&neo), "TensaCraft 1.21.1", "a client of its own stays");
        let mut fabric = build("b", "b", "B");
        fabric.loader = Some("fabric-loader-0.16.9-1.21.1".into());
        fabric.client = Some("Fabric".into());
        assert_eq!(loader_tag(&fabric).1, TagTone::Fabric);
        assert_eq!(card_subtitle(&fabric), "1.21.1", "the tag already says Fabric");
        let vanilla = build("c", "c", "C");
        assert_eq!(loader_tag(&vanilla), ("Minecraft".to_string(), TagTone::Vanilla));
        assert_eq!(card_subtitle(&vanilla), "1.21.1");
        let mut forge = build("d", "d", "D");
        forge.loader = None;
        forge.client = Some("forge".into());
        assert_eq!(loader_tag(&forge).1, TagTone::Forge, "the client names it when the loader does not");
    }

    #[test]
    fn new_names_are_checked_like_the_backend() {
        let builds = [build("aero", "aero", "Aero")];
        assert_eq!(check_new_name("  Nova  ", &builds), Ok("Nova".to_string()));
        assert_eq!(check_new_name("   ", &builds), Err(NameProblem::Empty));
        assert_eq!(check_new_name(" AERO ", &builds), Err(NameProblem::Taken));
        assert_eq!(NameProblem::Empty.key(), "empty_version_name");
        assert_eq!(NameProblem::Taken.key(), "version_exists");
        let taken = vec!["Minecraft 1.21.1".to_string(), "minecraft 1.21.1 (2)".to_string()];
        assert_eq!(unique_name("Minecraft 1.21.1", &taken), "Minecraft 1.21.1 (3)");
        assert_eq!(unique_name(" Aero ", &[]), "Aero");
    }

    #[test]
    fn shortcut_ids_find_builds_by_key_then_version_id() {
        let builds = [build("my pack", "my_pack", "My pack"), build("aero", "aero", "Aero")];
        assert_eq!(find_build(&builds, "aero").map(|b| b.name.as_str()), Some("Aero"));
        assert_eq!(find_build(&builds, "my_pack").map(|b| b.key.as_str()), Some("my pack"));
        assert!(find_build(&builds, "ghost").is_none());
    }

    #[test]
    fn a_second_click_while_launching_is_ignored() {
        assert_eq!(next_launch_step(true, false, false, false, 1), LaunchStep::Ignore);
        assert_eq!(next_launch_step(false, true, false, false, 1), LaunchStep::ConfirmDuplicate);
        assert_eq!(next_launch_step(false, true, true, false, 1), LaunchStep::Launch);
        assert_eq!(next_launch_step(false, false, false, true, 2), LaunchStep::PickProfile);
        assert_eq!(next_launch_step(false, false, false, true, 0), LaunchStep::NeedProfile);
        assert_eq!(
            next_launch_step(false, false, false, false, 0),
            LaunchStep::Launch,
            "the backend reports a missing profile"
        );
    }

    #[test]
    fn catalog_pages_and_dates() {
        assert_eq!(catalog_shown(900, 1), CATALOG_PAGE);
        assert_eq!(catalog_shown(900, 2), 160);
        assert_eq!(catalog_shown(100, 2), 100);
        assert_eq!(catalog_shown(0, 1), 0);
        assert_eq!(release_date(Some("2024-08-08T12:24:45+00:00")).as_deref(), Some("2024-08-08"));
        assert_eq!(release_date(Some("junk")), None);
        assert_eq!(release_date(None), None);
    }

    #[test]
    fn the_memory_slider_always_has_a_valid_range() {
        let info = |total, max, rec| MemoryInfo {
            total_gb: total,
            min_heap_gb: 1,
            max_heap_gb: max,
            recommended_heap_gb: rec,
        };
        assert_eq!(ram_slider(&info(16, 14, 6)), (1.0, 14.0, 6.0));
        assert_eq!(ram_slider(&info(1, 1, 1)), (1.0, 2.0, 1.0), "a one-value range still slides");
        assert_eq!(ram_slider(&info(0, 0, 0)), (1.0, 2.0, 1.0));
    }

    #[test]
    fn catalog_rows_offer_unique_default_names() {
        let builds = [build("minecraft_1_21_1", "minecraft_1_21_1", "Minecraft 1.21.1")];
        assert_eq!(default_build_name("Minecraft 1.21.1", &builds), "Minecraft 1.21.1 (2)");
        assert_eq!(default_build_name("Minecraft 1.20.1", &builds), "Minecraft 1.20.1");
        assert_eq!(default_build_name("Fabric 1.21.1", &builds), "Fabric 1.21.1");
    }
}
