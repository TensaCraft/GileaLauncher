//! Ordering and grouping of loader builds: numbers compare as numbers, a release
//! outranks its prereleases (`rc` > `pre` > `beta` > `alpha` > `snapshot`), and builds are offered
//! from the newest stable family.

use std::cmp::Reverse;

use launcher_shared::LoaderBuild;

const PRERELEASE: [(&str, u8); 5] = [("snapshot", 1), ("alpha", 2), ("beta", 3), ("pre", 4), ("rc", 5)];
const RELEASE_RANK: u8 = 10;

/// Comparable form of a version: its numbers, the rank of its prerelease word (a release is 10)
/// and the numbers after that word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SortKey {
    base: Vec<u64>,
    rank: u8,
    pre: Vec<u64>,
}

/// Stable builds group by their first two numbers; prereleases by the first three and the word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Family {
    Stable(Vec<u64>),
    Unstable(Vec<u64>, String),
}

enum Token {
    Number(u64),
    Word(String),
}

fn tokens(version: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, out: &mut Vec<Token>| {
        if current.is_empty() {
            return;
        }
        let token = match current.parse::<u64>() {
            Ok(n) => Token::Number(n),
            Err(_) => Token::Word(current.to_lowercase()),
        };
        out.push(token);
        current.clear();
    };
    for c in version.chars() {
        let same_kind = current.chars().next().is_none_or(|p| p.is_ascii_digit() == c.is_ascii_digit());
        if !c.is_ascii_alphanumeric() || !same_kind {
            flush(&mut current, &mut out);
        }
        if c.is_ascii_alphanumeric() {
            current.push(c);
        }
    }
    flush(&mut current, &mut out);
    out
}

/// (numbers before the prerelease word, its rank and word, numbers after it).
fn split(version: &str) -> (Vec<u64>, Option<(u8, String)>, Vec<u64>) {
    let (mut base, mut word, mut pre) = (Vec::new(), None, Vec::new());
    for token in tokens(version) {
        match token {
            Token::Number(n) if word.is_none() => base.push(n),
            Token::Number(n) => pre.push(n),
            Token::Word(w) if word.is_none() => {
                if let Some((_, rank)) = PRERELEASE.iter().find(|(name, _)| w == *name) {
                    word = Some((*rank, w));
                }
            }
            Token::Word(_) => {}
        }
    }
    (base, word, pre)
}

pub fn sort_key(version: &str) -> SortKey {
    let (base, word, pre) = split(version);
    SortKey { base, rank: word.map_or(RELEASE_RANK, |(rank, _)| rank), pre }
}

pub fn is_prerelease(version: &str) -> bool {
    split(version).1.is_some()
}

pub fn family(version: &str) -> Family {
    let (base, word, _) = split(version);
    match word {
        Some((_, word)) => Family::Unstable(base.into_iter().take(3).collect(), word),
        None => Family::Stable(base.into_iter().take(2).collect()),
    }
}

/// Builds to offer, newest first: the stable builds of the newest stable family and, with
/// `unstable`, the prereleases newer than its newest build (all prereleases when there is no
/// stable build).
pub fn offered_builds(builds: &[LoaderBuild], unstable: bool) -> Vec<LoaderBuild> {
    let mut sorted = builds.to_vec();
    sorted.sort_by_key(|b| Reverse(sort_key(&b.version)));
    sorted.dedup_by(|a, b| a.version == b.version);
    let Some(top) = sorted.iter().find(|b| b.stable).cloned() else {
        return if unstable { sorted } else { Vec::new() };
    };
    let (top_family, top_key) = (family(&top.version), sort_key(&top.version));
    sorted
        .into_iter()
        .filter(|b| {
            if b.stable {
                family(&b.version) == top_family
            } else {
                unstable && sort_key(&b.version) > top_key
            }
        })
        .collect()
}

/// The builds of one Minecraft version to offer (Forge, NeoForge), newest first — prereleases
/// only with `unstable` — and the default: `recommended` when it is on offer, else the newest
/// stable build, else the newest. `None` when nothing is on offer.
pub fn game_builds(
    versions: &[String],
    recommended: Option<&str>,
    unstable: bool,
) -> Option<(Vec<LoaderBuild>, String)> {
    let mut builds: Vec<LoaderBuild> = versions
        .iter()
        .map(|version| LoaderBuild { version: version.clone(), stable: !is_prerelease(version) })
        .filter(|build| build.stable || unstable)
        .collect();
    builds.sort_by_key(|build| Reverse(sort_key(&build.version)));
    builds.dedup_by(|a, b| a.version == b.version);
    let default = recommended
        .filter(|r| builds.iter().any(|build| build.version == *r))
        .map(str::to_string)
        .or_else(|| builds.iter().find(|build| build.stable).map(|build| build.version.clone()))
        .or_else(|| builds.first().map(|build| build.version.clone()))?;
    Some((builds, default))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(version: &str, stable: bool) -> LoaderBuild {
        LoaderBuild { version: version.into(), stable }
    }

    #[test]
    fn versions_sort_like_the_original() {
        let mut list =
            vec!["0.16.9", "0.16.10", "0.17.0-beta.1", "0.17.0-rc.1", "0.17.0", "0.15.11", "1.0.0-alpha.2"];
        list.sort_by_key(|v| Reverse(sort_key(v)));
        assert_eq!(
            list,
            ["1.0.0-alpha.2", "0.17.0", "0.17.0-rc.1", "0.17.0-beta.1", "0.16.10", "0.16.9", "0.15.11"]
        );
        assert!(
            is_prerelease("0.27.0-Beta.1") && is_prerelease("24w33a-snapshot") && !is_prerelease("0.16.9")
        );
        assert_eq!(family("0.16.9"), family("0.16.14"));
        assert_ne!(family("0.16.9"), family("0.17.0"));
        assert_ne!(family("0.17.0-beta.1"), family("0.17.0"));
    }

    #[test]
    fn offered_builds_follow_the_newest_stable_family() {
        let builds = [
            b("0.16.8", true),
            b("0.17.0-beta.1", false),
            b("0.16.9", true),
            b("0.15.11", true),
            b("0.16.9", true),
        ];
        let names = |list: Vec<LoaderBuild>| list.into_iter().map(|b| b.version).collect::<Vec<_>>();
        assert_eq!(names(offered_builds(&builds, false)), ["0.16.9", "0.16.8"]);
        assert_eq!(names(offered_builds(&builds, true)), ["0.17.0-beta.1", "0.16.9", "0.16.8"]);
        let only_betas = [b("0.1.0-beta.2", false), b("0.1.0-beta.1", false)];
        assert!(offered_builds(&only_betas, false).is_empty());
        assert_eq!(names(offered_builds(&only_betas, true)), ["0.1.0-beta.2", "0.1.0-beta.1"]);
        assert!(offered_builds(&[], true).is_empty());
    }

    #[test]
    fn game_builds_prefer_the_recommended_build() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let forge = v(&["47.4.10", "47.4.23", "47.3.0"]);
        let (builds, default) = game_builds(&forge, Some("47.4.10"), false).unwrap();
        let names: Vec<&str> = builds.iter().map(|b| b.version.as_str()).collect();
        assert_eq!(names, ["47.4.23", "47.4.10", "47.3.0"]);
        assert_eq!(default, "47.4.10");
        assert_eq!(
            game_builds(&forge, Some("40.0.0"), false).unwrap().1,
            "47.4.23",
            "a recommendation that is not on offer"
        );
        let neo = v(&["21.4.0-beta", "21.4.1-beta"]);
        assert!(game_builds(&neo, None, false).is_none(), "only betas without unstable");
        let (builds, default) = game_builds(&neo, None, true).unwrap();
        assert_eq!((builds.len(), default.as_str(), builds[0].stable), (2, "21.4.1-beta", false));
        let mixed = v(&["21.1.77", "21.1.78-beta"]);
        assert_eq!(
            game_builds(&mixed, None, true).unwrap().1,
            "21.1.77",
            "the newest stable build is the default"
        );
    }
}
