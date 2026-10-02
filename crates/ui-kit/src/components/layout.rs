use leptos::prelude::*;

use super::icon::Icon;
use super::tag::TagTone;
use crate::sound;

#[component]
pub fn Section(
    #[prop(into)] icon: String,
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] desc: MaybeProp<String>,
    /// A section of irreversible actions: its icon in the danger colour, as a dangerous dialog's.
    #[prop(optional)]
    danger: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="section">
            <div class="section__head">
                <div class="section__icon" class:section__icon--danger=danger><Icon name=icon outlined=true /></div>
                <div>
                    <h3 class="section__title">{move || title.get()}</h3>
                    {move || desc.get().map(|d| view! { <p class="section__desc">{d}</p> })}
                </div>
            </div>
            {children()}
        </section>
    }
}

#[component]
pub fn SettingRow(
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] desc: MaybeProp<String>,
    #[prop(optional)] stacked: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="row" class:row--stack=stacked>
            <div class="row__text">
                <div class="row__title">{move || title.get()}</div>
                {move || desc.get().map(|d| view! { <div class="row__desc">{d}</div> })}
            </div>
            <div class="row__control">{children()}</div>
        </div>
    }
}

#[derive(Clone)]
pub enum NavEntry {
    Group(Signal<String>),
    Item { id: &'static str, icon: &'static str, label: Signal<String> },
}

#[component]
pub fn SettingsNav(entries: Vec<NavEntry>, active: RwSignal<&'static str>) -> impl IntoView {
    view! {
        <nav class="snav">
            {entries.into_iter().map(|entry| match entry {
                NavEntry::Group(label) => view! { <div class="snav__group">{move || label.get()}</div> }.into_any(),
                NavEntry::Item { id, icon, label } => view! {
                    <button
                        type="button"
                        class="snav__item"
                        class:is-active=move || active.get() == id
                        on:click=move |_| {
                            sound::play_click();
                            active.set(id);
                        }
                    >
                        <Icon name=icon outlined=true />
                        <span>{move || label.get()}</span>
                    </button>
                }.into_any(),
            }).collect_view()}
        </nav>
    }
}

#[derive(Clone)]
pub struct TabDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: Signal<String>,
}

#[component]
pub fn Tabs(tabs: Vec<TabDef>, active: RwSignal<&'static str>) -> impl IntoView {
    view! {
        <div class="tabs" role="tablist">
            {tabs.into_iter().map(|tab| view! {
                <button
                    type="button"
                    role="tab"
                    class="tab"
                    class:is-active=move || active.get() == tab.id
                    title=move || tab.label.get()
                    on:click=move |_| {
                        sound::play_click();
                        active.set(tab.id);
                    }
                >
                    <span class="tab__label"><Icon name=tab.icon outlined=true />{move || tab.label.get()}</span>
                    <span class="tab__bar"></span>
                </button>
            }).collect_view()}
        </div>
    }
}

/// One choice of `ChipTabs`: an icon in its tone's colour and a label.
#[derive(Clone)]
pub struct ChipDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: Signal<String>,
    pub tone: TagTone,
}

/// Content-sized choices in a row (the loaders on "Create build"); the active one is lit in its
/// own colour. Same height as buttons and fields.
#[component]
pub fn ChipTabs(chips: Vec<ChipDef>, active: RwSignal<&'static str>) -> impl IntoView {
    view! {
        <div class="chips" role="tablist">
            {chips.into_iter().map(|chip| view! {
                <button
                    type="button"
                    role="tab"
                    class=format!("chip {}", chip.tone.chip_class())
                    class:is-active=move || active.get() == chip.id
                    aria-selected=move || (active.get() == chip.id).to_string()
                    on:click=move |_| {
                        sound::play_click();
                        active.set(chip.id);
                    }
                >
                    <span class="chip__icon"><Icon name=chip.icon outlined=true /></span>
                    <span class="chip__label">{move || chip.label.get()}</span>
                </button>
            }).collect_view()}
        </div>
    }
}

#[component]
pub fn ListRow(#[prop(optional, into)] on_click: Option<Callback<()>>, children: Children) -> impl IntoView {
    let clickable = on_click.is_some();
    view! {
        <div
            class="list-row"
            style=if clickable { "cursor:pointer" } else { "" }
            on:click=move |_| {
                if let Some(cb) = on_click {
                    sound::play_click();
                    cb.run(());
                }
            }
        >
            {children()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dangerous_section_shows_its_icon_like_a_dangerous_dialog() {
        let css = include_str!("../../styles/components.css");
        assert!(css.contains(
            ".section__icon--danger { background: rgba(240, 82, 74, 0.12); border-color: rgba(240, 82, 74, 0.35); color: var(--danger); }"
        ));
    }

    /// The declarations of the rule whose selector is exactly `selector`.
    fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let line = css
            .lines()
            .find(|line| line.split('{').next().is_some_and(|s| s.trim() == selector))
            .unwrap_or_else(|| panic!("no rule {selector}"));
        &line[line.find('{').unwrap() + 1..line.rfind('}').unwrap()]
    }

    #[test]
    fn free_content_in_a_section_keeps_its_distance() {
        let css = include_str!("../../styles/components.css");
        // Grids, button rows and lists get a gap; setting rows bring their own padding and line.
        let free = rule(css, ".section > :where(:not(.section__head, .row))");
        assert!(free.contains("margin: 12px 0"), "{free}");
    }

    #[test]
    fn open_menus_and_tips_float_above_dialogs() {
        let css = include_str!("../../styles/components.css");
        // A select's menu and the tooltip are drawn in portals in window coordinates: no dialog,
        // scroll box or neighbouring layer can clip or cover them.
        let z = |selector: &str| {
            let body = rule(css, selector);
            body.split("z-index:")
                .nth(1)
                .and_then(|r| r.split(';').next())
                .and_then(|n| n.trim().parse::<i32>().ok())
        };
        assert!(rule(css, ".menu.is-floating").contains("position: fixed"));
        assert!(rule(css, ".tip").contains("position: fixed"));
        let (backdrop, menu, tip) =
            (z(".backdrop").unwrap(), z(".menu.is-floating").unwrap(), z(".tip").unwrap());
        assert!(backdrop < menu && menu < tip, "{backdrop} < {menu} < {tip}");
    }

    #[test]
    fn chips_take_their_loader_colour() {
        let css = include_str!("../../styles/components.css");
        for (tone, rgb) in [
            (crate::TagTone::Vanilla, "69, 194, 126"),
            (crate::TagTone::Fabric, "219, 190, 120"),
            (crate::TagTone::Quilt, "170, 130, 240"),
            (crate::TagTone::Forge, "45, 184, 218"),
            (crate::TagTone::NeoForge, "245, 145, 66"),
        ] {
            let chip = rule(css, &format!(".{}", tone.chip_class()));
            assert!(chip.contains(&format!("--chip: {rgb}")), "{tone:?}: {chip}");
        }
        assert!(rule(css, ".chip.is-active").contains("rgba(var(--chip), 0.45)"));
    }
}
