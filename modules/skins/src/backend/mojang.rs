//! Minecraft Services: an account's skins and capes, read and changed with its token. A player's
//! skin by nickname is the core's (`launcher_core::skins`).

use std::time::Duration;

use launcher_core::net::{api_client, send_patiently};
use launcher_core::skins::service::is_nickname;
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{RequestBuilder, Url};
use serde::Deserialize;
use serde_json::Value;

use crate::dto::{MESSAGE, SkinVariant};

const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(20);

/// Where Minecraft Services is; replaced in the tests by a fake server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub services: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints { services: "https://api.minecraftservices.com".into() }
    }
}

impl Endpoints {
    /// The service at `base` (a fake server).
    pub fn local(base: &str) -> Self {
        Endpoints { services: base.into() }
    }
}

/// What Minecraft Services says an account wears and has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServicesProfile {
    pub skins: Vec<RemoteSkin>,
    pub capes: Vec<RemoteCape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkin {
    pub id: String,
    /// One of Minecraft's own skins (`STEVE`, `ALEX`…), when the profile names it so.
    pub alias: Option<String>,
    /// `ACTIVE` for the one worn.
    pub state: String,
    pub url: String,
    pub variant: SkinVariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCape {
    pub id: String,
    /// `ACTIVE` for the one worn.
    pub state: String,
    pub url: String,
    pub alias: String,
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// 401: the token is no longer taken.
    Unauthorized,
    /// 429: too many changes for now.
    Rate,
    /// 400: the skin was refused.
    Rejected,
    /// 400 to a cape: the cape was refused.
    RejectedCape,
    Other(AppError),
}

impl From<CallError> for AppError {
    /// The error the player reads; `Unauthorized` (a token refused even after a refresh) asks for a
    /// new sign-in.
    fn from(e: CallError) -> AppError {
        match e {
            CallError::Unauthorized => {
                AppError::new(ErrorCode::ReauthRequired, "Minecraft Services: HTTP 401")
                    .with_param(MESSAGE, "skins_error_reauth")
            }
            CallError::Rate => AppError::new(ErrorCode::RateLimited, "Minecraft Services: HTTP 429")
                .with_param(MESSAGE, "skins_error_rate"),
            CallError::Rejected => AppError::new(ErrorCode::InvalidInput, "Minecraft Services: HTTP 400")
                .with_param(MESSAGE, "skins_error_rejected"),
            CallError::RejectedCape => {
                AppError::new(ErrorCode::InvalidInput, "Minecraft Services: HTTP 400 to a cape")
                    .with_param(MESSAGE, "skins_error_cape_rejected")
            }
            CallError::Other(e) => e,
        }
    }
}

fn network(e: impl std::fmt::Display) -> CallError {
    CallError::Other(AppError::new(ErrorCode::Network, format!("Minecraft Services: {e}")))
}

/// What a status other than success means.
fn failed(status: u16) -> CallError {
    match status {
        401 => CallError::Unauthorized,
        429 => CallError::Rate,
        400 => CallError::Rejected,
        _ => CallError::Other(
            AppError::new(ErrorCode::Network, format!("Minecraft Services: HTTP {status}"))
                .with_param("status", status.to_string()),
        ),
    }
}

#[derive(Deserialize)]
struct RawProfile {
    #[serde(default)]
    skins: Vec<RawTexture>,
    #[serde(default)]
    capes: Vec<RawTexture>,
}

#[derive(Deserialize)]
struct RawTexture {
    #[serde(default)]
    id: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    variant: String,
    #[serde(default)]
    alias: String,
}

impl RawProfile {
    fn into_profile(self) -> ServicesProfile {
        ServicesProfile {
            skins: self
                .skins
                .into_iter()
                .map(|s| RemoteSkin {
                    id: s.id,
                    alias: Some(s.alias).filter(|a| !a.trim().is_empty()),
                    state: s.state.to_ascii_uppercase(),
                    url: s.url,
                    variant: if s.variant.eq_ignore_ascii_case("slim") {
                        SkinVariant::Slim
                    } else {
                        SkinVariant::Classic
                    },
                })
                .collect(),
            capes: self
                .capes
                .into_iter()
                .map(|c| RemoteCape {
                    id: c.id,
                    state: c.state.to_ascii_uppercase(),
                    url: c.url,
                    alias: c.alias,
                })
                .collect(),
        }
    }
}

/// `body` as a profile, when it is one (it names `skins`).
fn as_profile(body: &[u8]) -> Option<ServicesProfile> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value.get("skins")?;
    serde_json::from_value::<RawProfile>(value).ok().map(RawProfile::into_profile)
}

pub struct MojangClient {
    client: reqwest::Client,
    ep: Endpoints,
}

impl MojangClient {
    pub fn new(ep: Endpoints) -> AppResult<Self> {
        let client = api_client(&format!("{APP_NAME}/{VERSION}"), CONNECT, READ)?;
        Ok(MojangClient { client, ep })
    }

    /// `base` with `segments` added, each escaped.
    fn url(base: &str, segments: &[&str]) -> Result<Url, CallError> {
        let mut url = Url::parse(base.trim_end_matches('/')).map_err(network)?;
        url.path_segments_mut().map_err(|()| network("the address cannot have a path"))?.extend(segments);
        Ok(url)
    }

    fn services(&self, segments: &[&str]) -> Result<Url, CallError> {
        Self::url(&self.ep.services, &[&["minecraft", "profile"], segments].concat())
    }

    /// Sends `request` with `token`: the body of a success, else what it means.
    async fn send(request: RequestBuilder, token: &str) -> Result<Vec<u8>, CallError> {
        let request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        let response = send_patiently(request).await.map_err(network)?;
        if !response.status().is_success() {
            return Err(failed(response.status().as_u16()));
        }
        Ok(response.bytes().await.map_err(network)?.to_vec())
    }

    /// `GET /minecraft/profile`: the account's skins and capes.
    pub async fn profile(&self, token: &str) -> Result<ServicesProfile, CallError> {
        let body = Self::send(self.client.get(self.services(&[])?), token).await?;
        as_profile(&body).ok_or_else(|| network("the profile is unreadable"))
    }

    /// The profile a change answered with, or asked for when the answer is none.
    async fn after_change(&self, token: &str, body: &[u8]) -> Result<ServicesProfile, CallError> {
        match as_profile(body) {
            Some(profile) => Ok(profile),
            None => self.profile(token).await,
        }
    }

    /// `POST /minecraft/profile/skins`: puts `png` on as the account's skin.
    pub async fn upload_skin(
        &self,
        token: &str,
        png: Vec<u8>,
        variant: SkinVariant,
    ) -> Result<ServicesProfile, CallError> {
        let file = reqwest::multipart::Part::bytes(png)
            .file_name("skin.png")
            .mime_str("image/png")
            .map_err(network)?;
        let form = reqwest::multipart::Form::new().text("variant", variant.as_str()).part("file", file);
        let request = self.client.post(self.services(&["skins"])?).multipart(form);
        let body = Self::send(request, token).await?;
        self.after_change(token, &body).await
    }

    /// `PUT /minecraft/profile/capes/active` for a cape, `DELETE` for none.
    pub async fn set_cape(&self, token: &str, cape: Option<&str>) -> Result<ServicesProfile, CallError> {
        let url = self.services(&["capes", "active"])?;
        let request = match cape {
            Some(id) => self
                .client
                .put(url)
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::json!({ "capeId": id }).to_string()),
            None => self.client.delete(url),
        };
        let sent = Self::send(request, token).await;
        let body = sent.map_err(|e| if e == CallError::Rejected { CallError::RejectedCape } else { e })?;
        self.after_change(token, &body).await
    }
}

/// `name` trimmed, when it can be a Minecraft nickname (`[A-Za-z0-9_]{1,16}`); else
/// `skins_error_no_player`, before anything is asked.
pub fn check_nickname(name: &str) -> AppResult<String> {
    let name = name.trim();
    if !is_nickname(name) {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("no nickname {name:?}"))
            .with_param("name", name)
            .with_param(MESSAGE, "skins_error_no_player"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::MESSAGE;

    #[test]
    fn nicknames_are_checked_before_asking() {
        assert_eq!(check_nickname("  Steve_42 ").unwrap(), "Steve_42");
        assert_eq!(check_nickname("x").unwrap(), "x");
        for bad in ["", "   ", "two words", "Стів", "a-b", &"a".repeat(17), "../x"] {
            let error = check_nickname(bad).unwrap_err();
            assert_eq!(
                error.params.get(MESSAGE).map(String::as_str),
                Some("skins_error_no_player"),
                "{bad:?}"
            );
        }
    }
}
