//! Fabric and Quilt meta servers: the Minecraft versions each loader supports,
//! its builds, and the version JSON ("profile") of a build, which is installed directly — no Java
//! installer runs.

use std::sync::Arc;

use launcher_shared::{AppError, AppResult, ErrorCode, LoaderBuild, LoaderKind};
use serde_json::{Map, Value};

use super::versions::is_prerelease;
use crate::net::meta::MetaClient;

/// A Minecraft or loader version fit for a URL path and a folder name.
pub fn version_token_ok(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && !token.starts_with('.')
}

fn unexpected(what: &str, url: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("{url}: {what}")).with_param("error", what)
}

pub struct FabricMeta {
    kind: LoaderKind,
    base: String,
    meta: Arc<MetaClient>,
}

impl FabricMeta {
    pub fn new(kind: LoaderKind, base: &str, meta: Arc<MetaClient>) -> FabricMeta {
        FabricMeta { kind, base: base.trim_end_matches('/').to_string(), meta }
    }

    async fn list(&self, what: &str) -> AppResult<Vec<Value>> {
        let url = format!("{}/versions/{what}", self.base);
        match self.meta.get_json(&url).await? {
            Value::Array(items) => Ok(items),
            _ => Err(unexpected("not a list", &url)),
        }
    }

    /// Minecraft versions the loader supports, with the loader's `stable` flag.
    pub async fn games(&self) -> AppResult<Vec<(String, bool)>> {
        Ok(self
            .list("game")
            .await?
            .iter()
            .filter_map(|g| {
                let version = g.get("version")?.as_str()?.to_string();
                Some((version, g.get("stable").and_then(Value::as_bool).unwrap_or(true)))
            })
            .collect())
    }

    /// Loader builds, newest first as listed. A build is unstable when its version is a
    /// prerelease: Fabric marks only its newest build `stable` (a recommendation, not a release
    /// channel) and Quilt marks none.
    pub async fn loaders(&self) -> AppResult<Vec<LoaderBuild>> {
        Ok(self
            .list("loader")
            .await?
            .iter()
            .filter_map(|l| {
                let version = l.get("version")?.as_str()?.to_string();
                let stable = !is_prerelease(&version);
                Some(LoaderBuild { version, stable })
            })
            .collect())
    }

    /// The version JSON of loader build `lv` for Minecraft `mc`; its `id` must be the one the
    /// build installs as.
    pub async fn profile(&self, mc: &str, lv: &str) -> AppResult<Map<String, Value>> {
        if !version_token_ok(mc) || !version_token_ok(lv) {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("unusable versions {mc:?} / {lv:?}"))
                .with_param("version", mc));
        }
        let url = format!("{}/versions/loader/{mc}/{lv}/profile/json", self.base);
        let Value::Object(profile) = self.meta.get_json(&url).await? else {
            return Err(unexpected("not an object", &url));
        };
        let expected = self.kind.component_id(mc, lv);
        if profile.get("id").and_then(Value::as_str) != Some(expected.as_str()) {
            return Err(unexpected("the profile is for another version", &url));
        }
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tokens_are_plain() {
        for ok in ["1.21.1", "0.16.9", "0.27.0-beta.1", "24w33a", "1.20.1+build.5"] {
            assert!(version_token_ok(ok), "{ok}");
        }
        for bad in ["", "../x", "1.21 1", "a/b", "1.21.1?x=1", &"9".repeat(65)] {
            assert!(!version_token_ok(bad), "{bad}");
        }
    }
}
