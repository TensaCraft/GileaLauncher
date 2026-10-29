//! What the page decides without the window: which account it starts on, which skin is worn,
//! which one the viewer shows.

use crate::dto::{Account, AccountState};

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

/// The cape the account wears.
pub fn active_cape(state: &AccountState) -> Option<String> {
    state.capes.iter().find(|c| c.active).map(|c| c.id.clone())
}

/// The library skin the viewer shows: the one under the pointer, else the one chosen (`None`:
/// the skin the account wears).
pub fn previewed(hovered: Option<&str>, chosen: Option<&str>) -> Option<String> {
    hovered.or(chosen).map(str::to_string)
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
    fn the_viewer_shows_the_pointed_then_the_chosen_skin() {
        assert_eq!(previewed(Some("a"), Some("b")).as_deref(), Some("a"));
        assert_eq!(previewed(None, Some("b")).as_deref(), Some("b"));
        assert_eq!(previewed(None, None), None, "then the worn one");
    }
}
