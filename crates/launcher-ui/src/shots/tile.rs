//! A screenshot's thumbnail in a grid: its name and date on hover, a check in select mode, the
//! screenshot's menu on a right click.

use launcher_shared::ScreenshotDto;
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use ui_kit::Icon;
use ui_kit::i18n::use_i18n;

use super::viewer::date_time;

/// The ids of a screenshot menu's entries.
pub const MENU_VIEW: &str = "view";
pub const MENU_COPY: &str = "copy";
pub const MENU_RENAME: &str = "rename";
pub const MENU_OPEN: &str = "open";
pub const MENU_REVEAL: &str = "reveal";
pub const MENU_DELETE: &str = "delete";

/// The entries of a screenshot's menu.
pub fn menu_entries(i18n: ui_kit::i18n::I18nCtx) -> Vec<ui_kit::MenuEntry> {
    use ui_kit::MenuEntry;
    vec![
        MenuEntry::item(MENU_VIEW, i18n.t("shots_view")).icon("visibility"),
        MenuEntry::item(MENU_COPY, i18n.t("shots_copy")).icon("content_copy"),
        MenuEntry::item(MENU_RENAME, i18n.t("rename")).icon("edit"),
        MenuEntry::item(MENU_OPEN, i18n.t("open_screenshot")).icon("open_in_new"),
        MenuEntry::item(MENU_REVEAL, i18n.t("shots_reveal")).icon("folder_open"),
        MenuEntry::item(MENU_DELETE, i18n.t("delete")).icon("delete_outline").danger(),
    ]
}

#[component]
pub fn ShotTile(
    shot: ScreenshotDto,
    #[prop(into)] selecting: Signal<bool>,
    #[prop(into)] selected: Signal<bool>,
    /// A click: the viewer, or (selecting) the check.
    on_click: Callback<()>,
    /// A right click at (x, y).
    on_menu: Callback<(i32, i32)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let when = shot.modified_ms.map(date_time).unwrap_or_default();
    let menu = move |ev: MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        on_menu.run((ev.client_x(), ev.client_y()));
    };
    view! {
        <button
            type="button"
            class="shot-tile"
            class:is-selecting=selecting
            class:is-selected=selected
            aria-label=move || if selecting.get() { i18n.t("shots_select_one") } else { i18n.t("shots_view") }
            aria-pressed=move || selecting.get().then(|| selected.get().to_string())
            on:click=move |_| on_click.run(())
            on:contextmenu=menu
        >
            <img class="shot-tile__img" src=shot.thumb.clone() alt="" loading="lazy" draggable="false" />
            <span class="shot-tile__caption">
                <span class="shot-tile__name">{shot.name.clone()}</span>
                <span class="shot-tile__date">{when}</span>
            </span>
            <span class="shot-tile__check" aria-hidden="true"><Icon name="check" /></span>
        </button>
    }
}
