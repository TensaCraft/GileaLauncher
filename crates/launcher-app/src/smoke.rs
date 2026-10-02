//! `--smoke-test`: verifies that the packaged binary can resolve and prepare its directories.

use std::sync::Arc;

use launcher_core::core_app::{BootstrapOptions, CoreApp};
use launcher_core::feedback::NullSink;
use launcher_core::paths::PathEnv;

pub fn run() -> i32 {
    let opts = BootstrapOptions {
        init_logging: false,
        system_lang: "en_US".into(),
        opener: Arc::new(launcher_core::auth::NoOpener),
    };
    match CoreApp::bootstrap(
        PathEnv::from_system(),
        Arc::new(NullSink),
        crate::modules::backend_modules(),
        opts,
    ) {
        Ok(core) if core.paths.app_state_dir.is_dir() && core.paths.games_dir.is_dir() => {
            println!("smoke-ok");
            0
        }
        Ok(_) => {
            eprintln!("smoke-failed: directories were not created");
            1
        }
        Err(e) => {
            eprintln!("smoke-failed: {e}");
            1
        }
    }
}
