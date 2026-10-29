//! What the page decides without the window: which account it starts on, which skin is worn,
//! which one the viewer shows.

use crate::dto::{Account, AccountState, LibrarySkin, SkinVariant};

/// The account the page opens on: the launcher's default profile when it is official, else the
/// first official one.
pub fn starting_account(accounts: &[Account]) -> Option<String> {
    accounts.iter().find(|a| a.is_default).or(accounts.first()).map(|a| a.key.clone())
}

/// `current` while it is still among `accounts`, else the one the page starts on.
pub fn kept_account(accounts: &[Account], current: &str) -> Option<String> {
    match accounts.iter().find(|a| a.key == current) {
        Some(account) => Some(account.key.clone()),
        None => starting_account(accounts),
    }
}

/// The library card of the skin the account wears.
pub fn worn_id(state: &AccountState) -> Option<String> {
    state.skin.as_ref().and_then(|s| s.library_id.clone())
}

/// The library card of the skin the account wears, and the model it is worn on.
pub fn worn_as(state: &AccountState) -> Option<(String, SkinVariant)> {
    state.skin.as_ref().and_then(|s| s.library_id.clone().map(|id| (id, s.variant)))
}

/// Library skin `skin` is the one worn, on its model (worn on the other one, it can be put on
/// again).
pub fn is_worn(worn: Option<&(String, SkinVariant)>, skin: &LibrarySkin) -> bool {
    worn.is_some_and(|(id, variant)| *id == skin.id && *variant == skin.variant)
}

/// The cape the account wears.
pub fn active_cape(state: &AccountState) -> Option<String> {
    state.capes.iter().find(|c| c.active).map(|c| c.id.clone())
}

/// The library skin the viewer shows: the one under the pointer, else the one chosen (`None`:
/// the skin the account wears).
pub fn previewed(hovered: Option<&str>, chosen: Option<&str>) -> Option<String> {
    hovered.or(chosen).map(str::to_string)
}

/// The skin the model switch under the figure acts on: the one previewed, else the one worn (the
/// figure shows it then).
pub fn model_target(preview: Option<String>, worn: Option<String>) -> Option<String> {
    preview.or(worn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{Cape, CurrentSkin, SkinVariant};

    fn account(key: &str, is_default: bool) -> Account {
        Account { key: key.into(), name: key.into(), is_default, reauth_required: false }
    }

    fn state(library_id: Option<&str>) -> AccountState {
        AccountState {
            profile: "Notch".into(),
            skin: Some(CurrentSkin {
                sha256: "ab".into(),
                variant: SkinVariant::Classic,
                data: String::new(),
                library_id: library_id.map(str::to_string),
                default: false,
            }),
            capes: vec![Cape { id: "c1".into(), alias: "Pan".into(), active: true, data: String::new() }],
        }
    }

    #[test]
    fn the_default_account_starts() {
        let accounts = [account("alex", false), account("notch", true)];
        assert_eq!(starting_account(&accounts).as_deref(), Some("notch"));
        assert_eq!(starting_account(&accounts[..1]).as_deref(), Some("alex"), "else the first");
        assert_eq!(starting_account(&[]), None);
        assert_eq!(kept_account(&accounts, "alex").as_deref(), Some("alex"), "a chosen account stays");
        assert_eq!(kept_account(&accounts, "gone").as_deref(), Some("notch"));
    }

    #[test]
    fn the_worn_skin_is_marked() {
        assert_eq!(worn_id(&state(Some("s1"))).as_deref(), Some("s1"));
        assert_eq!(worn_id(&state(None)), None, "a skin outside the library marks no card");
        assert_eq!(worn_id(&AccountState { skin: None, ..state(None) }), None);
        assert_eq!(active_cape(&state(None)).as_deref(), Some("c1"));
    }

    #[test]
    fn a_worn_skin_on_another_model_can_be_put_on_again() {
        let card = |variant| LibrarySkin {
            id: "s1".into(),
            name: "Мій".into(),
            variant,
            source: "file".into(),
            sha256: "ab".into(),
            added_ms: 0,
            thumb: String::new(),
        };
        let worn = worn_as(&state(Some("s1")));
        assert_eq!(worn, Some(("s1".to_string(), SkinVariant::Classic)));
        assert!(is_worn(worn.as_ref(), &card(SkinVariant::Classic)));
        assert!(
            !is_worn(worn.as_ref(), &card(SkinVariant::Slim)),
            "worn on the other model: put it on again"
        );
        assert!(!is_worn(None, &card(SkinVariant::Classic)));
    }

    #[test]
    fn the_viewer_shows_the_pointed_then_the_chosen_skin() {
        assert_eq!(previewed(Some("a"), Some("b")).as_deref(), Some("a"));
        assert_eq!(previewed(None, Some("b")).as_deref(), Some("b"));
        assert_eq!(previewed(None, None), None, "then the worn one");
    }

    #[test]
    fn the_model_switch_acts_on_the_figure_s_skin() {
        assert_eq!(model_target(Some("hovered".into()), Some("worn".into())).as_deref(), Some("hovered"));
        assert_eq!(
            model_target(None, Some("worn".into())).as_deref(),
            Some("worn"),
            "nothing picked: the worn one"
        );
        assert_eq!(model_target(None, None), None);
    }
}
