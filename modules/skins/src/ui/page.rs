//! The «Скіни» page: the account switch and the tabs on one glass bar, the 3D figure on the left,
//! the library or the account's capes on the right.

use std::collections::HashMap;

use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::module::use_module_host;
use ui_kit::{
    Button, ChipDef, ChipTabs, EmptyState, Icon, IconAction, LatestRequest, Select, SelectOption, Skeleton,
    Switch, TagTone, Variant, use_toasts,
};

use super::capes::CapesTab;
use super::library::LibraryTab;
use super::model::{active_cape, kept_account, previewed, worn_id};
use super::viewer::SkinStage;
use crate::dto::{Account, AccountState, LibrarySkin, MESSAGE, SkinVariant, error_text};

/// The error is the account needing a new sign-in.
fn needs_sign_in(e: &AppError) -> bool {
    e.params.get(MESSAGE).is_some_and(|m| m == "skins_error_reauth")
}

/// The account switch: the current account's head beside the list of official profiles.
#[component]
fn AccountSwitch(
    accounts: Signal<Vec<Account>>,
    account: RwSignal<String>,
    avatars: RwSignal<HashMap<String, String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let options = Signal::derive(move || {
        accounts.get().into_iter().map(|a| SelectOption::new(a.key, a.name)).collect::<Vec<_>>()
    });
    let head = move || avatars.with(|map| map.get(&account.get()).cloned());
    view! {
        <div class="skins-account">
            <span class="skins-account__head">
                {move || match head() {
                    Some(src) => view! { <img src=src alt="" /> }.into_any(),
                    None => view! { <Icon name="person" /> }.into_any(),
                }}
            </span>
            <div class="skins-account__select" data-tip=move || i18n.t("skins_account") data-tip-side="top">
                <Select options=options value=account />
            </div>
        </div>
    }
}

#[component]
pub fn SkinsPage() -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let host = use_module_host();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let accounts = RwSignal::new(None::<Vec<Account>>);
    let account = RwSignal::new(String::new());
    let tab = RwSignal::new("skins");
    let state = RwSignal::new(None::<Result<AccountState, AppError>>);
    let library = RwSignal::new(None::<Vec<LibrarySkin>>);
    let chosen = RwSignal::new(None::<String>);
    let hovered = RwSignal::new(None::<String>);
    let cape_pick = RwSignal::new(None::<Option<String>>);
    let walking = RwSignal::new(true);
    let pictures = RwSignal::new(HashMap::<String, String>::new());
    let avatars = RwSignal::new(HashMap::<String, String>::new());
    let latest = StoredValue::new_local(LatestRequest::new());

    // The official profiles: now, and after every change of the profiles (a new sign-in).
    let profile_changes = super::api::profile_changes();
    Effect::new(move |_| {
        profile_changes.track();
        spawn_local(async move {
            let list = match super::api::accounts().await {
                Ok(list) => list,
                Err(e) => {
                    toasts.show(Level::Error, i18n.text(&error_text(&e)), None);
                    Vec::new()
                }
            };
            if let Some(key) = kept_account(&list, &account.get_untracked())
                && account.get_untracked() != key
            {
                let _ = account.try_set(key);
            }
            let _ = accounts.try_set(Some(list));
        });
    });
    // Their heads.
    Effect::new(move |_| {
        let keys: Vec<String> = accounts.with(|a| a.iter().flatten().map(|a| a.key.clone()).collect());
        for key in keys.into_iter().filter(|k| !avatars.with_untracked(|m| m.contains_key(k))) {
            spawn_local(async move {
                if let Some(src) = super::api::avatar(&key).await {
                    avatars.try_update(|m| m.insert(key, src));
                }
            });
        }
    });

    let load_library = move || {
        spawn_local(async move {
            match super::api::library().await {
                Ok(list) => {
                    let _ = library.try_set(Some(list));
                }
                Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
            }
        });
    };
    let reauth = Memo::new(move |_| {
        let key = account.get();
        accounts.with(|a| a.iter().flatten().any(|a| a.key == key && a.reauth_required))
    });
    let load_state = move || {
        let key = account.get_untracked();
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        if key.is_empty() || reauth.get_untracked() {
            return;
        }
        spawn_local(async move {
            let answer = super::api::state(&key).await;
            if latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                let _ = state.try_set(Some(answer));
                // A first look at the account may have put its skin in the library.
                load_library();
            }
        });
    };
    // The account's skin and capes, each time another account (or its sign-in) is chosen.
    Effect::new(move |_| {
        account.track();
        reauth.track();
        let _ = state.try_set(None);
        let _ = cape_pick.try_set(None);
        load_state();
    });
    // The library: now, and after every change.
    let changes = super::api::changes();
    Effect::new(move |_| {
        changes.track();
        load_library();
    });
    let reload = Callback::new(move |()| {
        load_state();
        load_library();
    });

    let current = Signal::derive(move || state.with(|s| s.as_ref().and_then(|r| r.as_ref().ok()).cloned()));
    let worn = Signal::derive(move || current.with(|s| s.as_ref().and_then(worn_id)));
    let entry = move |id: &str| library.with(|l| l.iter().flatten().find(|s| s.id == id).cloned());
    // A card deleted under the pointer leaves no preview behind.
    let preview = Memo::new(move |_| {
        previewed(hovered.get().as_deref(), chosen.get().as_deref()).filter(|id| entry(id).is_some())
    });
    // The previewed skin's picture, asked for once.
    Effect::new(move |_| {
        let Some(id) = preview.get() else { return };
        if pictures.with_untracked(|p| p.contains_key(&id)) {
            return;
        }
        spawn_local(async move {
            if let Ok(data) = super::api::skin(&id).await {
                pictures.try_update(|p| p.insert(id, data));
            }
        });
    });
    let worn_picture = move || {
        current.with(|s| {
            s.as_ref().and_then(|s| s.skin.as_ref()).map(|k| (k.data.clone(), k.variant == SkinVariant::Slim))
        })
    };
    let viewer_skin = Memo::new(move |shown: Option<&Option<(String, bool)>>| match preview.get() {
        Some(id) => match pictures.with(|p| p.get(&id).cloned()) {
            Some(data) => Some((data, entry(&id).is_some_and(|e| e.variant == SkinVariant::Slim))),
            // Kept as it is until the picture comes.
            None => shown.cloned().flatten().or_else(worn_picture),
        },
        None => worn_picture(),
    });
    let viewer_cape = Signal::derive(move || {
        current.with(|s| {
            let s = s.as_ref()?;
            let id = cape_pick.get().unwrap_or_else(|| active_cape(s))?;
            s.capes.iter().find(|c| c.id == id).map(|c| c.data.clone()).filter(|d| !d.is_empty())
        })
    });
    let flat = Signal::derive(move || {
        let id = preview.get().or_else(|| worn.get())?;
        entry(&id).map(|e| e.thumb)
    });
    let caption = move || match preview.get().and_then(|id| entry(&id)) {
        Some(e) => e.name,
        None => match current.with(|s| s.as_ref().map(|s| (s.skin.is_some(), worn_id(s)))) {
            Some((true, Some(id))) => entry(&id).map(|e| e.name).unwrap_or_default(),
            Some((false, _)) => i18n.t("skins_default_skin"),
            _ => String::new(),
        },
    };
    // The previewed library skin's model, changed from the switch under the figure.
    let slim = RwSignal::new(false);
    Effect::new(move |_| {
        let model = preview.get().and_then(|id| entry(&id)).map(|e| e.variant == SkinVariant::Slim);
        slim.set(model.unwrap_or(false));
    });
    let set_model = Callback::new(move |on: bool| {
        let Some(id) = preview.get_untracked() else { return };
        let variant = if on { SkinVariant::Slim } else { SkinVariant::Classic };
        spawn_local(async move {
            match super::api::set_variant(id, variant).await {
                Ok(changed) => library.update(|l| {
                    for skin in l.iter_mut().flatten().filter(|s| s.id == changed.id) {
                        *skin = changed.clone();
                    }
                }),
                Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
            }
        });
    });
    let on_state = Callback::new(move |answer: AccountState| {
        state.set(Some(Ok(answer)));
        cape_pick.set(None);
    });

    let chips = vec![
        ChipDef { id: "skins", icon: "checkroom", label: t("skins_tab_skins"), tone: TagTone::Vanilla },
        ChipDef { id: "capes", icon: "flag", label: t("skins_tab_capes"), tone: TagTone::Quilt },
    ];
    let note = move || match state.get() {
        Some(Err(e)) if !needs_sign_in(&e) => Some(view! {
            <div class="skins-note">
                <Icon name="error_outline" />
                <span class="grow">{i18n.text(&error_text(&e))}</span>
                <IconAction icon="refresh" title=t("refresh") on_click=reload />
            </div>
        }),
        _ => None,
    };
    let can_wear = Signal::derive(move || current.with(Option::is_some));
    // Only these two flips build the body anew: the 3D figure lives on through every load.
    let signed_out =
        Memo::new(move |_| reauth.get() || state.with(|s| matches!(s, Some(Err(e)) if needs_sign_in(e))));
    let has_accounts = Memo::new(move |_| accounts.with(|a| a.as_ref().map(|list| !list.is_empty())));
    let account_name = move || {
        accounts
            .with(|a| a.iter().flatten().find(|a| a.key == account.get()).map(|a| a.name.clone()))
            .unwrap_or_default()
    };
    let body = move || {
        if signed_out.get() {
            return view! {
                <EmptyState
                    icon="login"
                    title=Signal::derive(move || i18n.tp("skins_reauth_title", &[("profile", account_name())]))
                    desc=t("skins_reauth_desc")
                >
                    <Button
                        variant=Variant::Primary
                        icon="login"
                        on_click=move |_| {
                            if let Some(host) = host {
                                host.sign_in.run(());
                            }
                        }
                    >
                        {move || i18n.t("profile_sign_in_again")}
                    </Button>
                </EmptyState>
            }
            .into_any();
        }
        view! {
            <div class="skins-body">
                <aside class="skins-stage">
                    <SkinStage skin=viewer_skin cape=viewer_cape walking=walking flat=flat />
                    <div class="skins-stage__caption">{caption}</div>
                    <div class="skins-stage__tools">
                        <span class="skins-stage__switch" data-tip=t("skins_walk_tip") data-tip-side="top">
                            <Icon name="directions_walk" outlined=true />
                            <Switch checked=walking label=t("skins_walk_tip") />
                        </span>
                        <span class="skins-stage__switch" data-tip=t("skins_slim_tip") data-tip-side="top">
                            <Icon name="accessibility_new" outlined=true />
                            <Switch
                                checked=slim
                                label=t("skins_slim_tip")
                                disabled=Signal::derive(move || preview.get().is_none())
                                on_change=set_model
                            />
                        </span>
                    </div>
                </aside>
                <section class="skins-main">
                    {note}
                    {move || match tab.get() {
                        "capes" => view! {
                            <CapesTab state=current account=account cape_pick=cape_pick on_state=on_state />
                        }
                        .into_any(),
                        _ => view! {
                            <LibraryTab
                                library=library
                                account=account
                                worn=worn
                                chosen=chosen
                                hovered=hovered
                                can_wear=can_wear
                                on_state=on_state
                                reload=reload
                            />
                        }
                        .into_any(),
                    }}
                </section>
            </div>
        }
        .into_any()
    };
    view! {
        <div class="page skins-page">
            {move || match has_accounts.get() {
                None => view! { <div class="skins-body"><Skeleton /></div> }.into_any(),
                Some(false) => view! {
                    <EmptyState icon="checkroom" title=t("skins_empty_title") desc=t("skins_empty_desc")>
                        <Button
                            variant=Variant::Primary
                            icon="manage_accounts"
                            on_click=move |_| {
                                if let Some(host) = host {
                                    host.go.run("/profiles".to_string());
                                }
                            }
                        >
                            {move || i18n.t("skins_open_profiles")}
                        </Button>
                    </EmptyState>
                }
                .into_any(),
                Some(true) => view! {
                    <nav class="skins-bar">
                        <ChipTabs chips=chips.clone() active=tab />
                        <AccountSwitch
                            accounts=Signal::derive(move || accounts.get().unwrap_or_default())
                            account=account
                            avatars=avatars
                        />
                    </nav>
                    {body}
                }
                .into_any(),
            }}
        </div>
    }
}
