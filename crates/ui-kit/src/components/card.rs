//! `BuildCard`: a Home tile (mockup-v2) — name, "{client} {version}", the picture, and on hover a
//! Play surface.

use leptos::ev::MouseEvent;
use leptos::prelude::*;

use super::icon::Icon;
use crate::sound;

/// Builds without a picture show the grass block.
pub const FALLBACK_IMAGE: &str = "/img/grass_block.png";

/// The icon on a card's button: Play unless the card says otherwise (Download for a build to
/// install).
pub fn play_icon(icon: Option<&'static str>) -> &'static str {
    icon.unwrap_or("play_arrow")
}

pub fn card_image(image: Option<&str>) -> String {
    image.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(FALLBACK_IMAGE).to_string()
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
    /// The button's icon (Play by default).
    #[prop(optional)]
    icon: Option<&'static str>,
    on_play: Callback<()>,
    #[prop(optional, into)] on_menu: Option<Callback<(i32, i32)>>,
) -> impl IntoView {
    let src = card_image(image.as_deref());
    let is_running = move || running.get().unwrap_or(false);
    let is_busy = move || busy.get().unwrap_or(false);
    view! {
        <div
            class="card"
            class:is-running=is_running
            on:contextmenu=move |ev: MouseEvent| {
                if let Some(cb) = on_menu {
                    ev.prevent_default();
                    ev.stop_propagation();
                    cb.run((ev.client_x(), ev.client_y()));
                }
            }
        >
            <div class="card__title" data-tip=title.clone() data-tip-side="top">{title.clone()}</div>
            <div class="card__subtitle">{subtitle}</div>
            <div class="card__preview">
                <img class="card__image" src=src alt="" draggable="false" />
                <button
                    type="button"
                    class="card__play"
                    class:is-busy=is_busy
                    title=move || play_label.get()
                    aria-label=move || play_label.get()
                    on:click=move |_| {
                        if !is_busy() {
                            sound::play_click();
                            on_play.run(());
                        }
                    }
                >
                    <Icon name=play_icon(icon) />
                </button>
            </div>
            {move || is_running().then(|| {
                let label = running_label.get().unwrap_or_default();
                view! { <span class="card__status" role="status" data-tip=label.clone() data-tip-side="top" aria-label=label></span> }
            })}
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

    /// The declarations of `selector`'s rule (it may span lines).
    fn rule(css: &str, selector: &str) -> String {
        let start = css.find(&format!("{selector} {{")).unwrap_or_else(|| panic!("no rule {selector}"));
        let body = &css[start..];
        body[body.find('{').unwrap() + 1..body.find('}').unwrap()].to_string()
    }

    #[test]
    fn the_play_button_takes_the_click_over_the_hidden_picture() {
        // On hover the picture fades to opacity 0, which gives it a layer of its own above the
        // button: the button needs its own layer on top, and the picture takes no clicks.
        let css = include_str!("../../styles/components.css");
        let play = rule(css, ".card__play");
        assert!(play.contains("position: relative") && play.contains("z-index: 1"), "{play}");
        assert!(rule(css, ".card__image").contains("pointer-events: none"));
    }
}
