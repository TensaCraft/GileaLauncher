//! What a provider's tab and the modpacks page share: a project's row (icon, title, author and
//! downloads, a clipped description, the caller's actions), the list's loading rows and the page
//! switcher.

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Icon, Size, Skeleton, Variant};

use launcher_shared::provider::{ProjectHit, SearchPage, clip};
use launcher_shared::units;

/// Characters of a description on a row.
pub const DESCRIPTION: usize = 150;

/// A row's second line: the author, and the downloads when the provider counts them.
pub fn meta_of(hit: &ProjectHit, lang: &str) -> (String, Option<String>) {
    (hit.author.clone(), (hit.downloads > 0).then(|| units::count(hit.downloads, lang)))
}

#[component]
pub fn ProjectRow(
    hit: ProjectHit,
    /// Characters of its description (150 unless said).
    #[prop(optional)]
    description: Option<usize>,
    children: Children,
) -> impl IntoView {
    let i18n = use_i18n();
    let (author, downloads) = meta_of(&hit, &i18n.lang());
    // A server's own builds are not counted: their line is what they run.
    let meta = move || match downloads.clone() {
        Some(downloads) => {
            i18n.tp("provider_project_meta", &[("author", author.clone()), ("downloads", downloads)])
        }
        None => author.clone(),
    };
    let text = clip(&hit.description, description.unwrap_or(DESCRIPTION));
    let icon = match hit.icon_url.clone() {
        Some(src) => view! { <img src=src alt="" loading="lazy" /> }.into_any(),
        None => view! { <Icon name="extension" /> }.into_any(),
    };
    view! {
        <div class="list-row build-row provider-row">
            <div class="build-row__icon">{icon}</div>
            <div class="build-row__text">
                <div class="build-row__name">{hit.title.clone()}</div>
                <div class="build-row__sub">{meta}</div>
                {(!text.is_empty()).then(|| view! {
                    <div class="build-row__sub component__meta provider-row__desc">{text}</div>
                })}
            </div>
            <div class="build-row__actions">{children()}</div>
        </div>
    }
}

/// The rows a list shows while it loads.
#[component]
pub fn ListSkeleton() -> impl IntoView {
    view! {
        <div class="builds__list">
            {(0..4)
                .map(|_| view! { <div class="list-row create__skeleton"><Skeleton width=48 /><Skeleton width=260 /></div> })
                .collect_view()}
        </div>
    }
}

/// "‹ Page X of Y ›" under a list that has more than one page.
#[component]
pub fn Pager(page: Signal<Option<SearchPage>>, on_page: Callback<u32>) -> impl IntoView {
    let i18n = use_i18n();
    move || {
        page.get().filter(SearchPage::paginated).map(|p| {
            let (previous, next) = (p.previous_offset(), p.next_offset());
            let label = i18n.tp(
                "page_indicator",
                &[
                    ("current_page", p.current_page().to_string()),
                    ("total_pages", p.total_pages().to_string()),
                ],
            );
            view! {
                <div class="provider__pages">
                    <Button
                        size=Size::Sm
                        variant=Variant::Ghost
                        icon="chevron_left"
                        title=i18n.t("previous_page")
                        disabled=!p.has_previous()
                        on_click=Callback::new(move |()| on_page.run(previous))
                    />
                    <span>{label}</span>
                    <Button
                        size=Size::Sm
                        variant=Variant::Ghost
                        icon="chevron_right"
                        title=i18n.t("next_page")
                        disabled=!p.has_next()
                        on_click=Callback::new(move |()| on_page.run(next))
                    />
                </div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_without_downloads_shows_only_its_author() {
        let hit = |downloads| ProjectHit {
            project_id: "aero".into(),
            slug: "aero".into(),
            title: "Aero".into(),
            author: "Fabric 26.3".into(),
            description: String::new(),
            downloads,
            icon_url: None,
            url: None,
        };
        assert_eq!(meta_of(&hit(0), "en_US"), ("Fabric 26.3".to_string(), None));
        assert_eq!(meta_of(&hit(1500), "uk_UA").1.as_deref(), Some("1,5 тис."));
    }
}
