//! `cargo xtask mock-releases …` — local imitation of GitHub Releases for the launcher updater.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use mock_github::{NewRelease, Scenario};

use crate::cmd::{self, root};
use crate::profile;

const MOCK_PROFILE: &str = "mock-updates";
const DEMO_NOTES: &str = "• Самооновлення лаунчера з перевіркою SHA-256\n• Докачування після обриву мережі\n• Перезапуск уже новою версією";

pub fn releases_dir() -> PathBuf {
    root().join(".dev").join("mock-releases")
}

pub fn install_dir() -> PathBuf {
    root().join(".dev").join("mock-install")
}

/// The name the launcher's asset selection prefers on this OS for edition `edition` of an app
/// named `app`.
fn host_asset_name(app: &str, edition: &str) -> String {
    if cfg!(windows) {
        format!("{app}-{edition}.exe")
    } else if cfg!(target_os = "macos") {
        format!("{app}-{edition}-universal.dmg")
    } else {
        format!("{app}-{edition}-{}", std::env::consts::ARCH)
    }
}

fn validate_version(version: &str) -> Result<()> {
    let ok = version.chars().next().is_some_and(|c| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !ok {
        bail!("'{version}' is not a version like 0.2.0 or 1.0.0-beta.1");
    }
    Ok(())
}

fn scenario(name: &str) -> Result<Scenario> {
    name.parse().map_err(anyhow::Error::msg)
}

pub fn serve(scenario_name: &str, port: u16) -> Result<()> {
    let scenario = scenario(scenario_name)?;
    std::fs::create_dir_all(releases_dir())?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let server = mock_github::start_logged(releases_dir(), scenario, port).await?;
        println!(
            "Mock GitHub API: {} (repo {}, scenario {scenario_name}). Data: {}. Ctrl+C to stop.",
            server.base_url,
            mock_github::REPO,
            releases_dir().display()
        );
        tokio::signal::ctrl_c().await?;
        server.shutdown().await;
        anyhow::Ok(())
    })
}

pub fn publish(version: &str, beta: bool, notes: &str, file: Option<&Path>) -> Result<()> {
    validate_version(version)?;
    let spec = profile::load_profile(&root(), MOCK_PROFILE)?;
    let source = match file {
        Some(path) => path.to_path_buf(),
        None => {
            if cfg!(target_os = "macos") {
                bail!("on macOS pass --file <Launcher.dmg>");
            }
            println!("Building {} {version} ({MOCK_PROFILE} profile)…", spec.app_name());
            cmd::build_versioned_app(&spec, version)?
        }
    };
    let new = NewRelease {
        version,
        prerelease: beta,
        notes,
        asset_name: &host_asset_name(spec.app_name(), spec.edition()),
        source: &source,
    };
    let release = mock_github::add_release(&releases_dir(), new)
        .with_context(|| format!("cannot add the release to {}", releases_dir().display()))?;
    let asset = &release.assets[0];
    println!("Published {} — {} ({} bytes, {})", release.tag_name, asset.name, asset.size, asset.digest);
    Ok(())
}

pub fn reset() -> Result<()> {
    for dir in [releases_dir(), install_dir()] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("cannot remove {} (is the demo still running?)", dir.display()))?;
        }
    }
    println!("Removed mock releases and the demo installation");
    Ok(())
}

/// Builds 0.1.0 into `.dev/mock-install`, publishes 0.2.0, serves it and starts 0.1.0 with its data
/// in `.dev/mock-install/data` (portable mode: the real %LOCALAPPDATA% is untouched).
pub fn demo(scenario_name: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        bail!("the demo installs a plain executable; on macOS use `serve` and `publish --file`");
    }
    let scenario = scenario(scenario_name)?;
    reset()?;
    let spec = profile::load_profile(&root(), MOCK_PROFILE)?;
    println!("Building {} 0.1.0 ({MOCK_PROFILE} profile)…", spec.app_name());
    let v1 = cmd::build_versioned_app(&spec, "0.1.0")?;
    let install = install_dir();
    std::fs::create_dir_all(&install)?;
    let exe = install.join(host_asset_name(spec.app_name(), spec.edition()));
    std::fs::copy(&v1, &exe).with_context(|| format!("cannot copy to {}", exe.display()))?;
    publish("0.2.0", false, DEMO_NOTES, None)?;
    let data = install.join("data");
    tokio::runtime::Runtime::new()?.block_on(async {
        let server = mock_github::start_logged(releases_dir(), scenario, mock_github::DEFAULT_PORT).await?;
        Command::new(&exe)
            .env("LAUNCHER_APP_BASE", &data)
            .spawn()
            .with_context(|| format!("cannot start {}", exe.display()))?;
        println!("Started {} (0.1.0). In the launcher:", exe.display());
        println!("  Settings → Launcher → «Перевірити оновлення» → «Оновити зараз» → «Перезапустити зараз».");
        println!(
            "The launcher restarts as 0.2.0 and shows «Лаунчер оновлено до 0.2.0». Ctrl+C stops the server."
        );
        tokio::signal::ctrl_c().await?;
        server.shutdown().await;
        anyhow::Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_must_look_like_versions() {
        for good in ["0.2.0", "1.0.0-beta.1", "10.2"] {
            assert!(validate_version(good).is_ok(), "{good}");
        }
        for bad in ["", "v0.2.0", "latest", "0.2 0", "../0.2"] {
            assert!(validate_version(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn demo_folders_live_in_dot_dev() {
        assert!(releases_dir().ends_with(Path::new(".dev").join("mock-releases")));
        assert!(install_dir().ends_with(Path::new(".dev").join("mock-install")));
    }
}
