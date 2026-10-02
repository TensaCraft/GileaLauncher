//! Backend modules compiled into this build (cargo features `mod-*`).

use launcher_core::modules::Module;

// `vec![]` cannot hold `#[cfg]`-gated elements, hence push-after-new.
#[allow(clippy::vec_init_then_push)]
pub fn backend_modules() -> Vec<Box<dyn Module>> {
    #[allow(unused_mut)]
    let mut modules: Vec<Box<dyn Module>> = Vec::new();
    #[cfg(feature = "mod-tensa")]
    modules.push(module_tensa::backend::module());
    #[cfg(feature = "mod-modrinth")]
    modules.push(module_modrinth::backend::module());
    #[cfg(feature = "mod-curseforge")]
    modules.push(module_curseforge::backend::module());
    #[cfg(feature = "mod-backups")]
    modules.push(module_backups::backend::module());
    #[cfg(feature = "mod-reports")]
    modules.push(module_reports::backend::module());
    #[cfg(feature = "mod-diagnostics")]
    modules.push(module_diagnostics::backend::module());
    modules
}
