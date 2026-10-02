use leptos::prelude::*;

/// Material icon by ligature name, e.g. `play_arrow`. `outlined` switches to the outlined set.
/// A provider's brand mark (`brand:modrinth`) is drawn in its place, the same size.
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
    view! { <span class=classes style=style aria-hidden="true">{name}</span> }.into_any()
}
