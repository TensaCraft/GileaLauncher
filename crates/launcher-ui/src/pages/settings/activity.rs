use launcher_shared::{ActivityEntry, Level, OperationDto};
use leptos::prelude::*;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Icon, ProgressBar, Section, progress_percent};

use crate::store::{AppStore, use_store};

pub const ACTIVITY_LIMIT: usize = 100;

/// Newest first; an entry with an already-known `seq` replaces the old one.
pub fn push_activity(list: &mut Vec<ActivityEntry>, entry: ActivityEntry) {
    list.retain(|e| e.seq != entry.seq);
    list.insert(0, entry);
    list.truncate(ACTIVITY_LIMIT);
}

/// The entries asked for (`recent`, newest first) joined with those already in `list` (their events
/// came first): newest first, once each (the asked one wins), capped.
pub fn merge_activity(list: &mut Vec<ActivityEntry>, recent: Vec<ActivityEntry>) {
    list.retain(|e| !recent.iter().any(|r| r.seq == e.seq));
    list.extend(recent);
    list.sort_by_key(|e| std::cmp::Reverse(e.seq));
    list.truncate(ACTIVITY_LIMIT);
}

fn level_icon(level: Level) -> (&'static str, &'static str) {
    match level {
        Level::Success => ("check_circle_outline", "var(--ok)"),
        Level::Warning => ("warning_amber", "var(--warn)"),
        Level::Error => ("error_outline", "var(--danger)"),
        Level::Info => ("info", "var(--primary)"),
    }
}

fn time_of(at_ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at_ms as f64));
    format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds())
}

/// The translation key of an operation's kind; a kind no one named yet shows nothing.
pub fn kind_key(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "install" => "op_kind_install",
        "copy" => "op_kind_copy",
        "components" => "op_kind_components",
        "launch" => "op_kind_launch",
        "update" => "op_kind_update",
        "modrinth" => "op_kind_modrinth",
        "curseforge" => "op_kind_curseforge",
        "sync" => "op_kind_sync",
        "backup" => "op_kind_backup",
        _ => return None,
    })
}

/// The operations the section lists: the ones the player is meant to see.
pub fn listed(ops: &[OperationDto]) -> Vec<OperationDto> {
    ops.iter().filter(|op| op.visible).cloned().collect()
}

/// How far the work under way is, all of it: the mean of the operations' own percentages (each
/// counts its own bytes or steps); `None` while none is measured.
pub fn overall_percent(ops: &[OperationDto]) -> Option<f64> {
    let measured: Vec<f64> = ops.iter().filter_map(|op| progress_percent(op.progress, op.total)).collect();
    (!measured.is_empty()).then(|| measured.iter().sum::<f64>() / measured.len() as f64)
}

/// The operations listed, as the store has them now.
pub fn listed_ops(store: AppStore) -> Memo<Vec<OperationDto>> {
    Memo::new(move |_| store.ops.with(|o| listed(&o.operations)))
}

/// What a row shows of one operation, read as the operation moves: a row stays while its
/// operation's progress ticks (the operation's own words, and its place, stay).
#[derive(Clone, Copy)]
pub struct LiveOp {
    pub title: Memo<String>,
    pub status: Memo<Option<String>>,
    /// Percent done; `None` without a total.
    pub percent: Memo<Option<f64>>,
}

/// Operation `id` among the `listed` ones.
pub fn live_op(i18n: I18nCtx, listed: Memo<Vec<OperationDto>>, id: u64) -> LiveOp {
    let now = Memo::new(move |_| listed.with(|l| l.iter().find(|o| o.id == id).cloned()));
    LiveOp {
        title: Memo::new(move |_| now.with(|o| o.as_ref().map(|o| i18n.text(&o.title))).unwrap_or_default()),
        status: Memo::new(move |_| {
            now.with(|o| o.as_ref().and_then(|o| o.status.as_ref()).map(|s| i18n.text(s)))
        }),
        percent: Memo::new(move |_| {
            now.with(|o| o.as_ref().and_then(|o| progress_percent(o.progress, o.total)))
        }),
    }
}

#[component]
pub fn ActivitySection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let ops = listed_ops(store);
    let none = Memo::new(move |_| ops.with(Vec::is_empty));
    view! {
        <Section icon="pending_actions" title=t("activity_active_operations") desc=t("activity_active_operations_desc")>
            {move || {
                if none.get() {
                    view! { <div class="activity__empty">{i18n.t("activity_no_active_operations")}</div> }.into_any()
                } else {
                    view! {
                        <For
                            each=move || ops.get()
                            key=|op| op.id
                            children=move |op| {
                                let live = live_op(i18n, ops, op.id);
                                view! {
                                    <div class="activity__op">
                                        <div class="activity__op-head"><b>{move || live.title.get()}</b><span>{kind_key(&op.kind).map(|k| i18n.t(k))}</span></div>
                                        <Show when=move || live.status.with(Option::is_some)>
                                            <div class="activity__meta">{move || live.status.get()}</div>
                                        </Show>
                                        <ProgressBar value=live.percent />
                                    </div>
                                }
                            }
                        />
                    }
                    .into_any()
                }
            }}
        </Section>
        <Section icon="history" title=t("activity_recent_events") desc=t("activity_recent_events_desc")>
            {move || {
                let list = store.activity.get();
                if list.is_empty() {
                    view! { <div class="activity__empty">{i18n.t("activity_empty")}</div> }.into_any()
                } else {
                    list.into_iter().map(|e| {
                        let (icon, color) = level_icon(e.level);
                        let meta = match e.kind.as_deref().and_then(kind_key) {
                            Some(key) => format!("{} · {}", time_of(e.at_ms), i18n.t(key)),
                            None => time_of(e.at_ms),
                        };
                        view! {
                            <div class="activity__row">
                                <span style=format!("color:{color}")><Icon name=icon size=16 /></span>
                                <span class="activity__msg">{i18n.text(&e.message)}</span>
                                <span class="activity__meta">{meta}</span>
                            </div>
                        }
                    }).collect_view().into_any()
                }
            }}
        </Section>
    }
}

#[cfg(test)]
mod kind_tests {
    use launcher_shared::Text;

    use super::*;

    /// The kinds of operations the launcher and its modules run (`OperationSpec::kind`).
    const KINDS: [&str; 9] =
        ["install", "copy", "components", "launch", "update", "modrinth", "curseforge", "sync", "backup"];

    fn lang(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn operation_kinds_are_words() {
        let (uk, en) = (
            lang(include_str!("../../../../../assets/langs/uk_UA.json")),
            lang(include_str!("../../../../../assets/langs/en_US.json")),
        );
        for kind in KINDS {
            let key = kind_key(kind).unwrap_or_else(|| panic!("{kind} has no word"));
            assert!(uk[key].is_string() && en[key].is_string(), "{key}");
        }
        assert_eq!(kind_key("something_new"), None);
    }

    #[test]
    fn hidden_operations_are_not_listed() {
        let op = |id: u64, visible: bool| OperationDto {
            id,
            parent_id: None,
            title: Text::key("x"),
            kind: "launch".into(),
            status: None,
            progress: None,
            total: None,
            visible,
        };
        let ids: Vec<u64> = listed(&[op(1, true), op(2, false)]).iter().map(|o| o.id).collect();
        assert_eq!(ids, [1]);
    }

    #[test]
    fn the_ring_shows_all_the_work_under_way() {
        let op = |id: u64, progress: Option<f64>, total: Option<f64>| OperationDto {
            id,
            parent_id: None,
            title: Text::key("x"),
            kind: "sync".into(),
            status: None,
            progress,
            total,
            visible: true,
        };
        // Two builds synced at once: 37 % and 29 % of their own bytes.
        let two = [op(1, Some(37.0), Some(100.0)), op(2, Some(290.0), Some(1000.0))];
        assert_eq!(overall_percent(&two), Some(33.0));
        let unknown = [op(1, Some(50.0), Some(100.0)), op(2, None, None)];
        assert_eq!(overall_percent(&unknown), Some(50.0), "one without a total does not count");
        assert_eq!(overall_percent(&[op(1, None, None)]), None, "nothing measured: the ring spins");
        assert_eq!(overall_percent(&[]), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::{ActivityEvent, Text};

    fn entry(seq: u64) -> ActivityEntry {
        ActivityEntry {
            seq,
            at_ms: 0,
            event: ActivityEvent::Notify,
            level: Level::Info,
            message: Text::raw(seq.to_string()),
            operation_id: None,
            kind: None,
        }
    }

    #[test]
    fn newest_first_deduplicated_and_capped() {
        let mut list = Vec::new();
        for seq in 1..=120 {
            push_activity(&mut list, entry(seq));
        }
        push_activity(&mut list, entry(120));
        assert_eq!(list.len(), ACTIVITY_LIMIT);
        assert_eq!(list[0].seq, 120);
        assert_eq!(list[1].seq, 119);
    }

    #[test]
    fn the_entries_asked_for_at_start_join_those_already_heard() {
        // The list asked for at start arrives after an entry its event brought: both stay, newest
        // first, once each.
        let mut list = Vec::new();
        push_activity(&mut list, entry(5));
        push_activity(&mut list, entry(4));
        merge_activity(&mut list, vec![entry(4), entry(3), entry(2)]);
        assert_eq!(list.iter().map(|e| e.seq).collect::<Vec<_>>(), [5, 4, 3, 2]);
        let mut full = Vec::new();
        merge_activity(&mut full, (1..=150).rev().map(entry).collect());
        assert_eq!((full.len(), full[0].seq), (ACTIVITY_LIMIT, 150));
    }
}
