//! Launcher self-update UI: status line for Settings → Launcher and the two dialogs.

use launcher_shared::{Level, Text, UpdateChannel, UpdateInfo, UpdateState, UpdateStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Button, Dialog, DialogFooter, Tag, TagTone, Toasts, Variant, ipc, use_toasts};

use crate::store::{AppStore, use_store};

/// Local "HH:MM" of a Unix timestamp in milliseconds.
pub fn time_of_day(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// The status line under "Automatic updates". Errors are appended by the caller (`i18n.error`).
pub fn status_line(status: &UpdateStatus, checked_at: Option<String>) -> Text {
    if !status.configured {
        return Text::key("launcher_update_status_disabled");
    }
    match &status.state {
        UpdateState::Idle => Text::key("launcher_update_status_unknown"),
        UpdateState::Checking => Text::key("launcher_update_status_checking"),
        UpdateState::UpToDate => match checked_at {
            Some(time) => Text::key("launcher_update_status_checked_at")
                .param("version", &status.current_version)
                .param("time", time),
            None => Text::key("launcher_update_status_latest"),
        },
        UpdateState::Available { info } => {
            Text::key("launcher_update_status_update_available").param("version", &info.version)
        }
        UpdateState::Downloading { info } => {
            Text::key("launcher_update_status_downloading").param("version", &info.version)
        }
        UpdateState::Ready { info } => {
            Text::key("launcher_update_status_ready").param("version", &info.version)
        }
        UpdateState::Failed { .. } => Text::key("launcher_update_status_failed"),
    }
}

#[derive(serde::Serialize)]
struct CheckArgs {
    manual: bool,
}

/// Update commands. Create during component setup (`use_update_actions`), call from handlers.
#[derive(Clone, Copy)]
pub struct UpdateActions {
    store: AppStore,
    toasts: Toasts,
    i18n: I18nCtx,
}

impl UpdateActions {
    pub fn check(&self) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            match ipc::invoke::<_, UpdateStatus>("update_check", &CheckArgs { manual: true }).await {
                Ok(status) => store.update.set(Some(status)),
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    }

    pub fn download(&self) {
        let Self { store, toasts, i18n } = *self;
        spawn_local(async move {
            match ipc::call::<UpdateStatus>("update_download").await {
                Ok(status) => store.update.set(Some(status)),
                Err(e) => toasts.show(Level::Error, i18n.error(&e), None),
            }
        });
    }

    /// `starting` stays true until the launcher exits, so the button cannot be pressed twice.
    pub fn apply(&self, starting: RwSignal<bool>) {
        let Self { toasts, i18n, .. } = *self;
        starting.set(true);
        spawn_local(async move {
            if let Err(e) = ipc::call::<()>("update_apply").await {
                starting.set(false);
                toasts.show(Level::Error, i18n.error(&e), None);
            }
        });
    }
}

pub fn use_update_actions() -> UpdateActions {
    UpdateActions { store: use_store(), toasts: use_toasts(), i18n: use_i18n() }
}

fn current_info(status: Option<UpdateStatus>) -> Option<UpdateInfo> {
    match status?.state {
        UpdateState::Available { info } | UpdateState::Downloading { info } | UpdateState::Ready { info } => {
            Some(info)
        }
        _ => None,
    }
}

/// "Update available" and "Update ready" dialogs, driven by `app://update`.
#[component]
pub fn UpdateDialogs() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let actions = use_update_actions();
    let available_open = RwSignal::new(false);
    let ready_open = RwSignal::new(false);
    let starting = RwSignal::new(false);
    // Versions whose dialog was already shown or dismissed in this session.
    let dismissed = RwSignal::new(None::<String>);
    let shown_ready = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        let Some(status) = store.update.get() else { return };
        match status.state {
            UpdateState::Available { info } => {
                if dismissed.get_untracked().as_deref() != Some(info.version.as_str()) {
                    available_open.set(true);
                }
            }
            UpdateState::Ready { info } => {
                available_open.set(false);
                if shown_ready.get_untracked().as_deref() != Some(info.version.as_str()) {
                    shown_ready.set(Some(info.version));
                    ready_open.set(true);
                }
            }
            _ => {}
        }
    });

    let info = move || current_info(store.update.get());
    let apply_supported = move || store.update.get().is_some_and(|s| s.apply_supported);
    let dismiss = move || {
        dismissed.set(info().map(|i| i.version));
        available_open.set(false);
    };
    let meta = move || {
        info().map(|i| {
            let channel = match i.channel {
                UpdateChannel::Stable => i18n.t("update_channel_stable"),
                UpdateChannel::Beta => i18n.t("update_channel_beta"),
            };
            let date =
                i.published_at.as_deref().map(|d| d.chars().take(10).collect::<String>()).unwrap_or_default();
            let tone = if i.channel == UpdateChannel::Beta { TagTone::Beta } else { TagTone::Neutral };
            view! {
                <div class="update__meta">
                    <Tag tone=tone>{channel}</Tag>
                    <span>{launcher_shared::units::size(i.size, &i18n.lang())}</span>
                    <span>{date}</span>
                </div>
            }
        })
    };
    let notes = move || {
        info()
            .map(|i| i.notes)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| i18n.t("update_notes_empty"))
    };

    view! {
        <Dialog
            open=available_open
            title=Signal::derive(move || info().map(|i| i18n.tp("update_available_title", &[("version", i.version)])).unwrap_or_default())
            icon="system_update_alt"
            on_close=Callback::new(move |_| dismiss())
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| dismiss()>{move || i18n.t("update_later")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="download"
                    on_click=move |_| {
                        available_open.set(false);
                        actions.download();
                    }
                >
                    {move || i18n.t("update_now")}
                </Button>
            </DialogFooter>
            {meta}
            <div class="update__notes-title">{move || i18n.t("update_notes_title")}</div>
            <div class="update__notes">{move || notes_view(&notes())}</div>
        </Dialog>
        <Dialog open=ready_open title=Signal::derive(move || i18n.t("update_ready_title")) icon="check_circle_outline">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| ready_open.set(false)>{move || i18n.t("update_restart_later")}</Button>
                <Button
                    variant=Variant::Primary
                    icon="restart_alt"
                    loading=starting
                    disabled=Signal::derive(move || !apply_supported() || starting.get())
                    on_click=move |_| actions.apply(starting)
                >
                    {move || i18n.t("update_restart_now")}
                </Button>
            </DialogFooter>
            <p style="margin:0">
                {move || info().map(|i| i18n.tp("update_ready_message", &[("version", i.version)])).unwrap_or_default()}
            </p>
            <Show when=move || !apply_supported()>
                <p class="hint" style="margin:0">{move || i18n.t("update_dev_mode_disabled")}</p>
            </Show>
        </Dialog>
    }
}

/// A piece of a line of the release notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Plain(String),
    Strong(String),
    Code(String),
}

/// A line of the release notes as the dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Heading(String),
    Item(Vec<Span>),
    Text(Vec<Span>),
}

/// The release notes (Markdown, as `cargo xtask release-notes` writes them) as lines to show:
/// sections, list items and text with bold and code; the title line is the dialog's own.
pub fn parse_notes(markdown: &str) -> Vec<Note> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("# "))
        .map(|line| {
            if let Some(heading) = line.strip_prefix("## ").or_else(|| line.strip_prefix("### ")) {
                Note::Heading(heading.trim().to_string())
            } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                Note::Item(spans(item.trim()))
            } else {
                Note::Text(spans(line))
            }
        })
        .collect()
}

/// `**bold**` and `` `code` `` in a line; a mark left open stays text.
fn spans(line: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        let marked = [("**", Span::Strong as fn(String) -> Span), ("`", Span::Code)].into_iter().find_map(
            |(mark, make)| {
                let inner = rest.strip_prefix(mark)?;
                let end = inner.find(mark).filter(|end| *end > 0)?;
                Some((make(inner[..end].to_string()), &inner[end + mark.len()..]))
            },
        );
        match marked {
            Some((span, after)) => {
                if !plain.is_empty() {
                    out.push(Span::Plain(std::mem::take(&mut plain)));
                }
                out.push(span);
                rest = after;
            }
            None => {
                let c = rest.chars().next().unwrap_or_default();
                plain.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !plain.is_empty() {
        out.push(Span::Plain(plain));
    }
    out
}

fn span_view(span: Span) -> AnyView {
    match span {
        Span::Plain(text) => text.into_any(),
        Span::Strong(text) => view! { <strong>{text}</strong> }.into_any(),
        Span::Code(text) => view! { <code>{text}</code> }.into_any(),
    }
}

/// The release notes as the dialog shows them.
fn notes_view(markdown: &str) -> impl IntoView + use<> {
    parse_notes(markdown)
        .into_iter()
        .map(|note| match note {
            Note::Heading(text) => view! { <div class="update__notes-heading">{text}</div> }.into_any(),
            Note::Item(spans) => {
                view! { <div class="update__notes-item">{spans.into_iter().map(span_view).collect_view()}</div> }.into_any()
            }
            Note::Text(spans) => view! { <p>{spans.into_iter().map(span_view).collect_view()}</p> }.into_any(),
        })
        .collect_view()
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::{AppError, ErrorCode};

    #[test]
    fn release_notes_read_as_text_not_markdown() {
        let notes = "# v0.1.2\n\nChanges since `v0.1.1`.\n\n## Fixes\n\n- **Launch:** A game shows\n* plain item\n\nA **broken bold\n";
        let plain = |s: &str| Span::Plain(s.to_string());
        assert_eq!(
            parse_notes(notes),
            vec![
                Note::Text(vec![plain("Changes since "), Span::Code("v0.1.1".into()), plain(".")]),
                Note::Heading("Fixes".into()),
                Note::Item(vec![Span::Strong("Launch:".into()), plain(" A game shows")]),
                Note::Item(vec![plain("plain item")]),
                Note::Text(vec![plain("A **broken bold")]),
            ]
        );
    }

    fn info() -> UpdateInfo {
        UpdateInfo {
            version: "0.2.0".into(),
            channel: UpdateChannel::Stable,
            notes: String::new(),
            asset_name: "Launcher.exe".into(),
            size: 1,
            published_at: None,
        }
    }

    fn status(state: UpdateState) -> UpdateStatus {
        UpdateStatus { state, ..UpdateStatus::idle(true, "0.1.0".into(), None, true, None) }
    }

    fn key(t: &Text) -> String {
        match t {
            Text::Key { key, .. } => key.clone(),
            Text::Raw { text } => text.clone(),
        }
    }

    #[test]
    fn status_lines_cover_every_state() {
        let off = UpdateStatus::idle(false, "0.1.0".into(), None, false, None);
        assert_eq!(key(&status_line(&off, None)), "launcher_update_status_disabled");
        assert_eq!(key(&status_line(&status(UpdateState::Idle), None)), "launcher_update_status_unknown");
        assert_eq!(
            key(&status_line(&status(UpdateState::Checking), None)),
            "launcher_update_status_checking"
        );
        assert_eq!(key(&status_line(&status(UpdateState::UpToDate), None)), "launcher_update_status_latest");
        let available = status_line(&status(UpdateState::Available { info: info() }), None);
        assert_eq!(key(&available), "launcher_update_status_update_available");
        let downloading = status_line(&status(UpdateState::Downloading { info: info() }), None);
        assert_eq!(key(&downloading), "launcher_update_status_downloading");
        assert_eq!(
            key(&status_line(&status(UpdateState::Ready { info: info() }), None)),
            "launcher_update_status_ready"
        );
        let failed = status(UpdateState::Failed { error: AppError::new(ErrorCode::Network, "x") });
        assert_eq!(key(&status_line(&failed, None)), "launcher_update_status_failed");
        match status_line(&status(UpdateState::UpToDate), Some("12:03".into())) {
            Text::Key { key, params } => {
                assert_eq!(key, "launcher_update_status_checked_at");
                assert_eq!(params["time"], "12:03");
                assert_eq!(params["version"], "0.1.0");
            }
            other => panic!("{other:?}"),
        }
        match available {
            Text::Key { params, .. } => assert_eq!(params["version"], "0.2.0"),
            other => panic!("{other:?}"),
        }
    }
}
