//! A Horizon build's start: only for a Microsoft account that owns Java Edition (Horizon's rule:
//! it reads the player's own Minecraft files and never downloads Mojang's). First Horizon's build
//! and what its programs take, then the player's Minecraft 26.3 and its server jar, then the
//! resources prepared from them; then `horizon-client` with the options its build offers and the
//! session on its input.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock, Weak};

use launcher_core::core_app::CoreApp;
use launcher_core::feedback::{OperationHandle, OperationSpec};
use launcher_core::launch::engine::{EngineContent, EngineContext, EngineFuture, GameEngine};
use launcher_core::launch::options::{FULLSCREEN_OPTION, resolution};
use launcher_core::launch::process::{GameCommand, prepare_launch_log};
use launcher_core::minecraft::InstallProgress;
use launcher_core::net::downloader::{DownloadTask, ExpectedHash};
use launcher_core::settings::GAME_FULLSCREEN_KEY;
use launcher_shared::{AccountKind, AppError, AppResult, ErrorCode, Text};
use serde_json::Value;

use super::args::{self, Progress, Start};
use super::install::{self, Installed};
use crate::MINECRAFT;

/// Where Horizon keeps the content it switches itself (`docs/api/content.md` in its repository).
pub const CONTENT: EngineContent = EngineContent {
    mods: "horizon-mods",
    mods_list: "horizon-mods.txt",
    mod_manifest: "horizon-mod.toml",
    graphics: "horizon-graphics.txt",
    shader_pick: "pickedShaderPack",
};

pub struct HorizonEngine {
    /// The launcher's core, once it has started (weak: the core keeps this engine).
    pub core: Arc<OnceLock<Weak<CoreApp>>>,
}

impl GameEngine for HorizonEngine {
    fn id(&self) -> &'static str {
        crate::ENGINE
    }

    fn content(&self) -> Option<EngineContent> {
        Some(CONTENT)
    }

    fn command<'a>(&'a self, ctx: &'a EngineContext<'a>) -> EngineFuture<'a> {
        Box::pin(async move {
            let core = self
                .core
                .get()
                .and_then(Weak::upgrade)
                .ok_or_else(|| AppError::internal("Horizon is not attached"))?;
            if ctx.identity.kind != AccountKind::Microsoft {
                return Err(AppError::new(
                    ErrorCode::MicrosoftRequired,
                    "Horizon plays with a Microsoft account only",
                )
                .with_param("version", &ctx.build.name));
            }
            let op = core.feedback.begin(
                OperationSpec::new(Text::key("horizon_preparing"), "install")
                    .status(Text::key("horizon_preparing"))
                    .on_build(&ctx.build.key),
            );
            match ready(&core, ctx, &op).await {
                Ok(command) => {
                    op.finish();
                    Ok(command)
                }
                Err(e) => {
                    op.fail(
                        Text::key("horizon_prepare_failed")
                            .param("version", &ctx.build.name)
                            .param("error", e.detail.clone()),
                    );
                    Err(e)
                }
            }
        })
    }
}

/// Everything the build's game needs, then its command, in the order of Horizon's guide for other
/// launchers: its build, what both its programs take, the player's files, the resources.
async fn ready(core: &CoreApp, ctx: &EngineContext<'_>, op: &OperationHandle) -> AppResult<GameCommand> {
    let minecraft = &core.paths.minecraft_dir;
    let report = op.progress_reporter();
    op.status(Text::key("horizon_step_download"));
    let horizon = install::ensure(core, &report).await?;
    let offered = launch_api(&horizon.client).await?;
    // A build older than the preparer's `--launch-api` answers nothing: it takes every option.
    let prepares = launch_api(&horizon.prepare).await.ok();
    op.status(Text::key("horizon_step_minecraft"));
    let progress = |p: InstallProgress| {
        if let Some(d) = p.download {
            report(d.bytes_done as f64, d.bytes_total.max(1) as f64);
        }
    };
    let vanilla = core.components.ensure(MINECRAFT, None, false, &progress).await?;
    let server_jar = server_jar(core, minecraft).await?;
    let resources = install::root(minecraft).join("prepared");
    if !prepared(&horizon, &resources).await {
        op.status(Text::key("horizon_step_prepare"));
        // The Java home: `<home>/bin/java`.
        let java_home =
            vanilla.java.as_deref().and_then(Path::parent).and_then(Path::parent).map(Path::to_path_buf);
        let args = args::prepare_args(
            &resources,
            minecraft,
            &server_jar,
            java_home.as_deref(),
            prepares.as_deref(),
        )?;
        prepare(horizon.prepare.clone(), args, op.progress_reporter()).await?;
    }
    let start = Start {
        resources: &resources,
        game_dir: ctx.game_dir,
        size: resolution(&ctx.build.options),
        // The build's own choice, else the launcher's, as for any game.
        fullscreen: ctx
            .build
            .options
            .get(FULLSCREEN_OPTION)
            .and_then(Value::as_bool)
            .unwrap_or_else(|| core.config.get_bool(GAME_FULLSCREEN_KEY, false)),
        join: ctx.join,
    };
    let id = ctx.identity;
    Ok(GameCommand {
        program: horizon.client.clone(),
        args: args::client_args(&offered, &start)?,
        cwd: ctx.game_dir.to_path_buf(),
        env: Vec::new(),
        log: prepare_launch_log(ctx.game_dir, crate::ENGINE, MINECRAFT).ok(),
        stdin: Some(args::session(&id.username, &id.uuid, &id.access_token, id.xuid.as_deref())),
    })
}

/// Minecraft's server jar of version 26.3, which Horizon's preparing reads (no launcher keeps it):
/// downloaded for an owner only, checked by the version's own SHA-1.
async fn server_jar(core: &CoreApp, minecraft: &Path) -> AppResult<PathBuf> {
    let json = minecraft.join("versions").join(MINECRAFT).join(format!("{MINECRAFT}.json"));
    let version: Value = serde_json::from_slice(
        &fs::read(&json)
            .map_err(|e| AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", json.display())))?,
    )
    .map_err(|e| AppError::new(ErrorCode::InvalidInput, format!("{}: {e}", json.display())))?;
    let server = &version["downloads"]["server"];
    let (Some(url), Some(sha1)) = (server["url"].as_str(), server["sha1"].as_str()) else {
        return Err(AppError::new(
            ErrorCode::NoFileFound,
            format!("Minecraft {MINECRAFT} names no server jar"),
        ));
    };
    let jar = install::root(minecraft).join(format!("server-{MINECRAFT}.jar"));
    let mut task = DownloadTask::new(url, &jar).hash(ExpectedHash::sha1(sha1));
    if let Some(size) = server["size"].as_u64() {
        task = task.size(size);
    }
    core.downloader.download_all(vec![task], true, &|_| {}).await?.into_result()?;
    Ok(jar)
}

/// A command of Horizon's that shows no console window.
fn quiet(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW.
        command.creation_flags(0x0800_0000);
    }
    command
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| AppError::internal(e.to_string()))?
}

/// Whether the resources in `output` are complete and current for `horizon`'s preparer.
async fn prepared(horizon: &Installed, output: &Path) -> bool {
    let (prepare, output) = (horizon.prepare.clone(), output.to_path_buf());
    blocking(move || {
        let status = quiet(&prepare)
            .arg("--check")
            .arg("--output")
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(status.is_ok_and(|s| s.success()))
    })
    .await
    .unwrap_or(false)
}

/// Runs `horizon-prepare` with `args`, its progress lines told to `report`; its error line (or its
/// last words) when it fails.
async fn prepare(
    program: PathBuf,
    args: Vec<String>,
    report: impl Fn(f64, f64) + Send + 'static,
) -> AppResult<()> {
    blocking(move || {
        let failed = |why: String| AppError::new(ErrorCode::LaunchFailed, why);
        let mut child = quiet(&program)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| failed(format!("{}: {e}", program.display())))?;
        let mut stderr = child.stderr.take();
        let said = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(err) = stderr.as_mut() {
                let _ = err.read_to_string(&mut text);
            }
            text
        });
        let mut error = None;
        for line in child
            .stdout
            .take()
            .map(|out| BufReader::new(out).lines())
            .into_iter()
            .flatten()
            .map_while(Result::ok)
        {
            match args::progress(&line) {
                Progress::Fraction(f) => report(f, 1.0),
                Progress::Error(why) => error = Some(why),
                Progress::Message(message) => tracing::info!("Horizon: {message}"),
                Progress::Other => {}
            }
        }
        let status = child.wait().map_err(|e| failed(e.to_string()))?;
        let said = said.join().unwrap_or_default();
        if status.success() {
            return Ok(());
        }
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
        tracing::warn!("horizon-prepare ended with {status}: {said}");
        Err(failed(error.unwrap_or(if last.is_empty() {
            format!("horizon-prepare ended with {status}")
        } else {
            last
        })))
    })
    .await
}

/// Where one of Horizon's programs' `--launch-api` reply is kept, beside it.
pub fn kept_reply(program: &Path) -> PathBuf {
    let stem = program.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    program.with_file_name(format!("{stem}.launch-api.json"))
}

/// The options one of Horizon's programs offers (its `--launch-api` reply, kept beside it once
/// asked).
async fn launch_api(program: &Path) -> AppResult<Vec<String>> {
    let program = program.to_path_buf();
    let kept = kept_reply(&program);
    let reply = blocking(move || {
        if let Ok(reply) = fs::read_to_string(&kept) {
            return Ok(reply);
        }
        let out = quiet(&program)
            .arg("--launch-api")
            .stderr(Stdio::null())
            .output()
            .map_err(|e| AppError::new(ErrorCode::LaunchFailed, format!("{}: {e}", program.display())))?;
        let reply = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            let _ = fs::write(&kept, &reply);
        }
        Ok(reply)
    })
    .await?;
    args::offered(&reply)
}
