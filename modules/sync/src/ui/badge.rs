//! The mark on a build's card and row when it is in a sync group; its tip names the groups.

use leptos::prelude::*;
use ui_kit::Icon;
use ui_kit::i18n::use_i18n;

use super::state;

#[component]
pub fn SyncBadge(key: String) -> impl IntoView {
    let i18n = use_i18n();
    state::ensure();
    let groups = state::groups();
    move || {
        let names: Vec<String> = groups.with(|list| {
            list.iter()
                .flatten()
                .filter(|v| v.group.builds.contains(&key))
                .map(|v| v.group.name.clone())
                .collect()
        });
        (!names.is_empty()).then(|| {
            let tip = i18n.tp("sync_badge_tip", &[("groups", names.join(", "))]);
            view! {
                <span class="sync-mark" role="img" aria-label=tip.clone() data-tip=tip data-tip-side="top">
                    <Icon name="sync_lock" />
                </span>
            }
        })
    }
}
