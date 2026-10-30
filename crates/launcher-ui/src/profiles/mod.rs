//! Accounts UI: avatars, profile actions, the Microsoft sign-in dialog and the launch pickers.

pub mod actions;
pub mod auth;
pub mod avatar;
pub mod launch;

use launcher_shared::{AccountKind, ProfileDto};

/// Translation key of an account type line.
pub fn kind_key(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Microsoft => "microsoft_account",
        AccountKind::Offline => "offline_account",
        AccountKind::Yggdrasil => "auth_server_account",
    }
}

/// What an account's kind line says: an account on another auth server names its server.
pub fn kind_text(profile: &ProfileDto, t: impl Fn(&str) -> String) -> String {
    match (profile.kind, &profile.server_name) {
        (AccountKind::Yggdrasil, Some(name)) => name.clone(),
        (kind, _) => t(kind_key(kind)),
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
    fn an_auth_server_account_names_its_server() {
        let mut ely = ProfileDto {
            key: "elyby:Alex".into(),
            name: "Alex".into(),
            id: "7c0d1a2b".into(),
            kind: AccountKind::Yggdrasil,
            is_default: false,
            reauth_required: false,
            reauth_reason: None,
            server: Some("elyby".into()),
            server_name: Some("Ely.by".into()),
        };
        let t = |key: &str| format!("<{key}>");
        assert_eq!(kind_text(&ely, t), "Ely.by");
        ely.server_name = None;
        assert_eq!(kind_text(&ely, t), "<auth_server_account>");
        ely.kind = AccountKind::Offline;
        assert_eq!(kind_text(&ely, t), "<offline_account>");
    }

    #[test]
    fn kind_lines() {
        assert_eq!(kind_key(AccountKind::Microsoft), "microsoft_account");
        assert_eq!(kind_key(AccountKind::Offline), "offline_account");
        assert_eq!(kind_key(AccountKind::Yggdrasil), "auth_server_account");
        assert_eq!(kind_line("Offline Account".into(), None), "Offline Account");
        assert_eq!(
            kind_line("Microsoft Account".into(), Some("Default".into())),
            "Microsoft Account • Default"
        );
    }
}
