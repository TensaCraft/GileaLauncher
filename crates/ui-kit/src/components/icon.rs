use leptos::prelude::*;

/// Material icon by ligature name, e.g. `play_arrow`. `outlined` switches to the outlined set.
/// A provider's brand mark (`brand:modrinth`) or a loader's mark (`loader:fabric`) is drawn in its place,
/// the same size.
#[component]
pub fn Icon(
    #[prop(into)] name: String,
    #[prop(optional)] outlined: bool,
    #[prop(optional)] size: Option<u32>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let classes = format!(
        "icon{}{}{}",
        if outlined { " icon--o" } else { "" },
        if class.is_empty() { "" } else { " " },
        class
    );
    let style = size.map(|s| format!("font-size:{s}px"));
    if let Some(path) = super::brand::brand_path(&name) {
        let brand = name.trim_start_matches("brand:");
        let classes = format!("{classes} icon--brand icon--brand-{brand}");
        return view! {
            <svg class=classes style=style viewBox="0 0 24 24" aria-hidden="true"><path d=path fill="currentColor" /></svg>
        }
        .into_any();
    }
    if let Some(mark) = super::loader_mark::loader_mark(&name) {
        let loader = name.trim_start_matches("loader:");
        let classes = format!("{classes} icon--loader icon--loader-{loader}");
        return view! {
            <svg
                class=classes
                style=style
                viewBox="0 0 24 24"
                aria-hidden="true"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
            >
                {mark.fills.iter().map(|d| view! { <path d=*d fill="currentColor" fill-opacity="0.3" stroke="none" /> }).collect_view()}
                {mark.strokes.iter().map(|d| view! { <path d=*d /> }).collect_view()}
            </svg>
        }
        .into_any();
    }
    view! { <span class=classes style=style aria-hidden="true">{name}</span> }.into_any()
}
