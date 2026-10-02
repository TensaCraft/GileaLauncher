use std::sync::Arc;

use leptos::portal::Portal;
use leptos::prelude::*;

use super::icon::Icon;
use crate::sound;

const MENU_WIDTH: i32 = 230;
const ITEM_HEIGHT: i32 = 32;
const MARGIN: i32 = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Item {
        id: String,
        label: String,
        icon: Option<String>,
        shortcut: Option<String>,
        danger: bool,
        disabled: bool,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(id: impl Into<String>, label: impl Into<String>) -> Self {
        MenuEntry::Item {
            id: id.into(),
            label: label.into(),
            icon: None,
            shortcut: None,
            danger: false,
            disabled: false,
        }
    }

    pub fn icon(mut self, name: impl Into<String>) -> Self {
        if let MenuEntry::Item { icon, .. } = &mut self {
            *icon = Some(name.into());
        }
        self
    }

    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        if let MenuEntry::Item { shortcut, .. } = &mut self {
            *shortcut = Some(keys.into());
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let MenuEntry::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    pub fn disabled(mut self, value: bool) -> Self {
        if let MenuEntry::Item { disabled, .. } = &mut self {
            *disabled = value;
        }
        self
    }
}

/// Keeps a `w`×`h` menu opened at (x, y) inside a `vw`×`vh` viewport with an 8 px margin.
pub fn clamp_menu_pos(x: i32, y: i32, w: i32, h: i32, vw: i32, vh: i32) -> (i32, i32) {
    let max_x = (vw - w - MARGIN).max(MARGIN);
    let max_y = (vh - h - MARGIN).max(MARGIN);
    (x.clamp(MARGIN, max_x), y.clamp(MARGIN, max_y))
}

/// Receives the chosen item id. A plain closure, not a `Callback`: the row that opened the menu
/// may be re-rendered (and its reactive values disposed) while the menu is still open.
type MenuHandler = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Copy)]
pub struct ContextMenu {
    pos: RwSignal<Option<(i32, i32)>>,
    entries: RwSignal<Vec<MenuEntry>>,
    /// Receives the chosen id of a menu opened with `open_with`.
    handler: StoredValue<Option<MenuHandler>>,
}

impl ContextMenu {
    pub fn open_at(&self, x: i32, y: i32, entries: Vec<MenuEntry>) {
        self.handler.set_value(None);
        let (vw, vh) = web_sys::window()
            .map(|w| {
                let num = |v: Result<wasm_bindgen::JsValue, _>| {
                    v.ok().and_then(|v| v.as_f64()).unwrap_or(1200.0) as i32
                };
                (num(w.inner_width()), num(w.inner_height()))
            })
            .unwrap_or((1200, 800));
        let height = entries
            .iter()
            .map(|e| if matches!(e, MenuEntry::Separator) { 9 } else { ITEM_HEIGHT })
            .sum::<i32>()
            + 8;
        self.entries.set(entries);
        self.pos.set(Some(clamp_menu_pos(x, y, MENU_WIDTH, height, vw, vh)));
    }

    /// Opens a menu whose choice goes to `on_select` instead of the host's handler.
    pub fn open_with(
        &self,
        x: i32,
        y: i32,
        entries: Vec<MenuEntry>,
        on_select: impl Fn(String) + Send + Sync + 'static,
    ) {
        self.open_at(x, y, entries);
        self.set_handler(on_select);
    }

    fn set_handler(&self, on_select: impl Fn(String) + Send + Sync + 'static) {
        self.handler.set_value(Some(Arc::new(on_select)));
    }

    /// Runs the chosen item: the handler of `open_with`, else the host's `fallback`.
    pub fn select(&self, id: String, fallback: Callback<String>) {
        match self.handler.get_value() {
            Some(handler) => handler(id),
            None => fallback.run(id),
        }
    }

    pub fn close(&self) {
        self.pos.set(None);
    }
}

pub fn provide_context_menu() -> ContextMenu {
    let menu = ContextMenu {
        pos: RwSignal::new(None),
        entries: RwSignal::new(Vec::new()),
        handler: StoredValue::new(None),
    };
    provide_context(menu);
    menu
}

pub fn use_context_menu() -> ContextMenu {
    expect_context::<ContextMenu>()
}

#[component]
pub fn ContextMenuHost(on_select: Callback<String>) -> impl IntoView {
    let menu = use_context_menu();
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        // Opened over a dialog, the menu takes Escape first and marks it handled.
        if ev.key() == "Escape" && !ev.default_prevented() && menu.pos.get_untracked().is_some() {
            ev.prevent_default();
            menu.close();
        }
    });
    on_cleanup(move || escape.remove());
    view! {
        <Show when=move || menu.pos.get().is_some()>
            <Portal>
                <div class="menu-catcher" on:click=move |_| menu.close() on:contextmenu=move |ev| {
                    ev.prevent_default();
                    menu.close();
                }></div>
                <div
                    class="cmenu"
                    role="menu"
                    style=move || menu.pos.get().map(|(x, y)| format!("left:{x}px;top:{y}px")).unwrap_or_default()
                >
                    {move || menu.entries.get().into_iter().map(|entry| match entry {
                        MenuEntry::Separator => view! { <div class="menu__sep"></div> }.into_any(),
                        MenuEntry::Item { id, label, icon, shortcut, danger, disabled } => view! {
                            <div
                                class="menu__item"
                                class:is-danger=danger
                                style=if disabled { "opacity:.45;pointer-events:none" } else { "" }
                                role="menuitem"
                                on:click=move |_| {
                                    sound::play_click();
                                    menu.close();
                                    menu.select(id.clone(), on_select);
                                }
                            >
                                {icon.map(|i| view! { <Icon name=i outlined=true /> })}
                                <span>{label}</span>
                                {shortcut.map(|s| view! { <span class="menu__meta">{s}</span> })}
                            </div>
                        }.into_any(),
                    }).collect_view()}
                </div>
            </Portal>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_is_kept_inside_viewport() {
        assert_eq!(clamp_menu_pos(10, 10, 230, 200, 1200, 800), (10, 10));
        assert_eq!(clamp_menu_pos(1100, 700, 230, 200, 1200, 800), (962, 592));
        assert_eq!(clamp_menu_pos(-5, -5, 230, 200, 1200, 800), (8, 8));
    }

    #[test]
    fn a_handler_outlives_the_row_that_opened_the_menu() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = Owner::new();
        root.set();
        let menu = provide_context_menu();
        let hits = Arc::new(AtomicUsize::new(0));
        // A list row opens the menu, then the list re-renders and the row is disposed.
        let row = Owner::new();
        row.with(|| {
            let hits = hits.clone();
            menu.set_handler(move |_: String| {
                hits.fetch_add(1, Ordering::SeqCst);
            });
        });
        row.cleanup();
        menu.select("default".into(), Callback::new(|_| {}));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
