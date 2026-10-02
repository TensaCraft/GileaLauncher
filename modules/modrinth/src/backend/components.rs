//! How the module gets a Minecraft version or a loader installed: the launcher's
//! `ComponentInstaller` in the app, a fake in the tests (the core's `ComponentSource`).

pub use launcher_core::loaders::{ComponentFuture, ComponentSource};
