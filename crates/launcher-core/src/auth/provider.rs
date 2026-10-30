//! Accounts a module brings (Tensa's site): the module signs the player in on its site, checks the
//! session before a launch and ends it; the core keeps the account, sealed like the others, and
//! launches it as an offline player with the name and UUID the site vouches for.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use launcher_shared::AppResult;

use super::Hidden;

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// Who the site says signed in: their game UUID, their nickname and the site's token.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderAccount {
    pub id: String,
    pub name: String,
    pub token: String,
}

impl fmt::Debug for ProviderAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderAccount")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("token", &Hidden(true))
            .finish()
    }
}

pub trait AccountProvider: Send + Sync {
    /// The id its accounts name (`tensa`): their key is `<id>:<nickname>`.
    fn id(&self) -> &'static str;

    /// What the account's card calls it.
    fn name(&self) -> String;

    /// Signs in: `AuthCredentialsInvalid` for a wrong login or password, `AuthCodeRequired` when
    /// the site asks for the second factor (`code`).
    fn sign_in<'a>(
        &'a self,
        login: &'a str,
        password: &'a str,
        code: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderAccount>;

    /// The account behind `token` now; `ReauthRequired` when the site ended it.
    fn verify<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ProviderAccount>;

    /// Ends the session on the site.
    fn sign_out<'a>(&'a self, token: &'a str) -> ProviderFuture<'a, ()>;

    /// Where the site keeps the skin of player `name` (a PNG), if it keeps skins.
    fn skin_url(&self, name: &str) -> Option<String> {
        let _ = name;
        None
    }
}
