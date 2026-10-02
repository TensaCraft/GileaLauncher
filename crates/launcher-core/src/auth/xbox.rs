//! Microsoft access token → Xbox Live → XSTS → Minecraft session.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use launcher_shared::{AppError, ErrorCode};
use serde_json::{Value, json};

use super::AuthEndpoints;
use super::http::{AuthHttp, FlowError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftSession {
    pub id: String,
    pub name: String,
    pub access_token: String,
    pub xuid: Option<String>,
    pub expires_at: i64,
}

fn field(body: &Value, name: &str, context: &str) -> Result<String, FlowError> {
    body.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| FlowError::Failed(format!("{context} failed: no {name} in the reply")))
}

/// Friendly errors for XSTS `XErr` codes (the original only showed "HTTP 401").
pub fn xerr_error(xerr: u64) -> Option<AppError> {
    let code = match xerr {
        2_148_916_233 => ErrorCode::XboxAccountMissing,
        2_148_916_238 => ErrorCode::XboxChildAccount,
        2_148_916_227 | 2_148_916_229 | 2_148_916_234 | 2_148_916_235 | 2_148_916_236 | 2_148_916_237 => {
            ErrorCode::XboxUnavailable
        }
        _ => return None,
    };
    Some(AppError::new(code, format!("XSTS XErr {xerr}")).with_param("xerr", xerr.to_string()))
}

/// Claims of a JWT, decoded without verification (only `xuid` and `exp` are read).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub async fn minecraft_session(
    http: &AuthHttp,
    ep: &AuthEndpoints,
    ms_access_token: &str,
    now: i64,
) -> Result<MinecraftSession, FlowError> {
    let xbl = http
        .post_json(
            "xbox authenticate",
            &ep.xbox_user,
            &json!({
                "Properties": {
                    "AuthMethod": "RPS",
                    "SiteName": "user.auth.xboxlive.com",
                    "RpsTicket": format!("d={ms_access_token}")
                },
                "RelyingParty": "http://auth.xboxlive.com",
                "TokenType": "JWT"
            }),
        )
        .await?
        .ok("xbox authenticate")?;
    let xbl_token = field(&xbl, "Token", "xbox authenticate")?;
    let uhs = xbl
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| FlowError::Failed("xbox authenticate failed: no uhs in the reply".into()))?;

    let xsts = http
        .post_json(
            "xsts authorize",
            &ep.xsts,
            &json!({
                "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
                "RelyingParty": "rp://api.minecraftservices.com/",
                "TokenType": "JWT"
            }),
        )
        .await?;
    if xsts.status == 401
        && let Some(error) = xsts.body.get("XErr").and_then(Value::as_u64).and_then(xerr_error)
    {
        return Err(FlowError::App(error));
    }
    let xsts = xsts.ok("xsts authorize")?;
    let xsts_token = field(&xsts, "Token", "xsts authorize")?;

    let login = http
        .post_json(
            "minecraft authenticate",
            &ep.mc_login,
            &json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }),
        )
        .await?;
    if login.status >= 500 {
        return Err(FlowError::ServicesUnavailable(format!(
            "minecraft authenticate failed: HTTP {}",
            login.status
        )));
    }
    let login = login.ok("minecraft authenticate")?;
    let access_token = field(&login, "access_token", "minecraft authenticate")?;

    let profile = http.get_bearer("minecraft profile", &ep.mc_profile, &access_token).await?;
    if profile.status == 404 {
        return Err(FlowError::App(AppError::new(
            ErrorCode::MinecraftNotOwned,
            "the account has no Minecraft: Java Edition profile",
        )));
    }
    let profile = profile.ok("minecraft profile")?;
    let claims = jwt_claims(&access_token);
    let expires_at = login
        .get("expires_in")
        .and_then(Value::as_i64)
        .map(|seconds| now + seconds)
        .or_else(|| claims.as_ref().and_then(|c| c.get("exp")).and_then(Value::as_i64))
        .unwrap_or(now);
    let xuid = claims
        .as_ref()
        .and_then(|c| c.get("xuid"))
        .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())));
    Ok(MinecraftSession {
        id: field(&profile, "id", "minecraft profile")?,
        name: field(&profile, "name", "minecraft profile")?,
        access_token,
        xuid,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_claims_are_read_without_verification() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"xuid":"42","exp":7}"#);
        let claims = jwt_claims(&format!("h.{payload}.s")).unwrap();
        assert_eq!(claims["xuid"], serde_json::json!("42"));
        assert!(jwt_claims("not-a-jwt").is_none());
    }

    #[test]
    fn known_xsts_codes_have_messages() {
        assert_eq!(xerr_error(2_148_916_238).unwrap().code, ErrorCode::XboxChildAccount);
        assert_eq!(xerr_error(2_148_916_235).unwrap().code, ErrorCode::XboxUnavailable);
        assert!(xerr_error(1).is_none());
    }
}
