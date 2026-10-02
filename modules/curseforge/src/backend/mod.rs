//! The backend half: CurseForge's API, its catalog rules and the install service, answered
//! through the content-provider contract — only when the launcher has a CurseForge key.

pub mod api;
pub mod catalog;
pub mod fingerprint;
pub mod held;
pub mod key;
pub mod pack;
pub mod provenance;
pub mod resolver;
pub mod service;

use std::sync::{Arc, OnceLock};

use launcher_core::core_app::CoreApp;
use launcher_core::modules::Module;
use launcher_core::providers::ContentProvider;
use launcher_shared::provider::ProviderInfo;
use launcher_shared::{AppError, AppResult, ErrorCode};

use self::api::{BASE, CurseForgeApi};
use self::key::{ApiKey, resolve_key};
use self::service::{CurseForgeService, Deps};

/// The module; its service is made from the launcher's services on first use.
pub struct CurseForgeModule {
    key: Option<ApiKey>,
    service: OnceLock<Arc<CurseForgeService>>,
}

impl CurseForgeModule {
    /// The module with `key`; without one it offers nothing.
    pub fn with_key(key: Option<ApiKey>) -> CurseForgeModule {
        CurseForgeModule { key, service: OnceLock::new() }
    }

    fn service(&self, core: &CoreApp) -> AppResult<Arc<CurseForgeService>> {
        if let Some(service) = self.service.get() {
            return Ok(service.clone());
        }
        let key = self.key.clone().ok_or_else(|| AppError::new(ErrorCode::NotFound, "no CurseForge key"))?;
        let api = CurseForgeApi::new(BASE, key.clone())?;
        let deps = Deps {
            versions: core.versions.clone(),
            instances: core.instances.clone(),
            feedback: core.feedback.clone(),
            downloader: core.downloader.clone(),
            running: {
                let launcher = core.launcher.clone();
                Arc::new(move |key: &str| launcher.is_running(key))
            },
            components: core.components.clone(),
            gpu_mode: {
                let config = core.config.clone();
                Arc::new(move || launcher_core::builds::service::default_gpu_mode(&config).to_string())
            },
            elsewhere: held::Elsewhere {
                modrinth: Some(held::MODRINTH.to_string()),
                downloads: launcher_core::content::held::downloads_dir(),
            },
        };
        Ok(self.service.get_or_init(|| Arc::new(CurseForgeService::new(api, deps, &key))).clone())
    }
}

impl Module for CurseForgeModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn provider_info(&self) -> Option<ProviderInfo> {
        self.key.is_some().then(crate::types::provider_info)
    }

    fn provider(&self, core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
        Ok(self.service(core)?)
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(CurseForgeModule::with_key(resolve_key()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::ContentKind;
    use launcher_shared::provider::Need;

    #[test]
    fn without_a_key_curseforge_offers_nothing() {
        assert_eq!(CurseForgeModule::with_key(None).provider_info(), None);
        let registry =
            launcher_core::modules::ModuleRegistry::new(vec![Box::new(CurseForgeModule::with_key(None))])
                .unwrap();
        assert!(registry.provider_infos().is_empty(), "the app shows no CurseForge");
    }

    #[test]
    fn with_a_key_curseforge_offers_its_content_modpacks_and_their_updates() {
        let info = CurseForgeModule::with_key(ApiKey::new("test-key")).provider_info().unwrap();
        assert_eq!((info.id.as_str(), info.name.as_str()), (crate::ID, "CurseForge"));
        for kind in ContentKind::ALL {
            assert!(info.offers(Need::Content(kind)), "{kind:?}");
            assert!(info.offers(Need::Updates(kind)), "{kind:?} is checked for updates");
        }
        assert!(info.offers(Need::Modpacks) && info.offers(Need::ModpackUpdates));
    }
}
