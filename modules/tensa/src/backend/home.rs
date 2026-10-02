//! What Home shows of the server builds: while the switch is on, the server
//! builds not installed yet as cards to install. Installed ones are builds like any other and
//! stay on Home whatever the switch says.

use std::collections::HashSet;

use launcher_core::feedback::{ReportContext, ReportKind};
use launcher_core::launch::options::game_dir;
use launcher_core::storage::config::ConfigStore;
use launcher_core::storage::versions::Build;
use launcher_shared::{AppError, AppResult, ErrorCode, Level, LoaderKind, Text};
use serde_json::{Value, json};

use super::identity;
use super::install::install;
use super::pack::{Pack, find};
use super::service::Deps;

/// The switch "Show server builds on Home".
pub const SHOW_KEY: &str = "show_server_builds";

pub fn shown(config: &ConfigStore) -> bool {
    config.get_bool(SHOW_KEY, true)
}

pub fn set_shown(config: &ConfigStore, show: bool) -> std::io::Result<()> {
    config.set(SHOW_KEY, json!(if show { "yes" } else { "no" }))
}

/// "Fabric 26.3".
fn runs(pack: &Pack) -> String {
    let loader = pack.loader.as_deref().unwrap_or_default();
    let loader =
        LoaderKind::from_client(loader).map_or_else(|| loader.to_string(), |k| k.display_name().to_string());
    [loader, pack.minecraft.clone().unwrap_or_default()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The catalog's server builds that no installed build follows, as Home's cards: none while the
/// switch is off (the server is not asked); the server's failure, for Home to show.
pub async fn home_packs(deps: &Deps, config: &ConfigStore) -> AppResult<Vec<Value>> {
    if !shown(config) {
        return Ok(Vec::new());
    }
    let packs =
        deps.api.packs().await.inspect_err(|e| tracing::warn!("No server builds for Home: {}", e.detail))?;
    let mc = deps.versions.minecraft_dir();
    let installed: HashSet<String> = deps
        .versions
        .list()
        .iter()
        .filter(|build| identity::is_managed(build, &game_dir(build, mc)))
        .filter_map(identity::pack_id)
        .map(|id| id.to_lowercase())
        .collect();
    Ok(packs
        .iter()
        .filter_map(Pack::from_value)
        .filter(|pack| !installed.contains(&pack.id.to_lowercase()))
        .map(|pack| {
            json!({
                "id": pack.id, "name": pack.name, "description": pack.description, "image": pack.image,
                "runs": runs(&pack)
            })
        })
        .collect())
}

/// `name`, else "name 2", "name 3"… — the first no build has (case aside).
fn free_name(deps: &Deps, name: &str) -> String {
    let taken =
        |candidate: &str| deps.versions.list().iter().any(|b| b.name.trim().eq_ignore_ascii_case(candidate));
    if !taken(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} {n}")).find(|candidate| !taken(candidate)).expect("a free name exists")
}

/// Installs server build `pack_id` from its Home card, under its own name or a free one like it.
/// A failure (but "busy") is told by an alert the user can report.
pub async fn install_from_home(deps: &Deps, pack_id: &str) -> AppResult<Build> {
    let mut version = pack_id.to_string();
    let installed = async {
        let packs = deps.api.packs().await?;
        let pack = find(&packs, pack_id).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound, format!("no server build {pack_id}"))
                .with_param("pack", pack_id)
        })?;
        version = pack.name.trim().to_string();
        let name = free_name(deps, &version);
        install(deps, &pack.id, &name).await
    }
    .await;
    if let Err(e) = &installed
        && e.code != ErrorCode::Busy
    {
        report_failure(deps, pack_id, &version, e);
    }
    installed
}

fn report_failure(deps: &Deps, pack_id: &str, version: &str, e: &AppError) {
    let message = Text::key("version_install_error")
        .param("client", identity::CLIENT)
        .param("version", version)
        .param("error", e.detail.as_str());
    let report = ReportContext {
        kind: ReportKind::Error,
        title: "Server build install failed".into(),
        screen: "Home".into(),
        action: "tensacraft_install".into(),
        metadata: json!({"pack_id": pack_id, "version_name": version, "exception": format!("{:?}: {}", e.code, e.detail)}),
        attachments: Vec::new(),
    };
    deps.feedback.alert_with_report(Level::Error, Text::key("warning"), message, report);
}
