use leptos::prelude::*;

/// The faces of the launcher's mark, the cube of blocks from its icon (cell 7, top corner at the
/// top): each face's kind (its shade) and points.
const MARK: [(&str, &str); 12] = [
    ("top", "12.1,0 18.2,3.5 12.1,7 6.1,3.5"),
    ("top", "18.2,3.5 24.2,7 18.2,10.5 12.1,7"),
    ("top", "6.1,3.5 12.1,7 6.1,10.5 0,7"),
    ("floor", "12.1,14 18.2,17.5 12.1,21 6.1,17.5"),
    ("right", "24.2,21 18.2,24.5 18.2,17.5 24.2,14"),
    ("right", "18.2,24.5 12.1,28 12.1,21 18.2,17.5"),
    ("right", "24.2,14 18.2,17.5 18.2,10.5 24.2,7"),
    ("inner-right", "12.1,14 6.1,17.5 6.1,10.5 12.1,7"),
    ("left", "0,21 6.1,24.5 6.1,17.5 0,14"),
    ("left", "6.1,24.5 12.1,28 12.1,21 6.1,17.5"),
    ("left", "0,14 6.1,17.5 6.1,10.5 0,7"),
    ("inner-left", "12.1,14 18.2,17.5 18.2,10.5 12.1,7"),
];

/// The bottom of the window: the launcher's mark between two lines (no name: the launcher names no
/// brand).
#[component]
pub fn Footer() -> impl IntoView {
    view! {
        <footer class="footer">
            <div class="footer-mark" aria-hidden="true">
                <span class="footer-mark__line"></span>
                <svg class="footer-mark__cube" viewBox="-1 -1 26.3 30">
                    {MARK.iter().map(|(kind, points)| view! {
                        <polygon class=format!("is-{kind}") points=*points></polygon>
                    }).collect_view()}
                </svg>
                <span class="footer-mark__line is-right"></span>
            </div>
        </footer>
    }
}
