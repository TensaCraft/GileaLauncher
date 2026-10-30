//! Where answers land once they come back: the page may have been left (or another account
//! chosen) while they were on their way.

use launcher_shared::AppError;
use leptos::prelude::*;
use ui_kit::LatestRequest;

use super::model::kept_account;
use crate::dto::{Account, AccountState, LibrarySkin};

/// The official profiles arrived: the chosen account stays while it is among them.
pub fn land_accounts(
    accounts: RwSignal<Option<Vec<Account>>>,
    account: RwSignal<String>,
    list: Vec<Account>,
) {
    let Some(current) = account.try_get_untracked() else { return };
    if let Some(key) = kept_account(&list, &current)
        && current != key
    {
        let _ = account.try_set(key);
    }
    let _ = accounts.try_set(Some(list));
}

/// A skin's new model arrived.
pub fn land_variant(library: RwSignal<Option<Vec<LibrarySkin>>>, changed: LibrarySkin) {
    let _ = library.try_update(|l| {
        for skin in l.iter_mut().flatten().filter(|s| s.id == changed.id) {
            *skin = changed.clone();
        }
    });
}

/// A change's answer (a skin or a cape put on) arrived: it is shown, and a load begun before it
/// is not.
pub fn land_state(
    state: RwSignal<Option<Result<AccountState, AppError>>>,
    account: RwSignal<String>,
    latest: StoredValue<LatestRequest, LocalStorage>,
    answer: AccountState,
) {
    if account.try_get_untracked().as_deref() != Some(answer.profile.as_str()) {
        return;
    }
    // A load asked for before this change would bring back what was there before it.
    let _ = latest.try_with_value(|l| l.begin());
    let _ = state.try_set(Some(Ok(answer)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::SkinVariant;

    fn account(key: &str) -> Account {
        Account { key: key.into(), name: key.into(), is_default: false, reauth_required: false }
    }

    fn answer(profile: &str) -> AccountState {
        AccountState { profile: profile.into(), skin: None, capes: Vec::new(), skin_unavailable: false }
    }

    fn skin(id: &str, variant: SkinVariant) -> LibrarySkin {
        LibrarySkin {
            id: id.into(),
            name: id.into(),
            variant,
            source: "file".into(),
            sha256: id.into(),
            added_ms: 0,
            thumb: String::new(),
        }
    }

    #[test]
    fn answers_after_the_page_is_gone_land_nowhere() {
        let owner = Owner::new();
        let (accounts, chosen, state, library, latest) = owner.with(|| {
            (
                RwSignal::new(None::<Vec<Account>>),
                RwSignal::new("a".to_string()),
                RwSignal::new(None::<Result<AccountState, AppError>>),
                RwSignal::new(Some(vec![skin("s1", SkinVariant::Classic)])),
                StoredValue::new_local(LatestRequest::new()),
            )
        });
        owner.cleanup();
        land_accounts(accounts, chosen, vec![account("a")]);
        land_variant(library, skin("s1", SkinVariant::Slim));
        land_state(state, chosen, latest, answer("a"));
    }

    #[test]
    fn an_answer_for_another_account_is_dropped() {
        let owner = Owner::new();
        owner.with(|| {
            let (state, chosen) = (RwSignal::new(None), RwSignal::new("b".to_string()));
            let latest = StoredValue::new_local(LatestRequest::new());
            land_state(state, chosen, latest, answer("a"));
            assert!(state.get_untracked().is_none(), "A's skin is not shown under B");
            land_state(state, chosen, latest, answer("b"));
            assert_eq!(state.get_untracked().and_then(Result::ok).map(|s| s.profile).as_deref(), Some("b"));
        });
    }

    #[test]
    fn a_change_s_answer_outlives_an_older_load() {
        let owner = Owner::new();
        owner.with(|| {
            let (state, chosen) = (RwSignal::new(None), RwSignal::new("a".to_string()));
            let latest = StoredValue::new_local(LatestRequest::new());
            let older_load = latest.with_value(|l| l.begin());
            land_state(state, chosen, latest, answer("a"));
            assert!(!latest.with_value(|l| l.is_current(older_load)), "the older load lands nowhere");
        });
    }

    #[test]
    fn a_new_model_replaces_the_card_s() {
        let owner = Owner::new();
        owner.with(|| {
            let library =
                RwSignal::new(Some(vec![skin("s1", SkinVariant::Classic), skin("s2", SkinVariant::Classic)]));
            land_variant(library, skin("s2", SkinVariant::Slim));
            let models: Vec<SkinVariant> =
                library.get_untracked().unwrap().iter().map(|s| s.variant).collect();
            assert_eq!(models, [SkinVariant::Classic, SkinVariant::Slim]);
        });
    }
}
