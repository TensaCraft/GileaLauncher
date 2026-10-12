//! The sync module's backend: groups of builds whose chosen files are kept the same.

pub mod blacklist;
pub mod paths;
pub mod scan;
pub mod store;

use launcher_core::modules::Module;

pub struct SyncModule;

impl Module for SyncModule {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new(SyncModule)
}
