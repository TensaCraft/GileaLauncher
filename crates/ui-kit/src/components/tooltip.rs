use leptos::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipPlacement {
    #[default]
    Right,
    Top,
}

#[component]
pub fn Tooltip(
    #[prop(into)] text: Signal<String>,
    #[prop(optional)] placement: TooltipPlacement,
    children: Children,
) -> impl IntoView {
    let side = match placement {
        TooltipPlacement::Right => "right",
        TooltipPlacement::Top => "top",
    };
    // Drawn by `TipLayer`, above everything.
    view! {
        <span class="tooltip-host" data-tip=move || text.get() data-tip-side=side>
            {children()}
        </span>
    }
}
