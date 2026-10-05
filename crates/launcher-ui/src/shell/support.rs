//! Support: one button (the sidebar, About, an error's window) opens every contact the build has.

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, SettingRow, Variant};

use super::use_url_opener;
use crate::store::use_store;

/// A way to reach the launcher's people: its title and description (text keys) and its address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub title: &'static str,
    pub desc: &'static str,
    pub url: String,
}

/// The contacts this build has, Discord first.
pub fn contacts(discord: Option<&str>, issues: Option<&str>) -> Vec<Contact> {
    let contact =
        |title, desc, url: Option<&str>| url.map(|url| Contact { title, desc, url: url.to_string() });
    [
        contact("support_discord", "support_discord_desc", discord),
        contact("support_issues", "support_issues_desc", issues),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The support window, shared by every support button.
#[derive(Clone, Copy)]
pub struct Support {
    open: RwSignal<bool>,
}

impl Support {
    pub fn open(&self) {
        self.open.set(true);
    }
}

pub fn provide_support() -> Support {
    let support = Support { open: RwSignal::new(false) };
    provide_context(support);
    support
}

pub fn use_support() -> Support {
    expect_context::<Support>()
}

/// The build's contacts now (none until the app's info comes).
fn build_contacts() -> Signal<Vec<Contact>> {
    let store = use_store();
    Signal::derive(move || {
        store.info.with(|info| {
            info.as_ref()
                .map(|i| contacts(i.support_url.as_deref(), i.issues_url.as_deref()))
                .unwrap_or_default()
        })
    })
}

/// Whether the build has a contact: support buttons show only then.
pub fn has_contacts() -> Signal<bool> {
    let list = build_contacts();
    Signal::derive(move || !list.with(Vec::is_empty))
}

#[component]
pub fn SupportDialog() -> impl IntoView {
    let i18n = use_i18n();
    let opener = use_url_opener();
    let support = use_support();
    let list = build_contacts();
    let parts = crate::modules::use_module_parts();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    view! {
        <Dialog open=support.open title=t("support_title") icon="support_agent">
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| support.open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            {move || list.get().into_iter().map(|Contact { title, desc, url }| view! {
                <SettingRow title=t(title) desc=t(desc)>
                    <Button variant=Variant::Secondary icon="open_in_new" outlined=true on_click=move |_| opener.open(url.clone())>
                        {move || i18n.t("support_open")}
                    </Button>
                </SettingRow>
            }).collect_view()}
            // Modules' ways to reach the developers (a report of a problem).
            {move || parts.with(|p| p.support_actions.iter().map(|a| (a.view)()).collect_view())}
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contacts_list_what_the_build_has_discord_first() {
        let all = contacts(Some("https://discord.gg/x"), Some("https://github.com/o/r/issues"));
        let titles: Vec<&str> = all.iter().map(|c| c.title).collect();
        assert_eq!(titles, ["support_discord", "support_issues"]);
        assert_eq!(all[1].url, "https://github.com/o/r/issues");
        assert_eq!(contacts(None, Some("https://github.com/o/r/issues")).len(), 1);
        assert!(contacts(None, None).is_empty(), "no contacts, no support button");
    }
}
