mod app;
mod builds;
mod mock;
#[cfg(feature = "mod-backups")]
mod mock_backups;
mod mock_builds;
mod mock_components;
mod mock_content;
#[cfg(feature = "mod-diagnostics")]
mod mock_diagnostics;
#[cfg(feature = "mod-modrinth")]
mod mock_modrinth;
#[cfg(feature = "mod-reports")]
mod mock_reports;
#[cfg(feature = "mod-tensa")]
mod mock_tensa;
mod modules;
mod pages;
mod profiles;
mod providers;
mod recent;
mod shell;
mod store;
mod update;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
