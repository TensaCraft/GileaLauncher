//! The «Плащі» tab: every cape the account has, and none.

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, EmptyState, Icon, Skeleton, Tag, TagTone, Variant, use_toasts};

use super::api;
use crate::dto::{AccountState, Cape, error_text};

#[component]
pub fn CapesTab(
    #[prop(into)] state: Signal<Option<AccountState>>,
    account: RwSignal<String>,
    /// The cape the viewer shows (`Some(None)`: none); `None`: the one worn.
    cape_pick: RwSignal<Option<Option<String>>>,
    on_state: Callback<AccountState>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    // The cape being put on (`Some(None)`: being taken off).
    let busy = RwSignal::new(None::<Option<String>>);
    let wear = move |cape: Option<String>, alias: String| {
        if busy.get_untracked().is_some() {
            return;
        }
        busy.set(Some(cape.clone()));
        let profile = account.get_untracked();
        spawn_local(async move {
            match api::set_cape(profile, cape.clone()).await {
                Ok(answer) => {
                    let _ = on_state.try_run(answer);
                    let said = match cape {
                        Some(_) => i18n.tp("skins_cape_applied", &[("name", alias)]),
                        None => i18n.t("skins_cape_removed"),
                    };
                    toasts.show(Level::Success, said, None);
                }
                Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
            }
            let _ = busy.try_set(None);
        });
    };
    // A card: `cape` is `None` for «Без плаща».
    let card = move |cape: Option<Cape>, active: bool| {
        let id = cape.as_ref().map(|c| c.id.clone());
        let alias = cape.as_ref().map(|c| c.alias.clone()).unwrap_or_else(|| i18n.t("skins_no_cape"));
        let picture = cape.as_ref().map(|c| c.data.clone()).filter(|d| !d.is_empty());
        let (for_pick, for_chosen, for_busy, for_wear) = (id.clone(), id.clone(), id.clone(), id);
        let (tip, shown_alias) = (alias.clone(), alias.clone());
        view! {
            <div
                class="skins-card skins-cape"
                class:is-chosen=move || cape_pick.get().as_ref() == Some(&for_chosen)
                class:is-worn=active
                on:click=move |_| cape_pick.set(Some(for_pick.clone()))
            >
                <div class="skins-card__picture">
                    {match picture {
                        Some(src) => view! {
                            <div class="skins-cape__front" style=format!("background-image:url({src})")></div>
                        }
                        .into_any(),
                        None => view! { <div class="skins-cape__none"><Icon name="block" outlined=true /></div> }.into_any(),
                    }}
                </div>
                <div class="skins-card__name" data-tip=tip data-tip-side="top">{shown_alias}</div>
                <div class="skins-card__tags">
                    {active.then(|| view! { <Tag tone=TagTone::Vanilla>{move || i18n.t("skins_active")}</Tag> })}
                </div>
                <div class="skins-card__actions">
                    <Button
                        variant=Variant::Primary
                        class="skins-card__wear"
                        loading=Signal::derive(move || busy.get().as_ref() == Some(&for_busy))
                        disabled=Signal::derive(move || active || busy.get().is_some())
                        on_click=move |_| wear(for_wear.clone(), alias.clone())
                    >
                        {move || i18n.t("skins_wear")}
                    </Button>
                </div>
            </div>
        }
    };
    move || match state.get() {
        None => view! { <div class="skins-grid"><Skeleton /></div> }.into_any(),
        Some(state) if state.capes.is_empty() => view! {
            <EmptyState icon="flag" title=Signal::derive(move || i18n.t("skins_capes_empty")) />
        }
        .into_any(),
        Some(state) => {
            let none_on = state.capes.iter().all(|c| !c.active);
            view! {
                <div class="skins-grid">
                    {card(None, none_on)}
                    {state.capes.into_iter().map(|c| { let on = c.active; card(Some(c), on) }).collect_view()}
                </div>
            }
            .into_any()
        }
    }
}
