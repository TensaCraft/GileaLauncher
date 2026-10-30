//! The «Скіни» page: the account switch and the tabs on one glass bar, the 3D figure on the left,
//! the library or the account's capes on the right.

use std::collections::HashMap;

use launcher_shared::{AppError, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::module::use_module_host;
use ui_kit::{
    Button, ChipDef, ChipTabs, EmptyState, Icon, IconAction, LatestRequest, Select, SelectOption, Size,
    Skeleton, Switch, TagTone, Variant, use_toasts,
};

use super::capes::CapesTab;
use super::land::{land_accounts, land_state, land_variant};
use super::library::LibraryTab;
use super::model::{active_cape, model_target, previewed, worn_as, worn_id};
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
            land_accounts(accounts, account, list);
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

    // An account's head again once its skin is known or changed (put on here, or elsewhere).
    let worn_skin = Memo::new(move |_| {
        state.with(|s| match s {
            Some(Ok(s)) => Some((s.profile.clone(), s.skin.as_ref().map(|k| k.sha256.clone()))),
            _ => None,
        })
    });
    Effect::new(move |_| {
        let Some((key, _)) = worn_skin.get() else { return };
        spawn_local(async move {
            if let Some(src) = super::api::avatar(&key).await {
                avatars.try_update(|m| m.insert(key, src));
            }
        });
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
    let worn_card = Signal::derive(move || current.with(|s| s.as_ref().and_then(worn_as)));
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
        // The worn skin shows on the model its library card has: the switch below previews it.
        None => worn_picture().map(|(data, slim)| {
            (data, worn.get().and_then(|id| entry(&id)).map_or(slim, |e| e.variant == SkinVariant::Slim))
        }),
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
        // A Mojang default skin (or none) is named so; a library skin by its name.
        None => {
            let worn = current.with(|s| {
                s.as_ref().map(|s| (s.skin.as_ref().map(|k| k.default), worn_id(s), s.skin_unavailable))
            });
            match worn {
                Some((_, _, true)) => i18n.t("skins_skin_unavailable"),
                Some((Some(false), Some(id), _)) => entry(&id).map(|e| e.name).unwrap_or_default(),
                Some((None | Some(true), _, _)) => i18n.t("skins_default_skin"),
                _ => String::new(),
            }
        }
    };
    // The model of the skin the figure shows (the previewed one, else the worn one), changed from
    // the «Модель» row under it; a worn skin is then put on again on its new model.
    let target = Memo::new(move |_| model_target(preview.get(), worn.get()).filter(|id| entry(id).is_some()));
    let model = RwSignal::new(SkinVariant::Classic.as_str().to_string());
    Effect::new(move |_| {
        let variant = target.get().and_then(|id| entry(&id)).map_or(SkinVariant::Classic, |e| e.variant);
        model.set(variant.as_str().to_string());
    });
    let models = Signal::derive(move || {
        vec![
            SelectOption::new(SkinVariant::Classic.as_str(), "Classic"),
            SelectOption::new(SkinVariant::Slim.as_str(), "Slim"),
        ]
    });
    let on_state = Callback::new(move |answer: AccountState| {
        land_state(state, account, latest, answer);
        let _ = cape_pick.try_set(None);
    });
    let set_model = Callback::new(move |chosen: String| {
        let Some(id) = target.get_untracked() else { return };
        let (variant, was) = match chosen.as_str() {
            "slim" => (SkinVariant::Slim, SkinVariant::Classic),
            _ => (SkinVariant::Classic, SkinVariant::Slim),
        };
        // The worn skin changes on the account too: it stays worn, on its new model.
        let wearing = worn.get_untracked().as_deref() == Some(id.as_str());
        let profile = account.get_untracked();
        spawn_local(async move {
            match super::api::set_variant(id.clone(), variant).await {
                Ok(changed) => land_variant(library, changed),
                Err(e) => {
                    // The choice showed at the click: it goes back to the model the skin still has.
                    let _ = model.try_set(was.as_str().to_string());
                    return toasts.show(Level::Error, i18n.text(&error_text(&e)), None);
                }
            }
            if wearing {
                match super::api::apply(profile.clone(), id).await {
                    Ok(answer) => {
                        let _ = on_state.try_run(answer);
                        if let Some(host) = host {
                            host.avatar_changed.run(profile);
                        }
                    }
                    Err(e) => toasts.show(Level::Error, i18n.text(&error_text(&e)), None),
                }
            }
        });
    });

    let chips = vec![
        ChipDef { id: "skins", icon: "brand:skin", label: t("skins_tab_skins"), tone: TagTone::Vanilla },
        ChipDef { id: "capes", icon: "brand:cape", label: t("skins_tab_capes"), tone: TagTone::Quilt },
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
                    // Each option its own row, its control at the right edge, as in the settings.
                    <div class="skins-stage__tools">
                        <div class="skins-stage__switch">
                            <Icon name="directions_walk" outlined=true />
                            <span class="grow">{move || i18n.t("skins_walk_tip")}</span>
                            <Switch checked=walking label=t("skins_walk_tip") />
                        </div>
                        <div class="skins-stage__switch">
                            <Icon name="accessibility_new" outlined=true />
                            <span class="grow">{move || i18n.t("skins_model_label")}</span>
                            <div class="skins-stage__model">
                                <Select
                                    options=models
                                    value=model
                                    size=Size::Sm
                                    disabled=Signal::derive(move || target.get().is_none())
                                    on_change=set_model
                                />
                            </div>
                        </div>
                    </div>
                </aside>
                <section class="skins-main">
                    {note}
                    {move || match tab.get() {
                        "capes" => view! {
                            <CapesTab
                                state=current
                                failed=Signal::derive(move || state.with(|s| matches!(s, Some(Err(_)))))
                                account=account
                                cape_pick=cape_pick
                                on_state=on_state
                            />
                        }
                        .into_any(),
                        _ => view! {
                            <LibraryTab
                                library=library
                                account=account
                                worn=worn_card
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
                    <EmptyState icon="brand:skin" title=t("skins_empty_title") desc=t("skins_empty_desc")>
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
                    <nav class="content__nav">
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
