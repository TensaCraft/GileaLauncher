//! Launcher version parsing and ordering (the original updater's rules).

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreToken {
    Num(u64),
    Text(String),
}

/// `v`-prefix and `+build` metadata are ignored; missing numeric parts count as zero.
#[derive(Debug, Clone)]
pub struct Version {
    base: Vec<u64>,
    pre: Vec<PreToken>,
    text: String,
}

impl Version {
    pub fn parse(raw: &str) -> Option<Version> {
        let trimmed = raw.trim();
        let s = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
        let s = s.split('+').next().unwrap_or_default();
        if s.is_empty() {
            return None;
        }
        let (base_part, pre_part) = match s.split_once('-') {
            Some((b, p)) => (b, Some(p)),
            None => (s, None),
        };
        let mut base = Vec::new();
        for token in base_part.split('.') {
            if token.is_empty() {
                base.push(0);
            } else if token.chars().all(|c| c.is_ascii_digit()) {
                base.push(token.parse().ok()?);
            } else {
                return None;
            }
        }
        let pre = pre_part
            .map(|p| {
                p.replace('-', ".")
                    .split('.')
                    .filter(|t| !t.is_empty())
                    .map(|t| match t.parse::<u64>() {
                        Ok(n) if t.chars().all(|c| c.is_ascii_digit()) => PreToken::Num(n),
                        _ => PreToken::Text(t.to_ascii_lowercase()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Version { base, pre, text: s.to_string() })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// Normalised text: no `v` prefix, no `+build`.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.base.len().max(other.base.len());
        for i in 0..len {
            let a = self.base.get(i).copied().unwrap_or(0);
            let b = other.base.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let ord = match (a, b) {
                        (PreToken::Num(x), PreToken::Num(y)) => x.cmp(y),
                        (PreToken::Text(x), PreToken::Text(y)) => x.cmp(y),
                        (PreToken::Num(_), PreToken::Text(_)) => Ordering::Less,
                        (PreToken::Text(_), PreToken::Num(_)) => Ordering::Greater,
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s} must parse"))
    }

    #[test]
    fn ordering_follows_the_original_rules() {
        let cases = [
            ("1.0.0", "0.9.9", Greater),
            ("v1.2", "1.2.0", Equal),
            ("V1.2.0", "1.2", Equal),
            ("1.2.0+build.5", "1.2.0", Equal),
            ("1.10.0", "1.9.9", Greater),
            ("1.2.0", "1.2.0-rc.1", Greater),
            ("1.2.0-beta.10", "1.2.0-beta.9", Greater),
            ("1.2.0-beta.2", "1.2.0-alpha.9", Greater),
            ("1.2.0-beta", "1.2.0-beta.1", Less),
            ("1.2.0-1", "1.2.0-alpha", Less),
            ("1.2.0-rc-2", "1.2.0-rc.2", Equal),
            ("1.2.0-RC.1", "1.2.0-rc.1", Equal),
            ("0.2.0", "0.1.9", Greater),
            ("2", "1.99.99", Greater),
            ("1..2", "1.0.2", Equal),
            ("1.2.3.4", "1.2.3", Greater),
            ("0.1.0", "0.1.0", Equal),
            ("1.2.0-beta.1+sha", "1.2.0-beta.1", Equal),
            ("10.0.0", "9.0.0", Greater),
            ("1.0.0-alpha.beta", "1.0.0-alpha.1", Greater),
        ];
        for (a, b, expected) in cases {
            assert_eq!(v(a).cmp(&v(b)), expected, "{a} vs {b}");
            assert_eq!(v(b).cmp(&v(a)), expected.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn invalid_versions_are_rejected() {
        for bad in ["", "v", "latest", "1.x", "abc-1", "+build", "99999999999999999999999"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn prerelease_flag_and_normalised_text() {
        assert!(v("1.0.0-beta.1").is_prerelease());
        assert!(!v("v1.0.0+meta").is_prerelease());
        assert_eq!(v("v1.0.0+meta").as_str(), "1.0.0");
        assert_eq!(v(" v0.2.0 ").as_str(), "0.2.0");
    }
}
