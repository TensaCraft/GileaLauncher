//! Screenshots shared by the Screenshots page and a build's Screenshots tab: what is shown (search,
//! build, sort), the viewer, and what can be done to a screenshot.

pub mod actions;
pub mod tile;
pub mod viewer;

use launcher_shared::{BuildShots, ScreenshotDto};

/// How the shown screenshots are ordered within their build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShotSort {
    #[default]
    Newest,
    Oldest,
    Name,
    Size,
}

impl ShotSort {
    pub const ALL: [ShotSort; 4] = [ShotSort::Newest, ShotSort::Oldest, ShotSort::Name, ShotSort::Size];

    pub fn id(self) -> &'static str {
        match self {
            ShotSort::Newest => "newest",
            ShotSort::Oldest => "oldest",
            ShotSort::Name => "name",
            ShotSort::Size => "size",
        }
    }

    pub fn from_id(id: &str) -> ShotSort {
        ShotSort::ALL.into_iter().find(|s| s.id() == id).unwrap_or_default()
    }

    pub fn label_key(self) -> &'static str {
        match self {
            ShotSort::Newest => "shots_sort_newest",
            ShotSort::Oldest => "shots_sort_oldest",
            ShotSort::Name => "shots_sort_name",
            ShotSort::Size => "shots_sort_size",
        }
    }

    fn order(self, shots: &mut [ScreenshotDto]) {
        match self {
            ShotSort::Newest => {
                shots.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.name.cmp(&b.name)))
            }
            ShotSort::Oldest => {
                shots.sort_by(|a, b| a.modified_ms.cmp(&b.modified_ms).then_with(|| a.name.cmp(&b.name)))
            }
            ShotSort::Name => shots.sort_by_cached_key(|s| s.name.to_lowercase()),
            ShotSort::Size => shots.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
        }
    }
}

/// The groups the page shows: in the builds' `order` (keys; others after), only build `only` when
/// given, only screenshots whose name has `query` (any case), each sorted by `sort`; empty groups
/// are left out.
pub fn shown_groups(
    all: &[BuildShots],
    order: &[String],
    query: &str,
    only: Option<&str>,
    sort: ShotSort,
) -> Vec<BuildShots> {
    let query = query.trim().to_lowercase();
    let mut groups: Vec<BuildShots> = all
        .iter()
        .filter(|g| only.is_none_or(|key| g.key == key))
        .map(|g| {
            let mut shots: Vec<ScreenshotDto> = g
                .shots
                .iter()
                .filter(|s| query.is_empty() || s.name.to_lowercase().contains(&query))
                .cloned()
                .collect();
            sort.order(&mut shots);
            BuildShots { key: g.key.clone(), shots }
        })
        .filter(|g| !g.shots.is_empty())
        .collect();
    groups.sort_by_key(|g| order.iter().position(|k| *k == g.key).unwrap_or(usize::MAX));
    groups
}

/// The screenshots of `groups` one after another, as the viewer steps through them.
pub fn flat(groups: &[BuildShots]) -> Vec<(String, ScreenshotDto)> {
    groups.iter().flat_map(|g| g.shots.iter().map(|s| (g.key.clone(), s.clone()))).collect()
}

/// The place `delta` away from `at` among `len`, when there is one.
pub fn step(at: usize, len: usize, delta: isize) -> Option<usize> {
    at.checked_add_signed(delta).filter(|next| *next < len)
}

/// A file name as its name and its extension (with the dot): the rename field shows the name.
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    }
}

/// How many screenshots and bytes `shots` holds.
pub fn totals(shots: &[ScreenshotDto]) -> (usize, u64) {
    (shots.len(), shots.iter().map(|s| s.size).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(name: &str, size: u64, at: u64) -> ScreenshotDto {
        ScreenshotDto {
            name: name.into(),
            size,
            modified_ms: Some(at),
            src: String::new(),
            thumb: String::new(),
        }
    }

    fn names(groups: &[BuildShots]) -> Vec<(String, Vec<String>)> {
        groups.iter().map(|g| (g.key.clone(), g.shots.iter().map(|s| s.name.clone()).collect())).collect()
    }

    fn all() -> Vec<BuildShots> {
        vec![
            BuildShots { key: "zeta".into(), shots: vec![shot("Base.png", 30, 3), shot("cave.png", 10, 1)] },
            BuildShots {
                key: "aero".into(),
                shots: vec![shot("sunset.png", 20, 5), shot("Airship.png", 50, 2)],
            },
        ]
    }

    #[test]
    fn groups_follow_the_builds_order_and_their_own_sort() {
        let order = vec!["aero".to_string(), "zeta".to_string()];
        let newest = shown_groups(&all(), &order, "", None, ShotSort::Newest);
        assert_eq!(
            names(&newest),
            [
                ("aero".into(), vec!["sunset.png".into(), "Airship.png".into()]),
                ("zeta".into(), vec!["Base.png".into(), "cave.png".into()])
            ]
        );
        let by_name = shown_groups(&all(), &order, "", None, ShotSort::Name);
        assert_eq!(by_name[0].shots[0].name, "Airship.png", "any case");
        assert_eq!(shown_groups(&all(), &order, "", None, ShotSort::Size)[0].shots[0].name, "Airship.png");
        assert_eq!(shown_groups(&all(), &order, "", None, ShotSort::Oldest)[1].shots[0].name, "cave.png");
        let unknown = shown_groups(&all(), &[], "", None, ShotSort::Newest);
        assert_eq!(unknown[0].key, "zeta", "builds not in the order keep theirs");
    }

    #[test]
    fn search_and_build_leave_out_what_does_not_match() {
        let order = vec!["aero".to_string(), "zeta".to_string()];
        let found = shown_groups(&all(), &order, "  AIR ", None, ShotSort::Newest);
        assert_eq!(names(&found), [("aero".into(), vec!["Airship.png".into()])], "empty groups are left out");
        let zeta = shown_groups(&all(), &order, "", Some("zeta"), ShotSort::Newest);
        assert_eq!(zeta.len(), 1);
        assert!(shown_groups(&all(), &order, "nothing", None, ShotSort::Newest).is_empty());
        let in_order: Vec<String> = flat(&shown_groups(&all(), &order, "", None, ShotSort::Newest))
            .into_iter()
            .map(|(key, s)| format!("{key}/{}", s.name))
            .collect();
        assert_eq!(in_order, ["aero/sunset.png", "aero/Airship.png", "zeta/Base.png", "zeta/cave.png"]);
    }

    #[test]
    fn the_viewer_steps_within_the_list() {
        assert_eq!(step(0, 3, 1), Some(1));
        assert_eq!(step(2, 3, 1), None);
        assert_eq!(step(0, 3, -1), None);
        assert_eq!(step(1, 3, -1), Some(0));
        assert_eq!(step(0, 0, 0), None);
    }

    #[test]
    fn a_name_is_edited_without_its_extension() {
        assert_eq!(split_name("2026-10-03_17.21.44.png"), ("2026-10-03_17.21.44", ".png"));
        assert_eq!(split_name("noext"), ("noext", ""));
        assert_eq!(split_name(".png"), (".png", ""));
        assert_eq!(totals(&[shot("a", 3, 0), shot("b", 4, 0)]), (2, 7));
        assert_eq!(ShotSort::from_id("size"), ShotSort::Size);
        assert_eq!(ShotSort::from_id("?"), ShotSort::Newest);
    }
}
