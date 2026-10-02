//! Accounts UI: avatars, profile actions, the Microsoft sign-in dialog and the launch pickers.

pub mod actions;
pub mod auth;
pub mod avatar;
pub mod launch;

use launcher_shared::AccountKind;

/// Translation key of an account type line.
pub fn kind_key(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Microsoft => "microsoft_account",
        AccountKind::Offline => "offline_account",
    }
}

/// "Microsoft Account • Default".
pub fn kind_line(kind: String, default_mark: Option<String>) -> String {
    match default_mark {
        Some(mark) => format!("{kind} • {mark}"),
        None => kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_lines() {
        assert_eq!(kind_key(AccountKind::Microsoft), "microsoft_account");
        assert_eq!(kind_key(AccountKind::Offline), "offline_account");
        assert_eq!(kind_line("Offline Account".into(), None), "Offline Account");
        assert_eq!(
            kind_line("Microsoft Account".into(), Some("Default".into())),
            "Microsoft Account • Default"
        );
    }
}
