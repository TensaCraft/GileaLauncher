use leptos::prelude::*;

use super::icon::Icon;
use crate::sound;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

impl Variant {
    pub fn class(self) -> &'static str {
        match self {
            Variant::Primary => "btn--primary",
            Variant::Secondary => "btn--secondary",
            Variant::Ghost => "btn--ghost",
            Variant::Danger => "btn--danger",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

impl Size {
    pub fn class(self) -> &'static str {
        match self {
            Size::Sm => "btn--sm",
            Size::Md => "",
            Size::Lg => "btn--lg",
        }
    }

    /// The same size for fields and selects, so a row of controls lines up.
    pub fn control_class(self) -> &'static str {
        match self {
            Size::Sm => "control--sm",
            Size::Md => "",
            Size::Lg => "control--lg",
        }
    }
}

#[component]
pub fn Button(
    #[prop(optional)] variant: Variant,
    #[prop(optional)] size: Size,
    #[prop(optional, into)] icon: Option<String>,
    #[prop(optional)] outlined: bool,
    #[prop(optional, into)] loading: MaybeProp<bool>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    #[prop(optional, into)] title: MaybeProp<String>,
    #[prop(optional, into)] on_click: Option<Callback<()>>,
    #[prop(optional, into)] class: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let icon_only = children.is_none() && icon.is_some();
    let classes = format!(
        "btn {} {} {} {}",
        variant.class(),
        size.class(),
        if icon_only { "btn--icon" } else { "" },
        class
    );
    let busy = move || loading.get().unwrap_or(false);
    let off = move || disabled.get().unwrap_or(false);
    view! {
        <button
            type="button"
            class=classes
            class:is-loading=busy
            class:is-disabled=off
            disabled=off
            data-tip=move || title.get()
            data-tip-side="top"
            aria-label=move || title.get()
            on:click=move |_| {
                if busy() || off() {
                    return;
                }
                sound::play_click();
                if let Some(cb) = on_click {
                    cb.run(());
                }
            }
        >
            {move || (!busy()).then(|| icon.clone().map(|name| view! { <Icon name=name outlined=outlined /> }))}
            {children.map(|c| c())}
        </button>
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionTone {
    #[default]
    Neutral,
    Ok,
    Danger,
    Info,
}

impl ActionTone {
    pub fn class(self) -> &'static str {
        match self {
            ActionTone::Neutral => "",
            ActionTone::Ok => "action--ok",
            ActionTone::Danger => "action--danger",
            ActionTone::Info => "action--info",
        }
    }
}

/// Square icon button used for row actions (play, copy, folder, delete...).
#[component]
pub fn IconAction(
    #[prop(into)] icon: String,
    #[prop(optional)] outlined: bool,
    #[prop(optional)] tone: ActionTone,
    #[prop(into)] title: Signal<String>,
    /// At work: a spinner in place of the icon, no clicks.
    #[prop(optional, into)]
    loading: MaybeProp<bool>,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
    /// Just the icon, as tall as a switch: for a settings row beside its switch.
    #[prop(optional)]
    compact: bool,
    /// A small count on the icon's corner (updates waiting).
    #[prop(optional, into)]
    badge: MaybeProp<String>,
    #[prop(optional, into)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=format!("action {}{}", tone.class(), if compact { " action--compact" } else { "" })
            class:is-loading=move || loading.get().unwrap_or(false)
            disabled=move || disabled.get().unwrap_or(false)
            data-tip=move || title.get()
            data-tip-side="top"
            aria-label=move || title.get()
            on:click=move |ev| {
                ev.stop_propagation();
                // At work: a key press reaches it although the style takes no pointer.
                if loading.get_untracked().unwrap_or(false) {
                    return;
                }
                sound::play_click();
                if let Some(cb) = on_click {
                    cb.run(());
                }
            }
        >
            <Icon name=icon outlined=outlined />
            {move || badge.get().filter(|b| !b.is_empty()).map(|b| view! { <span class="action__badge">{b}</span> })}
        </button>
    }
}

/// Related icon actions joined into one control split in halves (e.g. "Refresh" | "Check for
/// updates"): one edge, a divider between them, each half with its own tooltip.
#[component]
pub fn ActionGroup(children: Children) -> impl IntoView {
    view! { <div class="action-group" role="group">{children()}</div> }
}

/// The place of an `IconAction` a row does not have, so the list's actions stay in columns.
#[component]
pub fn ActionGap() -> impl IntoView {
    view! { <span class="action-gap" aria-hidden="true"></span> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_and_size_classes() {
        assert_eq!(Variant::default().class(), "btn--secondary");
        assert_eq!(Variant::Primary.class(), "btn--primary");
        assert_eq!(Size::default().class(), "");
        assert_eq!(Size::Sm.class(), "btn--sm");
        assert_eq!(ActionTone::Danger.class(), "action--danger");
        assert_eq!(Size::Sm.control_class(), "control--sm");
        assert_eq!(Size::Md.control_class(), "");
        assert_eq!(Size::Lg.control_class(), "control--lg");
    }

    /// The declarations of the first rule whose selector is exactly `selector`.
    fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let start = css
            .lines()
            .scan(0, |offset, line| {
                let at = *offset;
                *offset += line.len() + 1;
                Some((at, line))
            })
            .find(|(_, line)| line.split('{').next().is_some_and(|s| s.trim() == selector))
            .map(|(at, _)| at)
            .unwrap_or_else(|| panic!("no rule {selector}"));
        let body = &css[start..];
        &body[body.find('{').unwrap() + 1..body.find('}').unwrap()]
    }

    /// A dim border lets a dark control read shorter than a bright one beside it: every control
    /// that sits in a row shows the same, clearly visible edge.
    #[test]
    fn controls_in_a_row_show_one_visible_edge() {
        let tokens = include_str!("../../styles/tokens.css");
        assert!(tokens.contains("--line-ctl: rgba(123, 227, 210, 0.26);"));
        let css = include_str!("../../styles/components.css");
        for selector in [".btn--secondary", ".btn--ghost", ".action", ".control", ".seg"] {
            assert!(rule(css, selector).contains("var(--line-ctl)"), "{selector}");
        }
        let shell = include_str!("../../../launcher-ui/styles/shell.css");
        assert!(shell.contains(".ops__btn {") && shell.contains("border: 1px solid var(--line-ctl)"));
    }

    /// Joined actions are one control split in halves: one edge (neighbours overlap by its width),
    /// round corners only at the ends, the hovered half's edge on top.
    #[test]
    fn joined_actions_are_one_control() {
        let css = include_str!("../../styles/components.css");
        assert!(rule(css, ".action-group").contains("display: inline-flex"));
        assert!(rule(css, ".action-group > .action + .action").contains("margin-left: -1px"));
        assert!(
            rule(css, ".action-group > .action:first-child")
                .contains("border-radius: var(--r-sm) 0 0 var(--r-sm)")
        );
        assert!(
            rule(css, ".action-group > .action:last-child")
                .contains("border-radius: 0 var(--r-sm) var(--r-sm) 0")
        );
        assert!(rule(css, ".action-group > .action:hover").contains("z-index: 1"));
    }

    /// An icon action at work shows the buttons' spinner in place of its icon and takes no clicks.
    #[test]
    fn a_busy_icon_action_spins_and_waits() {
        let css = include_str!("../../styles/components.css");
        assert!(rule(css, ".action.is-loading").contains("pointer-events: none"));
        assert!(rule(css, ".action.is-loading .icon").contains("display: none"));
        assert!(css.contains(".btn.is-loading::before, .action.is-loading::before {"));
    }

    /// A row without one of the list's optional actions keeps its place, so each action stays in
    /// its column down the list.
    #[test]
    fn an_action_gap_takes_an_action_s_place() {
        let css = include_str!("../../styles/components.css");
        let gap = rule(css, ".action-gap");
        assert!(gap.contains("width: var(--h-md)") && gap.contains("height: var(--h-md)"));
        assert!(gap.contains("flex: none"));
    }

    #[test]
    fn controls_in_a_row_share_one_height_scale() {
        let tokens = include_str!("../../styles/tokens.css");
        for token in ["--h-sm: 28px;", "--h-md: 36px;", "--h-lg: 40px;"] {
            assert!(tokens.contains(token), "{token}");
        }
        let css = include_str!("../../styles/components.css");
        let expected = [
            (".btn", "height: var(--h-md)"),
            (".btn--sm", "height: var(--h-sm)"),
            (".btn--lg", "height: var(--h-lg)"),
            (".btn--icon", "width: var(--h-md)"),
            (".btn--icon.btn--sm", "width: var(--h-sm)"),
            (".btn--icon.btn--lg", "width: var(--h-lg)"),
            (".action", "height: var(--h-md)"),
            (".control", "height: var(--h-md)"),
            (".control--sm", "height: var(--h-sm)"),
            (".control--lg", "height: var(--h-lg)"),
            (".seg", "height: var(--h-md)"),
            (".chip", "height: var(--h-md)"),
            (".seg__btn", "height: 100%"),
            (".group > .btn", "height: auto"),
        ];
        for (selector, declaration) in expected {
            assert!(rule(css, selector).contains(declaration), "{selector} needs {declaration}");
        }
    }

    #[test]
    fn primary_gradient_covers_the_border_box() {
        // background-origin defaults to padding-box, so a gradient repeats under a
        // transparent border: the last pixel row shows the gradient's other end and the
        // button looks taller/shifted next to its neighbours. `border-box` fixes that.
        let css = include_str!("../../styles/components.css");
        assert!(rule(css, ".btn--primary").contains("border-box"));
        assert!(rule(css, ".btn--primary:hover").contains("border-box"));
    }

    #[test]
    fn ghost_text_buttons_have_a_visible_box() {
        // Text ghost buttons ("Скасувати", "Закрити"...) need a quiet but visible box so they
        // read as buttons; icon-only ghost buttons (header back arrow, dialog close) stay
        // transparent.
        let css = include_str!("../../styles/components.css");
        let ghost = rule(css, ".btn--ghost");
        assert!(ghost.contains("border-color: var(--line-ctl)"));
        assert!(ghost.contains("background:") && !ghost.contains("background: none"));
        let ghost_icon = rule(css, ".btn--ghost.btn--icon");
        assert!(ghost_icon.contains("background: none"));
        assert!(ghost_icon.contains("border-color: transparent"));
    }
}
