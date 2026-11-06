//! A new Horizon build: its record only (version 26.3, its engine); its first start brings the
//! game. A failed write leaves no folder behind.

use std::fs;
use std::path::Path;

use launcher_core::core_app::CoreApp;
use launcher_core::launch::engine::ENGINE_OPTION;
use launcher_core::storage::versions::Build;
use launcher_shared::AppResult;
use serde_json::json;

use crate::CLIENT;
use crate::dto::CreateArgs;

pub fn create(core: &CoreApp, args: &CreateArgs) -> AppResult<Build> {
    let id = core.versions.claim_folder(args.name.trim())?;
    let written = (|| {
        let mut build = Build::new(args.name.trim());
        build.client = Some(CLIENT.into());
        build.version = Some(crate::MINECRAFT.into());
        build.description = args.description.trim().to_string();
        build.options.insert(ENGINE_OPTION.into(), json!(crate::ENGINE));
        if let Some(path) = args.image_path.as_deref().filter(|p| !p.trim().is_empty()) {
            build.image = Some(launcher_core::builds::settings::read_icon(Path::new(path))?);
        }
        core.versions.create_in(&mut build, &id)?;
        Ok(build)
    })();
    if written.is_err() {
        let _ = fs::remove_dir_all(core.versions.games_dir().join(&id));
    }
    written
}
