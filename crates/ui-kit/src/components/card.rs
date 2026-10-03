//! `BuildCard`: a Home tile. The build's picture fills it (blurred behind, sharp in the middle), its
//! loader is a tag at the top, the name and version sit at its foot. Its Play button is where the
//! user put it (`CardPlay`): a round one in the middle on hover, a bar in place of the version on
//! hover, or a small one in the corner, always there. A running build stops from the same button
//! when the card is given `on_stop`.

use launcher_shared::CardPlay;
use leptos::ev::MouseEvent;
use leptos::prelude::*;

use super::icon::Icon;
use super::tag::TagTone;
use crate::sound;

/// Builds without a picture show the grass block.
pub const FALLBACK_IMAGE: &str = "/img/grass_block.png";

/// Where every build card puts its Play button: the app provides it from its settings (the middle
/// without it).
#[derive(Clone, Copy)]
pub struct CardPlayStyle(pub Signal<CardPlay>);

/// The icon on a card's button: Play unless the card says otherwise (Download for a build to
/// install).
pub fn play_icon(icon: Option<&'static str>) -> &'static str {
    icon.unwrap_or("play_arrow")
}

pub fn card_image(image: Option<&str>) -> String {
    image.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(FALLBACK_IMAGE).to_string()
}

/// The class of the Play button where `play` puts it.
fn play_class(play: CardPlay) -> &'static str {
    match play {
        CardPlay::Center => "card__play is-center",
        CardPlay::Bar => "card__play is-bar",
        CardPlay::Corner => "card__play is-corner",
    }
}

#[component]
pub fn BuildCard(
    #[prop(into)] title: String,
    #[prop(into)] subtitle: String,
    #[prop(into)] image: Option<String>,
    #[prop(into)] play_label: Signal<String>,
    #[prop(optional, into)] running_label: MaybeProp<String>,
    #[prop(optional, into)] running: MaybeProp<bool>,
    #[prop(optional, into)] busy: MaybeProp<bool>,
    /// What the button says while the build starts (the bar shows it).
    #[prop(optional, into)]
    busy_label: MaybeProp<String>,
    /// The button's icon (Play by default).
    #[prop(optional)]
    icon: Option<&'static str>,
    /// The loader's tag at the top (its name and colour).
    #[prop(optional)]
    tag: Option<(String, TagTone)>,
    on_play: Callback<()>,
    /// A running build's button stops it (with `stop_label`).
    #[prop(optional, into)]
    on_stop: Option<Callback<()>>,
    #[prop(optional, into)] stop_label: MaybeProp<String>,
    #[prop(optional, into)] on_menu: Option<Callback<(i32, i32)>>,
) -> impl IntoView {
    let style = use_context::<CardPlayStyle>().map_or_else(|| Signal::stored(CardPlay::Center), |s| s.0);
    let src = card_image(image.as_deref());
    let is_running = move || running.get().unwrap_or(false);
    let is_busy = move || busy.get().unwrap_or(false);
    let stops = move || is_running() && on_stop.is_some();
    let label = Signal::derive(move || {
        if is_busy() {
            busy_label.get().unwrap_or_else(|| play_label.get())
        } else if stops() {
            stop_label.get().unwrap_or_default()
        } else {
            play_label.get()
        }
    });
    let button = move |play: CardPlay| {
        view! {
            <button
                type="button"
                class=play_class(play)
                class:is-busy=is_busy
                class:is-stop=stops
                data-tip=move || (play != CardPlay::Bar).then(|| label.get())
                data-tip-side="top"
                aria-label=move || label.get()
                on:click=move |_| {
                    if is_busy() {
                        return;
                    }
                    sound::play_click();
                    match on_stop.filter(|_| stops()) {
                        Some(stop) => stop.run(()),
                        None => on_play.run(()),
                    }
                }
            >
                {move || view! { <Icon name=if stops() { "stop" } else { play_icon(icon) } /> }}
                <span class="card__play-label">{move || label.get()}</span>
            </button>
        }
    };
    view! {
        <div
            class="card"
            class:is-running=is_running
            class:card--center=move || style.get() == CardPlay::Center
            class:card--bar=move || style.get() == CardPlay::Bar
            class:card--corner=move || style.get() == CardPlay::Corner
            on:contextmenu=move |ev: MouseEvent| {
                if let Some(cb) = on_menu {
                    ev.prevent_default();
                    ev.stop_propagation();
                    cb.run((ev.client_x(), ev.client_y()));
                }
            }
        >
            <div class="card__cover" aria-hidden="true">
                <img class="card__backdrop" src=src.clone() alt="" draggable="false" />
                <img class="card__image" src=src alt="" draggable="false" />
            </div>
            {tag.map(|(name, tone)| view! { <span class=format!("tag card__tag {}", tone.class())>{name}</span> })}
            {move || is_running().then(|| {
                let text = running_label.get().unwrap_or_default();
                // A dot beside the loader's tag (both fit the card), its words in its tip.
                view! {
                    <span class="card__badge" role="status" aria-label=text.clone() data-tip=text data-tip-side="top">
                        <span class="card__dot"></span>
                    </span>
                }
            })}
            <div class="card__foot">
                <div class="card__title" data-tip=title.clone() data-tip-side="top">{title.clone()}</div>
                <div class="card__subtitle">{subtitle}</div>
                {move || (style.get() == CardPlay::Bar).then(|| button(CardPlay::Bar))}
            </div>
            {move || {
                let play = style.get();
                (play != CardPlay::Bar).then(|| button(play))
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_s_button_plays_unless_told_otherwise() {
        assert_eq!(play_icon(None), "play_arrow");
        assert_eq!(play_icon(Some("download")), "download");
    }

    #[test]
    fn cards_fall_back_to_the_grass_block() {
        assert_eq!(card_image(None), FALLBACK_IMAGE);
        assert_eq!(card_image(Some("  ")), FALLBACK_IMAGE);
        assert_eq!(card_image(Some("https://cdn/pack.png")), "https://cdn/pack.png");
        assert_eq!(FALLBACK_IMAGE, "/img/grass_block.png");
    }

    #[test]
    fn each_place_of_the_play_button_has_its_class() {
        assert_eq!(play_class(CardPlay::Center), "card__play is-center");
        assert_eq!(play_class(CardPlay::Bar), "card__play is-bar");
        assert_eq!(play_class(CardPlay::Corner), "card__play is-corner");
    }

    /// The declarations of `selector`'s rule (it may span lines).
    fn rule(css: &str, selector: &str) -> String {
        let start = css.find(&format!("{selector} {{")).unwrap_or_else(|| panic!("no rule {selector}"));
        let body = &css[start..];
        body[body.find('{').unwrap() + 1..body.find('}').unwrap()].to_string()
    }

    #[test]
    fn the_play_button_takes_the_click_over_the_cover() {
        // The pictures take no clicks, and the button sits above the cover, its dimming and the
        // name's shade wherever it is.
        let css = include_str!("../../styles/components.css");
        assert!(rule(css, ".card__cover").contains("pointer-events: none"));
        for place in [".card__play.is-center", ".card__play.is-corner"] {
            let play = rule(css, place);
            assert!(play.contains("position: absolute") && play.contains("z-index: 3"), "{place}: {play}");
        }
        assert!(rule(css, ".card__foot").contains("z-index: 2"));
    }
}
