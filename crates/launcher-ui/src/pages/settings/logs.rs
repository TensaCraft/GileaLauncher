//! The launcher log viewer: the newest records of `app.log`, coloured by level, filtered by level
//! and by text.

use launcher_shared::{AppError, Level, LogEntry, LogLevel, LogView};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{
    Button, ChipDef, ChipTabs, Dialog, DialogFooter, IconAction, Tag, TagTone, TextInput, Variant, ipc,
    use_toasts,
};

use crate::shell::copy_text;

/// The most records drawn at once.
pub const SHOWN: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelFilter {
    All,
    Errors,
    Warnings,
    Info,
    Debug,
}

impl LevelFilter {
    pub fn from_id(id: &str) -> Self {
        match id {
            "error" => Self::Errors,
            "warn" => Self::Warnings,
            "info" => Self::Info,
            "debug" => Self::Debug,
            _ => Self::All,
        }
    }

    pub fn admits(self, level: Option<LogLevel>) -> bool {
        match self {
            Self::All => true,
            Self::Errors => level == Some(LogLevel::Error),
            Self::Warnings => level == Some(LogLevel::Warn),
            Self::Info => level == Some(LogLevel::Info),
            Self::Debug => matches!(level, Some(LogLevel::Debug | LogLevel::Trace)),
        }
    }
}

pub fn level_name(level: Option<LogLevel>) -> &'static str {
    match level {
        Some(LogLevel::Error) => "ERROR",
        Some(LogLevel::Warn) => "WARN",
        Some(LogLevel::Info) => "INFO",
        Some(LogLevel::Debug) => "DEBUG",
        Some(LogLevel::Trace) => "TRACE",
        None => "",
    }
}

pub fn level_tone(level: Option<LogLevel>) -> TagTone {
    match level {
        Some(LogLevel::Error) => TagTone::Danger,
        Some(LogLevel::Warn) => TagTone::Warn,
        Some(LogLevel::Info) => TagTone::Info,
        Some(LogLevel::Debug | LogLevel::Trace) => TagTone::Quilt,
        None => TagTone::Neutral,
    }
}

pub fn row_class(level: Option<LogLevel>) -> &'static str {
    match level {
        Some(LogLevel::Error) => "logs__row is-error",
        Some(LogLevel::Warn) => "logs__row is-warn",
        _ => "logs__row",
    }
}

/// What a record is searched by — its time, level and every line of its message — in lower case,
/// made once per load.
pub fn haystack(entry: &LogEntry) -> String {
    format!("{} {} {}", entry.time, level_name(entry.level), entry.message).to_lowercase()
}

/// The search text as `matches_query` wants it.
pub fn needle(query: &str) -> String {
    query.trim().to_lowercase()
}

pub fn matches_query(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.contains(needle)
}

/// The positions of the newest `limit` records that pass the filter and the search, and how many
/// matching ones were left out.
pub fn visible(
    entries: &[LogEntry],
    haystacks: &[String],
    filter: LevelFilter,
    query: &str,
    limit: usize,
) -> (Vec<usize>, usize) {
    let needle = needle(query);
    let matching: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            filter.admits(entries[i].level) && haystacks.get(i).is_some_and(|h| matches_query(h, &needle))
        })
        .collect();
    let hidden = matching.len().saturating_sub(limit);
    (matching[hidden..].to_vec(), hidden)
}

/// How many records a filter admits.
pub fn count(entries: &[LogEntry], filter: LevelFilter) -> usize {
    entries.iter().filter(|e| filter.admits(e.level)).count()
}

/// The footer's notes: how many matching records the list leaves out, and how many older records
/// the viewer never received (they stay in the file).
pub fn footer_notes(
    shown: usize,
    hidden: usize,
    skipped: usize,
) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let mut notes = Vec::new();
    if hidden > 0 {
        notes.push((
            "log_shown_last",
            vec![("shown", shown.to_string()), ("total", (shown + hidden).to_string())],
        ));
    }
    if skipped > 0 {
        notes.push(("log_older_not_loaded", vec![("count", skipped.to_string())]));
    }
    notes
}

/// The records as the log file has them, for the clipboard.
pub fn as_text<'a>(entries: impl IntoIterator<Item = &'a LogEntry>) -> String {
    entries
        .into_iter()
        .map(|e| match e.level {
            Some(_) => format!("{} {:<8} {}", e.time, level_name(e.level), e.message),
            None => e.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pause after typing before the list follows the search.
const SEARCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(150);

#[derive(Clone)]
enum Load {
    Loading,
    /// The records with their `haystack`s.
    Ready(LogView, Vec<String>),
    Failed(AppError),
}

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

/// Shows the file selected in its folder.
pub fn reveal(path: String, on_error: impl Fn(AppError) + 'static) {
    spawn_local(async move {
        if let Err(e) = ipc::invoke::<_, ()>("reveal_path", &PathArgs { path }).await {
            on_error(e);
        }
    });
}

#[component]
pub fn LogViewer(open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let state = RwSignal::new(Load::Loading);
    let filter = RwSignal::new("all");
    let query = RwSignal::new(String::new());
    // The search the list follows: the typed text after a pause.
    let search = RwSignal::new(String::new());
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let list = NodeRef::<html::Div>::new();

    let load = move || {
        state.set(Load::Loading);
        spawn_local(async move {
            let next = match ipc::call::<LogView>("log_view").await {
                Ok(view) => {
                    let haystacks = view.entries.iter().map(haystack).collect();
                    Load::Ready(view, haystacks)
                }
                Err(e) => Load::Failed(e),
            };
            let _ = state.try_set(next);
        });
    };
    Effect::new(move |_| {
        if open.get() {
            load();
        }
    });
    let cancel_timer = move || {
        if let Some(handle) = timer.try_get_value().flatten() {
            handle.clear();
        }
    };
    Effect::new(move |_| {
        let text = query.get();
        cancel_timer();
        if text.trim().is_empty() {
            search.set(String::new());
        } else {
            timer.set_value(set_timeout_with_handle(move || search.set(text), SEARCH_PAUSE).ok());
        }
    });
    on_cleanup(cancel_timer);

    let shown = Memo::new(move |_| {
        let filter = LevelFilter::from_id(filter.get());
        let search = search.get();
        state.with(|s| match s {
            Load::Ready(view, haystacks) => visible(&view.entries, haystacks, filter, &search, SHOWN),
            _ => (Vec::new(), 0),
        })
    });
    // The record at a position of the loaded log.
    let entry_at = move |i: usize| {
        state.with(|s| match s {
            Load::Ready(view, _) => view.entries.get(i).cloned(),
            _ => None,
        })
    };
    // The newest records are at the bottom: keep the list scrolled there.
    Effect::new(move |_| {
        shown.track();
        if let Some(el) = list.get() {
            leptos::leptos_dom::helpers::request_animation_frame(move || {
                el.set_scroll_top(el.scroll_height())
            });
        }
    });

    let chip = move |id: &'static str, icon: &'static str, key: &'static str, tone: TagTone| {
        let admits = LevelFilter::from_id(id);
        ChipDef {
            id,
            icon,
            tone,
            label: Signal::derive(move || {
                let n = state.with(|s| match s {
                    Load::Ready(view, _) => count(&view.entries, admits),
                    _ => 0,
                });
                format!("{} · {n}", i18n.t(key))
            }),
        }
    };
    let chips = StoredValue::new(vec![
        chip("all", "list", "log_filter_all", TagTone::Neutral),
        chip("error", "error", "log_filter_errors", TagTone::Danger),
        chip("warn", "warning", "log_filter_warnings", TagTone::Warn),
        chip("info", "info", "log_filter_info", TagTone::Info),
        chip("debug", "bug_report", "log_filter_debug", TagTone::Quilt),
    ]);

    let copy = move || {
        let (text, lines) = shown.with(|(rows, _)| {
            state.with(|s| match s {
                Load::Ready(view, _) => {
                    (as_text(rows.iter().filter_map(|&i| view.entries.get(i))), rows.len())
                }
                _ => (String::new(), 0),
            })
        });
        spawn_local(async move {
            if copy_text(text).await {
                toasts.show(Level::Success, i18n.tp("log_copied", &[("count", lines.to_string())]), None);
            }
        });
    };
    let file = move || {
        state.with(|s| match s {
            Load::Ready(view, _) => view.file.clone(),
            _ => String::new(),
        })
    };
    let reveal_file = move || {
        let path = file();
        if !path.is_empty() {
            reveal(path, move |e| toasts.show(Level::Error, i18n.error(&e), None));
        }
    };
    let hint = Signal::derive(move || {
        let skipped = state.with(|s| match s {
            Load::Ready(view, _) => view.skipped,
            _ => 0,
        });
        shown.with(|(rows, hidden)| {
            footer_notes(rows.len(), *hidden, skipped)
                .into_iter()
                .map(|(key, params)| i18n.tp(key, &params))
                .collect::<Vec<_>>()
                .join(" · ")
        })
    });

    view! {
        <Dialog open=open full=true icon="receipt_long" title=t("log_viewer_title") subtitle=Signal::derive(file) footer_hint=hint>
            <div class="logs">
                <div class="logs__bar">
                    <ChipTabs chips=chips.get_value() active=filter />
                    <div class="logs__tools">
                        <TextInput value=query icon="search" placeholder=t("log_search") />
                        <IconAction icon="refresh" title=t("refresh") on_click=Callback::new(move |()| load()) />
                        <IconAction icon="content_copy" title=t("copy") on_click=Callback::new(move |()| copy()) />
                        <IconAction icon="folder_open" title=t("log_reveal") on_click=Callback::new(move |()| reveal_file()) />
                    </div>
                </div>
                <div class="logs__list" node_ref=list>
                    {move || state.with(|s| match s {
                        Load::Loading => view! { <div class="logs__note">{i18n.t("log_loading")}</div> }.into_any(),
                        Load::Failed(e) => {
                            let text = i18n.tp("log_load_failed", &[("error", i18n.error(e))]);
                            view! { <div class="logs__note is-error">{text}</div> }.into_any()
                        }
                        Load::Ready(view, _) if view.entries.is_empty() => {
                            view! { <div class="logs__note">{i18n.t("log_empty")}</div> }.into_any()
                        }
                        Load::Ready(..) => view! {
                            // Keyed by position: a narrower search only takes rows away.
                            <For each=move || shown.get().0 key=|i| *i let:i>
                                {entry_at(i).map(|entry| view! { <LogRow entry=entry /> })}
                            </For>
                            <Show when=move || shown.with(|(rows, _)| rows.is_empty())>
                                <div class="logs__note">{move || i18n.t("log_no_matches")}</div>
                            </Show>
                        }
                        .into_any(),
                    })}
                </div>
            </div>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
        </Dialog>
    }
}

#[component]
fn LogRow(entry: LogEntry) -> impl IntoView {
    view! {
        <div class=row_class(entry.level)>
            <span class="logs__time">{entry.time.clone()}</span>
            <span class="logs__level">
                {entry.level.map(|level| view! { <Tag tone=level_tone(Some(level))>{level_name(Some(level))}</Tag> })}
            </span>
            <span class="logs__msg">{entry.message}</span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(level: Option<LogLevel>, message: &str) -> LogEntry {
        LogEntry { time: "2026-09-27 12:00:00".into(), level, message: message.into() }
    }

    #[test]
    fn filters_pick_their_levels() {
        assert!(LevelFilter::All.admits(None));
        assert!(LevelFilter::Errors.admits(Some(LogLevel::Error)));
        assert!(!LevelFilter::Errors.admits(Some(LogLevel::Warn)));
        assert!(
            LevelFilter::Debug.admits(Some(LogLevel::Debug))
                && LevelFilter::Debug.admits(Some(LogLevel::Trace))
        );
        assert!(!LevelFilter::Info.admits(None));
        assert_eq!(LevelFilter::from_id("warn"), LevelFilter::Warnings);
        assert_eq!(LevelFilter::from_id("anything"), LevelFilter::All);
    }

    #[test]
    fn search_ignores_case_and_reaches_continuation_lines() {
        let hay = haystack(&e(Some(LogLevel::Error), "launch failed\nCaused by: Java NOT found"));
        assert!(matches_query(&hay, &needle("JAVA not")));
        assert!(matches_query(&hay, &needle("  ")));
        assert!(matches_query(&hay, &needle("error")), "the level name is searchable");
        assert!(matches_query(&hay, &needle("12:00")), "and the time");
        assert!(!matches_query(&hay, &needle("modrinth")));
    }

    #[test]
    fn only_the_newest_matching_records_are_shown() {
        let entries: Vec<LogEntry> = (0..6)
            .map(|i| e(Some(if i % 2 == 0 { LogLevel::Info } else { LogLevel::Error }), &format!("m{i}")))
            .collect();
        let hays: Vec<String> = entries.iter().map(haystack).collect();
        let (rows, hidden) = visible(&entries, &hays, LevelFilter::Errors, "", 2);
        assert_eq!((rows, hidden), (vec![3, 5], 1), "positions in the loaded records, newest last");
        assert_eq!(visible(&entries, &hays, LevelFilter::All, " M4 ", 10).0, [4]);
        assert_eq!(count(&entries, LevelFilter::Info), 3);
    }

    #[test]
    fn copied_text_reads_like_the_file() {
        let rows = [e(Some(LogLevel::Warn), "slow"), e(None, "raw")];
        assert_eq!(as_text(&rows), "2026-09-27 12:00:00 WARN     slow\nraw");
    }

    #[test]
    fn the_search_shrinks_before_the_toolbar_wraps() {
        let css = include_str!("../../../styles/shell.css");
        assert!(css.contains(
            ".logs__tools { margin-left: auto; flex: 1 1 260px; min-width: 0; display: flex; align-items: center; justify-content: flex-end; gap: 8px; }"
        ));
        assert!(css.contains(".logs__tools .control { flex: 0 1 240px; min-width: 140px; }"));
        assert!(css.contains(".logs__tools .action { flex: none; }"), "square actions keep their size");
    }

    #[test]
    fn the_footer_says_what_is_left_out() {
        assert!(footer_notes(12, 0, 0).is_empty());
        assert_eq!(
            footer_notes(1000, 20, 0),
            vec![("log_shown_last", vec![("shown", "1000".to_string()), ("total", "1020".to_string())])]
        );
        assert_eq!(
            footer_notes(3, 0, 11000),
            vec![("log_older_not_loaded", vec![("count", "11000".to_string())])],
            "older records never loaded are named even when the list shows everything it has"
        );
        assert_eq!(footer_notes(1000, 5, 7).len(), 2);
    }

    #[test]
    fn levels_have_their_colours() {
        assert_eq!(level_tone(Some(LogLevel::Error)), TagTone::Danger);
        assert_eq!(level_tone(Some(LogLevel::Warn)), TagTone::Warn);
        assert_eq!(level_tone(Some(LogLevel::Info)), TagTone::Info);
        assert_eq!(level_tone(Some(LogLevel::Trace)), TagTone::Quilt);
        assert_eq!(row_class(Some(LogLevel::Error)), "logs__row is-error");
        assert_eq!(row_class(None), "logs__row");
    }
}
