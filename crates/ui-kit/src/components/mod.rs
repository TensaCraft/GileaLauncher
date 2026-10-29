pub mod appearance;
pub mod brand;
pub mod button;
pub mod card;
pub mod choice;
pub mod code;
pub mod dialog;
pub mod feedback;
pub mod field;
pub mod float;
pub mod icon;
pub mod layout;
pub mod loader_mark;
pub mod menu;
pub mod nudge;
pub mod select;
pub mod slider;
pub mod tag;
pub mod toast;
pub mod toggle;
pub mod tooltip;

pub use appearance::{AccentSwatches, FontTiles, accent_input, chosen_preset, field_follows, live_accent};
pub use button::{ActionGap, ActionGroup, ActionTone, Button, IconAction, Size, Variant};
pub use card::{BuildCard, CardPlayStyle, FALLBACK_IMAGE, card_image};
pub use choice::{ChoiceCards, ChoiceOption};
pub use code::CodeEditor;
pub use dialog::{ConfirmDialog, Dialog, DialogFooter, DialogTone};
pub use feedback::{EmptyState, ProgressBar, Skeleton, progress_percent};
pub use field::{Field, PathField, TextArea, TextInput};
pub use float::{MenuPlace, Rect, TipLayer, TipSide, menu_place, tip_place, viewport};
pub use icon::Icon;
pub use layout::{ChipDef, ChipTabs, ListRow, NavEntry, Section, SettingRow, SettingsNav, TabDef, Tabs};
pub use menu::{ContextMenu, ContextMenuHost, MenuEntry, provide_context_menu, use_context_menu};
pub use nudge::{NudgeLayer, NudgeMark, NudgePlace, NudgeSide, nudge_place};
pub use select::{Select, SelectOption};
pub use slider::Slider;
pub use tag::{Tag, TagTone};
pub use toast::{ToastItem, Toaster, Toasts, provide_toasts, use_toasts};
pub use toggle::{Checkbox, SegOption, Segmented, Switch};
pub use tooltip::{Tooltip, TooltipPlacement};

#[cfg(test)]
mod styles_guard {
    /// The accent's colours, as they were written before the design followed the chosen accent.
    const ACCENT: [&str; 12] = [
        "23, 195, 162",
        "123, 227, 210",
        "63, 224, 192",
        "64, 190, 110",
        "63, 224, 138",
        "#17c3a2",
        "#3fe0c0",
        "#0f8d77",
        "#7be3d2",
        "#22d6b2",
        "#13b494",
        "#34e3c0",
    ];

    /// A dark colour with the default accent's green-blue cast: one of the design's tints.
    fn dark_tint(r: u8, g: u8, b: u8) -> bool {
        r.max(g).max(b) < 90 && g >= r.saturating_add(6) && b >= r.saturating_add(3)
    }

    /// The colours `line` writes as `rgb(a)(r, g, b…)` or `#rrggbb`.
    fn colours(line: &str) -> Vec<(u8, u8, u8)> {
        let mut found = Vec::new();
        for (i, _) in line.match_indices("rgb") {
            let rest = &line[i..];
            let Some(open) = rest.find('(') else { continue };
            if let Some(close) = rest[open..].find(')') {
                let parts: Vec<u8> = rest[open + 1..open + close]
                    .split(',')
                    .take(3)
                    .filter_map(|p| p.trim().parse().ok())
                    .collect();
                if let [r, g, b] = parts[..] {
                    found.push((r, g, b));
                }
            }
        }
        for (i, _) in line.match_indices('#') {
            let hex = line.get(i + 1..i + 7).unwrap_or_default();
            if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
                found.push((channel(0), channel(2), channel(4)));
            }
        }
        found
    }

    fn named(css: &str, allowed: impl Fn(&str) -> bool) -> Vec<&str> {
        css.lines()
            .filter(|line| {
                ACCENT.iter().any(|c| line.to_lowercase().contains(c))
                    || colours(line).into_iter().any(|(r, g, b)| dark_tint(r, g, b))
            })
            .filter(|line| !allowed(line.trim()))
            .collect()
    }

    /// Every colour of the accent and of its dark tints comes from a variable, so the design
    /// follows the chosen accent; only `tokens.css` names the defaults, and the footer's cube
    /// keeps its own green.
    #[test]
    fn no_css_names_the_accent_by_value() {
        // The guard sees what it guards against.
        for written in [
            ".x { color: #03241c; }",
            ".x { background: rgba(22, 40, 36, 0.94); }",
            ".x { --c: 23, 195, 162; }",
        ] {
            assert_eq!(named(written, |_| false), vec![written], "{written}");
        }
        let tokens = include_str!("../../styles/tokens.css");
        let defaults = |line: &str| line.starts_with("--");
        assert_eq!(named(tokens, defaults), Vec::<&str>::new(), "tokens.css");
        for (file, css) in [
            ("components.css", include_str!("../../styles/components.css")),
            ("base.css", include_str!("../../styles/base.css")),
            ("shell.css", include_str!("../../../launcher-ui/styles/shell.css")),
        ] {
            let cube = |line: &str| line.starts_with(".footer-mark__cube");
            assert_eq!(named(css, cube), Vec::<&str>::new(), "{file}");
        }
    }
}
