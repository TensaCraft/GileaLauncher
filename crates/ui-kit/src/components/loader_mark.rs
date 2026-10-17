//! Marks of the plain game and its mod loaders, drawn where a Material icon would be
//! (`loader:fabric`): line drawings of the icon set's weight, each in its loader's colour.

/// A mark on a 24x24 grid: its lines, and the shapes filled faintly under them.
pub struct LoaderMark {
    pub strokes: &'static [&'static str],
    pub fills: &'static [&'static str],
}

/// The mark `name` (`loader:quilt`) means, if it is one.
pub fn loader_mark(name: &str) -> Option<&'static LoaderMark> {
    match name.strip_prefix("loader:")? {
        "vanilla" => Some(&VANILLA),
        "fabric" => Some(&FABRIC),
        "forge" => Some(&FORGE),
        "neoforge" => Some(&NEOFORGE),
        "quilt" => Some(&QUILT),
        _ => None,
    }
}

/// A block of grass: a cube with its top lit.
const VANILLA: LoaderMark = LoaderMark {
    strokes: &["M12 2.8 20.2 7.4v9.2L12 21.2l-8.2-4.6V7.4Z", "M3.8 7.4 12 12l8.2-4.6M12 12v9.2"],
    fills: &["M12 2.8 20.2 7.4 12 12 3.8 7.4Z"],
};

/// A strip of cloth, rolled up at one end.
const FABRIC: LoaderMark = LoaderMark {
    strokes: &[
        "M13.2 5.2 4.1 14.3a1.6 1.6 0 0 0 0 2.2l3.4 3.4a1.6 1.6 0 0 0 2.2 0l9.1-9.1",
        "M13.2 5.2a4 4 0 0 1 5.6 5.6 2 2 0 0 1-2.8-2.8",
    ],
    fills: &[],
};

/// An anvil.
const FORGE: LoaderMark = LoaderMark {
    strokes: &[
        "M6 6h15v3h-3.5c-1.5 0-2.5 1.2-2.5 2.6v1.2c0 1 .7 1.8 1.7 2.1L18 15.5V18H6v-2.5l1.3-.6c1-.3 1.7-1.1 1.7-2.1v-1.2C9 10 8 9.4 6.6 9.2 4.6 8.8 3 8 2 7l4-1Z",
    ],
    fills: &[],
};

/// A fox's head.
const NEOFORGE: LoaderMark = LoaderMark {
    strokes: &[
        "M4 3.5 8.5 8.5h7L20 3.5l.5 7.5c0 4.3-3.7 8.2-8.5 10.5C7.2 19.2 3.5 15.3 3.5 11Z",
        "M8.3 12.6l1.6 1M15.7 12.6l-1.6 1",
    ],
    fills: &["M11 17h2l-1 1.1Z"],
};

/// Four patches, the last one turned.
const QUILT: LoaderMark = LoaderMark {
    strokes: &[
        "M4.5 3h4A1.5 1.5 0 0 1 10 4.5v4A1.5 1.5 0 0 1 8.5 10h-4A1.5 1.5 0 0 1 3 8.5v-4A1.5 1.5 0 0 1 4.5 3Z",
        "M15.5 3h4A1.5 1.5 0 0 1 21 4.5v4a1.5 1.5 0 0 1-1.5 1.5h-4A1.5 1.5 0 0 1 14 8.5v-4A1.5 1.5 0 0 1 15.5 3Z",
        "M4.5 14h4a1.5 1.5 0 0 1 1.5 1.5v4A1.5 1.5 0 0 1 8.5 21h-4A1.5 1.5 0 0 1 3 19.5v-4A1.5 1.5 0 0 1 4.5 14Z",
        "M16.6 13.4a1.3 1.3 0 0 1 1.8 0l2.9 2.9a1.3 1.3 0 0 1 0 1.8l-2.9 2.9a1.3 1.3 0 0 1-1.8 0l-2.9-2.9a1.3 1.3 0 0 1 0-1.8Z",
    ],
    fills: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_and_each_loader_have_their_marks() {
        for name in ["vanilla", "fabric", "forge", "neoforge", "quilt"] {
            let mark = loader_mark(&format!("loader:{name}")).unwrap_or_else(|| panic!("{name} has no mark"));
            assert!(!mark.strokes.is_empty() && mark.strokes.iter().all(|d| d.starts_with('M')), "{name}");
            assert!(mark.fills.iter().all(|d| d.starts_with('M')), "{name}");
        }
        assert!(loader_mark("loader:unknown").is_none());
        assert!(loader_mark("layers").is_none(), "a Material icon is no loader mark");
    }
}
