//! The backend half: Modrinth's API, its catalog rules and the install service, answered through
//! the content-provider contract.

pub mod api;
pub mod catalog;
pub mod components;
pub mod inventory;
pub mod pack;
pub mod provenance;
pub mod resolver;
pub mod service;

use std::sync::{Arc, OnceLock};

use launcher_core::core_app::CoreApp;
use launcher_core::modules::Module;
use launcher_core::providers::ContentProvider;
use launcher_shared::AppResult;
use launcher_shared::provider::ProviderInfo;

use self::api::{BASE, ModrinthApi};
use self::service::{Deps, ModrinthService};

/// The module; its service is made from the launcher's services on first use.
#[derive(Default)]
pub struct ModrinthModule {
    service: OnceLock<Arc<ModrinthService>>,
}

impl ModrinthModule {
    fn service(&self, core: &CoreApp) -> AppResult<Arc<ModrinthService>> {
        if let Some(service) = self.service.get() {
            return Ok(service.clone());
        }
        let api = ModrinthApi::new(BASE)?;
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
        };
        Ok(self.service.get_or_init(|| Arc::new(ModrinthService::new(api, deps))).clone())
    }
}

impl Module for ModrinthModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn provider_info(&self) -> Option<ProviderInfo> {
        Some(crate::types::provider_info())
    }

    fn provider(&self, core: &Arc<CoreApp>) -> AppResult<Arc<dyn ContentProvider>> {
        Ok(self.service(core)?)
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(ModrinthModule::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::ContentKind;
    use launcher_shared::provider::Need;

    #[test]
    fn modrinth_is_reached_only_as_a_provider() {
        let registry = launcher_core::modules::ModuleRegistry::new(vec![module()]).unwrap();
        assert!(
            !registry.changes_builds(crate::ID, "pack_install"),
            "the provider command announces builds itself"
        );
        assert!(registry.provider_infos().iter().any(|p| p.id == crate::ID));
    }

    #[test]
    fn modrinth_offers_content_updates_for_mods_and_modpacks() {
        let info = ModrinthModule::default().provider_info().unwrap();
        assert_eq!((info.id.as_str(), info.name.as_str()), (crate::ID, "Modrinth"));
        for kind in ContentKind::ALL {
            assert!(info.offers(Need::Content(kind)), "{kind:?}");
        }
        for kind in ContentKind::ALL {
            assert!(info.offers(Need::Updates(kind)), "{kind:?} is checked for updates");
        }
        assert!(info.offers(Need::Modpacks));
    }
}
