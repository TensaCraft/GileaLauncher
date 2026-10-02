//! The launcher's Tauri shell.

pub mod bridge;
pub mod cli;
pub mod commands;
pub mod component_commands;
pub mod content_commands;
pub mod game_commands;
pub mod modules;
pub mod provider_commands;
pub mod screenshot_protocol;
pub mod smoke;
pub mod tray;
pub mod window;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use launcher_core::core_app::{BootstrapOptions, CoreApp};
use launcher_core::paths::{LauncherPaths, PathEnv};
use launcher_core::settings::detect_system_lang;
use launcher_core::updater::apply::{self, HelperArgs};
use launcher_core::updater::stage::{self, ExecContext, ResumeOutcome};
use launcher_shared::{ExternalLaunch, names};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;

const AUTO_UPDATE_CHECK_DELAY: Duration = Duration::from_secs(2);

/// The window back in front — out of the tray too, when it was hidden there.
fn focus_main_window(app: &AppHandle) {
    tray::restore(app);
}

/// A deferred update is installed by the primary instance only (this runs after single-instance).
fn resume_pending_update(env: &PathEnv) -> ResumeOutcome {
    if env.dev_root.is_some() {
        return ResumeOutcome::Nothing;
    }
    let Ok(exec) = ExecContext::current() else { return ResumeOutcome::Nothing };
    let cache = LauncherPaths::resolve(env, None).cache_dir;
    stage::resume_pending(&cache, &exec)
}

/// Startup: refresh every Microsoft profile in the background.
fn spawn_profile_refresh(core: Arc<CoreApp>) {
    tauri::async_runtime::spawn(async move {
        core.auth.refresh_all().await;
    });
}

fn spawn_auto_update_check(core: Arc<CoreApp>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(AUTO_UPDATE_CHECK_DELAY).await;
        let settings = core.settings.snapshot();
        if settings.auto_update && core.updater.status().configured {
            core.updater.check(settings.include_beta_updates, false).await;
        }
    });
}

/// Refreshes the list of Java found on this computer when it is empty, old or from an old version.
fn spawn_java_rescan(core: Arc<CoreApp>) {
    tauri::async_runtime::spawn_blocking(move || {
        let now = launcher_core::java::preferences::now_secs();
        if core.java_settings.needs_rescan(now)
            && let Err(e) = core.java_settings.rescan(now)
        {
            tracing::warn!("Java discovery failed: {}", e.detail);
        }
    });
}

pub fn run() -> i32 {
    let args = cli::parse_args(std::env::args());
    if let Some(marker) = args.apply_update {
        let helper =
            HelperArgs { marker, wait_pid: args.wait_pid.unwrap_or(0), wait_timeout: apply::WAIT_TIMEOUT };
        return apply::run_helper(&helper);
    }
    if args.smoke_test {
        return smoke::run();
    }
    let initial_launch = args.launch_version;

    let built = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let parsed = cli::parse_args(argv);
            focus_main_window(app);
            tracing::info!("Second instance forwarded launch request: {:?}", parsed.launch_version);
            let _ = app.emit(names::EXTERNAL_LAUNCH, ExternalLaunch { version_id: parsed.launch_version });
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(screenshot_protocol::SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(screenshot_protocol::respond(&app, &request));
            });
        })
        .setup(move |app| {
            let env = PathEnv::from_system();
            let resumed = resume_pending_update(&env);
            if matches!(resumed, ResumeOutcome::Launched | ResumeOutcome::InProgress) {
                app.handle().exit(0);
                return Ok(());
            }
            let sink = Arc::new(bridge::TauriSink::new(app.handle().clone()));
            let system_lang = detect_system_lang(sys_locale::get_locale().as_deref());
            let core = Arc::new(CoreApp::bootstrap(
                env,
                sink,
                modules::backend_modules(),
                BootstrapOptions {
                    init_logging: true,
                    system_lang,
                    opener: Arc::new(bridge::TauriOpener::new(app.handle().clone())),
                },
            )?);
            if let ResumeOutcome::Discarded(reason) = &resumed {
                tracing::warn!("Discarded a pending launcher update: {reason}");
            }
            app.manage(AppState {
                core: core.clone(),
                pending_launch: Mutex::new(initial_launch.clone()),
                clicks: launcher_core::platform::sound::ClickPlayer::new(),
            });
            spawn_profile_refresh(core.clone());
            spawn_java_rescan(core.clone());
            if let Some(window) = app.get_webview_window("main") {
                let size = launcher_shared::WindowSize::parse(&core.settings.snapshot().window_size)
                    .unwrap_or_default();
                window::apply_window_size(&window, size);
                tray::retitle(app.handle());
                let _ = window.show();
            }
            spawn_auto_update_check(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::play_click,
            commands::startup_warnings,
            commands::take_pending_launch,
            commands::settings_get,
            commands::settings_set,
            commands::settings_save_minecraft_dir,
            commands::setup_state,
            commands::setup_apply,
            commands::setup_preview,
            commands::ops_snapshot,
            commands::activity_recent,
            commands::pick_directory,
            commands::pick_java_file,
            commands::pick_image_file,
            commands::open_path,
            commands::log_view,
            commands::reveal_path,
            commands::open_url,
            commands::module_invoke,
            provider_commands::provider_search,
            provider_commands::provider_plan,
            provider_commands::provider_install,
            provider_commands::provider_overview,
            provider_commands::provider_modpacks,
            provider_commands::provider_modpack_versions,
            provider_commands::provider_install_modpack,
            provider_commands::provider_modpack_builds,
            provider_commands::provider_update_modpack,
            provider_commands::held_files_found,
            commands::app_restart,
            commands::window_control,
            commands::update_status,
            commands::update_check,
            commands::update_download,
            commands::update_apply,
            commands::profiles_list,
            commands::profile_create_offline,
            commands::profile_delete,
            commands::profile_set_default,
            commands::auth_state,
            commands::auth_sign_in,
            commands::auth_cancel,
            commands::profile_avatar,
            game_commands::builds_list,
            game_commands::builds_reorder,
            game_commands::build_create_vanilla,
            game_commands::build_copy,
            game_commands::build_delete,
            game_commands::build_launch,
            game_commands::build_stop,
            game_commands::build_open_dir,
            game_commands::build_shortcut,
            game_commands::catalog_minecraft,
            game_commands::catalog_loader,
            game_commands::build_create_loader,
            game_commands::build_settings_get,
            game_commands::build_settings_save,
            game_commands::build_change_component,
            component_commands::components_list,
            component_commands::component_install,
            component_commands::component_verify,
            component_commands::component_reinstall,
            component_commands::component_delete,
            component_commands::component_open_dir,
            content_commands::content_list,
            content_commands::content_toggle,
            content_commands::content_delete,
            content_commands::content_restore,
            content_commands::content_open_dir,
            content_commands::screenshots_list,
            content_commands::screenshot_delete,
            content_commands::screenshot_open,
            content_commands::screenshots_open_dir,
            game_commands::java_list,
            game_commands::java_add,
            game_commands::java_remove,
            game_commands::java_scan,
            game_commands::memory_info,
        ])
        .build(tauri::generate_context!());

    match built {
        Ok(app) => {
            app.run(|_app, _event| {
                // macOS: the Dock icon brings a launcher hidden in the tray back.
                #[cfg(target_os = "macos")]
                if let tauri::RunEvent::Reopen { .. } = _event {
                    tray::restore(_app);
                }
            });
            0
        }
        Err(e) => {
            tracing::error!("Fatal Tauri error: {e}");
            eprintln!("The launcher failed to start: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn webview_gets_no_plugin_permissions() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(caps["permissions"], serde_json::json!(["core:default"]));
    }

    #[test]
    fn csp_lets_the_frontend_fetch_its_own_wasm() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        let connect = csp.split(';').map(str::trim).find(|d| d.starts_with("connect-src")).unwrap();
        assert!(connect.split_whitespace().any(|s| s == "'self'"), "{connect}");
    }

    #[test]
    fn the_launcher_draws_its_own_title_bar() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["app"]["windows"][0]["decorations"], serde_json::json!(false));
    }

    #[test]
    fn main_window_starts_hidden_until_the_core_is_ready() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["app"]["windows"][0]["visible"], serde_json::json!(false));
    }
}
