//! The screenshot viewer: the picture as large as the window allows, ← → through the shown list,
//! and beside it the name (renamed in place), the build, the size and what can be done.

use launcher_shared::{ScreenshotDto, ShotRef};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, ConfirmDialog, Dialog, Icon, IconAction, Size, TextInput, Variant};

use super::actions::use_shot_actions;
use super::{split_name, step};

/// Local "dd.mm.yyyy hh:mm" of Unix milliseconds.
pub fn date_time(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:02}.{:02}.{} {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}

/// What a page asks the viewer to show: `list[at]`, and a rename started from a menu.
#[derive(Clone, Copy)]
pub struct ViewerState {
    pub open: RwSignal<bool>,
    pub at: RwSignal<usize>,
    pub renaming: RwSignal<bool>,
}

impl ViewerState {
    pub fn new() -> ViewerState {
        ViewerState { open: RwSignal::new(false), at: RwSignal::new(0), renaming: RwSignal::new(false) }
    }

    pub fn show(&self, at: usize, rename: bool) {
        self.at.set(at);
        self.renaming.set(rename);
        self.open.set(true);
    }
}

#[component]
pub fn ScreenshotViewer(
    state: ViewerState,
    /// The shown screenshots with their build's key, in the page's order.
    #[prop(into)]
    list: Signal<Vec<(String, ScreenshotDto)>>,
    /// A build's name by its key.
    build_name: Callback<String, String>,
    /// After a rename (the build key and the new name, to stay on it) or a delete (`None`).
    on_changed: Callback<Option<(String, String)>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let actions = use_shot_actions();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let current = Memo::new(move |_| list.with(|l| l.get(state.at.get()).cloned()));
    let count = Memo::new(move |_| list.with(Vec::len));
    // A shorter list (a delete) keeps the viewer on the last one.
    Effect::new(move |_| {
        let len = count.get();
        if len == 0 {
            state.open.set(false);
        } else if state.at.get_untracked() >= len {
            state.at.set(len - 1);
        }
    });
    // A comparison inside `view!` would end the tag: these are worked out here.
    let at_end = Signal::derive(move || state.at.get() + 1 >= count.get());
    let go = move |delta: isize| {
        if let Some(next) = step(state.at.get_untracked(), count.get_untracked(), delta) {
            state.renaming.set(false);
            state.at.set(next);
        }
    };
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        let typing = ev
            .target()
            .and_then(|t| js_sys::Reflect::get(&t, &"tagName".into()).ok())
            .and_then(|tag| tag.as_string())
            .is_some_and(|tag| tag == "INPUT" || tag == "TEXTAREA");
        if !state.open.get_untracked() || typing {
            return;
        }
        match ev.key().as_str() {
            "ArrowLeft" => go(-1),
            "ArrowRight" => go(1),
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());

    // The picture's own size, read once it has loaded.
    let picture = NodeRef::<leptos::html::Img>::new();
    let resolution = RwSignal::new(None::<(u32, u32)>);
    Effect::new(move |_| {
        current.track();
        resolution.set(None);
    });

    let wanted = RwSignal::new(String::new());
    let rename_error = RwSignal::new(None::<String>);
    let rename_box = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if state.renaming.get() {
            let name = current.with_untracked(|c| c.as_ref().map(|(_, s)| split_name(&s.name).0.to_string()));
            wanted.set(name.unwrap_or_default());
            rename_error.set(None);
            // The old name is selected: typing replaces it.
            request_animation_frame(move || {
                use wasm_bindgen::JsCast;
                let field = rename_box
                    .try_get_untracked()
                    .flatten()
                    .and_then(|b| b.query_selector("input").ok().flatten())
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
                if let Some(field) = field {
                    let _ = field.focus();
                    if let Ok(select) = js_sys::Reflect::get(&field, &"select".into())
                        && let Ok(select) = select.dyn_into::<js_sys::Function>()
                    {
                        let _ = select.call0(&field);
                    }
                }
            });
        }
    });
    let renamed = Callback::new(move |(key, result): (String, Result<ScreenshotDto, String>)| match result {
        Ok(renamed) => {
            state.renaming.set(false);
            on_changed.run(Some((key, renamed.name)));
        }
        Err(message) => rename_error.set(Some(message)),
    });
    let save_name = move || {
        let Some((key, shot)) = current.get_untracked() else { return };
        actions.rename(key, shot.name, wanted.get_untracked(), renamed);
    };

    let delete_open = RwSignal::new(false);
    let delete_text = Signal::derive(move || {
        current
            .with(|c| {
                c.as_ref().map(|(_, s)| i18n.tp("confirm_delete_screenshot", &[("name", s.name.clone())]))
            })
            .unwrap_or_default()
    });
    let deleted = Callback::new(move |_: usize| on_changed.run(None));
    let confirm_delete = Callback::new(move |()| {
        delete_open.set(false);
        let Some((key, shot)) = current.get_untracked() else { return };
        actions.delete(vec![ShotRef { key, name: shot.name }], deleted);
    });

    let with_current = move |act: fn(&super::actions::ShotActions, String, String)| {
        if let Some((key, shot)) = current.get_untracked() {
            act(&actions, key, shot.name);
        }
    };
    let title =
        Signal::derive(move || current.with(|c| c.as_ref().map(|(_, s)| s.name.clone()).unwrap_or_default()));
    let subtitle = Signal::derive(move || {
        current.with(|c| {
            c.as_ref().map(|(key, _)| {
                format!("{} · {} / {}", build_name.run(key.clone()), state.at.get() + 1, count.get())
            })
        })
    });

    view! {
        <Dialog open=state.open full=true icon="image" title=title subtitle=subtitle>
            {move || current.get().map(|(key, shot)| {
                let size = launcher_shared::units::size(shot.size, &i18n.lang());
                let when = shot.modified_ms.map(date_time).unwrap_or_else(|| "-".into());
                let build = build_name.run(key.clone());
                view! {
                    <div class="viewer">
                        <div class="viewer__stage">
                            <img
                                class="viewer__picture"
                                src=shot.src.clone()
                                alt=""
                                draggable="false"
                                node_ref=picture
                                on:load=move |_| {
                                    if let Some(img) = picture.get_untracked() {
                                        resolution.set(Some((img.natural_width(), img.natural_height())));
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="viewer__nav is-prev"
                                aria-label=move || i18n.t("shots_previous")
                                disabled=move || state.at.get() == 0
                                on:click=move |_| go(-1)
                            >
                                <Icon name="chevron_left" />
                            </button>
                            <button
                                type="button"
                                class="viewer__nav is-next"
                                aria-label=move || i18n.t("shots_next")
                                disabled=at_end
                                on:click=move |_| go(1)
                            >
                                <Icon name="chevron_right" />
                            </button>
                        </div>
                        <div class="viewer__side">
                            <Show
                                when=move || state.renaming.get()
                                fallback=move || view! {
                                    <div class="viewer__name">
                                        <span>{move || title.get()}</span>
                                        <IconAction icon="edit" title=t("rename") on_click=Callback::new(move |()| state.renaming.set(true)) />
                                    </div>
                                }
                            >
                                <div class="viewer__rename" node_ref=rename_box>
                                    <TextInput
                                        value=wanted
                                        autofocus=true
                                        invalid=Signal::derive(move || rename_error.with(Option::is_some))
                                        on_enter=Callback::new(move |()| save_name())
                                    />
                                    <IconAction icon="check" title=t("save") on_click=Callback::new(move |()| save_name()) />
                                    <IconAction icon="close" title=t("cancel") on_click=Callback::new(move |()| state.renaming.set(false)) />
                                </div>
                                {move || rename_error.get().map(|e| view! { <div class="viewer__error">{e}</div> })}
                            </Show>
                            <dl class="viewer__info">
                                <dt>{move || i18n.t("shots_build")}</dt>
                                <dd>{build}</dd>
                                <dt>{move || i18n.t("shots_resolution")}</dt>
                                <dd>{move || resolution.get().map(|(w, h)| format!("{w} × {h}")).unwrap_or_else(|| "…".into())}</dd>
                                <dt>{move || i18n.t("shots_size")}</dt>
                                <dd>{size}</dd>
                                <dt>{move || i18n.t("shots_taken")}</dt>
                                <dd>{when}</dd>
                            </dl>
                            <div class="viewer__actions">
                                <Button size=Size::Md icon="content_copy" on_click=move |_| with_current(|a, k, n| a.copy(k, n))>
                                    {move || i18n.t("shots_copy")}
                                </Button>
                                <Button size=Size::Md icon="open_in_new" on_click=move |_| with_current(|a, k, n| a.open(k, n))>
                                    {move || i18n.t("open_screenshot")}
                                </Button>
                                <Button size=Size::Md icon="folder_open" on_click=move |_| with_current(|a, k, n| a.reveal(k, n))>
                                    {move || i18n.t("shots_reveal")}
                                </Button>
                                <Button size=Size::Md variant=Variant::Danger icon="delete_outline" on_click=move |_| delete_open.set(true)>
                                    {move || i18n.t("delete")}
                                </Button>
                            </div>
                        </div>
                    </div>
                }
            })}
        </Dialog>
        <ConfirmDialog
            open=delete_open
            danger=true
            title=t("confirmation")
            message=delete_text
            confirm_label=t("delete")
            cancel_label=t("cancel")
            on_confirm=confirm_delete
        />
    }
}
