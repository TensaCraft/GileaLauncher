pub mod build_settings;
pub mod builds;
pub mod components;
pub mod content;
pub mod create;
pub mod home;
pub mod kit;
pub mod modpacks;
pub mod profiles;
pub mod settings;
pub mod setup;

#[cfg(test)]
mod tests {
    /// The declarations of the rule whose selector is exactly `selector`.
    fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let line = css
            .lines()
            .find(|line| line.split('{').next().is_some_and(|s| s.trim() == selector))
            .unwrap_or_else(|| panic!("no rule {selector}"));
        &line[line.find('{').unwrap() + 1..line.rfind('}').unwrap()]
    }

    #[test]
    fn pages_line_up_with_the_header_title() {
        let css = include_str!("../../styles/shell.css");
        let side = |selector: &str| {
            let content = rule(css, selector);
            let padding = content.split(';').find_map(|d| d.trim().strip_prefix("padding:")).unwrap_or("0");
            padding.split_whitespace().nth(1).unwrap_or("0").to_string()
        };
        assert_eq!(side(".header"), "20px");
        // The content area has the header's inset: on the left as it is, on the right less the
        // scroll bar's place it always keeps (the shell measures it into `--gutter`).
        let content = rule(css, ".content");
        assert!(content.contains("scrollbar-gutter: stable"), "a page that comes to scroll keeps its width");
        let padding = content.split(';').find_map(|d| d.trim().strip_prefix("padding:")).unwrap().trim();
        assert!(padding.ends_with(" 20px"), "the left inset is the header's: {padding}");
        assert!(
            padding.contains("calc(20px - var(--gutter, 0px))"),
            "the right one gives the bar's place back: {padding}"
        );
        for page in [".home", ".builds", ".create", ".profiles"] {
            assert_eq!(side(page), "0", "{page} adds no inset of its own");
        }
    }

    #[test]
    fn header_controls_share_the_md_height() {
        // Every control in the page header (back button, page actions, ops indicator) is md
        // height, so they line up instead of the back button (sm) looking shorter.
        let css = include_str!("../../styles/shell.css");
        assert!(rule(css, ".header .btn").contains("height: var(--h-md)"));
        assert!(rule(css, ".header .btn--icon").contains("width: var(--h-md)"));
    }

    #[test]
    fn the_window_controls_keep_their_place_in_the_header() {
        // The window's own buttons float above every layer (a dialog's too) at the header's right
        // end; the page actions stop short of them.
        let css = include_str!("../../styles/shell.css");
        let bar = rule(css, ".window-controls");
        assert!(bar.contains("position: fixed") && bar.contains("width: var(--window-bar)"), "{bar}");
        assert!(rule(css, ".header__actions").contains("margin-right: calc(var(--window-bar) + 12px)"));
    }

    #[test]
    fn line_rows_in_a_section_stay_flush() {
        // A section spaces out free content; rows drawn as lines join up like setting rows.
        let css = include_str!("../../styles/shell.css");
        for row in [".activity__row", ".activity__empty"] {
            assert!(rule(css, row).contains("margin: 0;"), "{row} needs margin: 0");
        }
    }
}
