use launcher_shared::{ActivityEntry, Level, OperationDto};
use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Icon, ProgressBar, Section, progress_percent};

use crate::store::use_store;

pub const ACTIVITY_LIMIT: usize = 100;

/// Newest first; an entry with an already-known `seq` replaces the old one.
pub fn push_activity(list: &mut Vec<ActivityEntry>, entry: ActivityEntry) {
    list.retain(|e| e.seq != entry.seq);
    list.insert(0, entry);
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
pub fn listed(ops: Vec<OperationDto>) -> Vec<OperationDto> {
    ops.into_iter().filter(|op| op.visible).collect()
}

#[component]
pub fn ActivitySection() -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    view! {
        <Section icon="pending_actions" title=t("activity_active_operations") desc=t("activity_active_operations_desc")>
            {move || {
                let ops = listed(store.ops.get().operations);
                if ops.is_empty() {
                    view! { <div class="activity__empty">{i18n.t("activity_no_active_operations")}</div> }.into_any()
                } else {
                    ops.into_iter().map(|op| {
                        let pct = progress_percent(op.progress, op.total);
                        view! {
                            <div class="activity__op">
                                <div class="activity__op-head"><b>{i18n.text(&op.title)}</b><span>{kind_key(&op.kind).map(|k| i18n.t(k))}</span></div>
                                {op.status.as_ref().map(|s| view! { <div class="activity__meta">{i18n.text(s)}</div> })}
                                <ProgressBar value=Signal::derive(move || pct) />
                            </div>
                        }
                    }).collect_view().into_any()
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
        let ids: Vec<u64> = listed(vec![op(1, true), op(2, false)]).iter().map(|o| o.id).collect();
        assert_eq!(ids, [1]);
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
}
