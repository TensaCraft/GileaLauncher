//! Installer processors of modern Forge and NeoForge: which ones run for the
//! client and with what arguments, whether their outputs are already right, and running them with
//! Java. A marker in the version folder remembers what they made, so a launch can tell a finished
//! install from one that stopped half-way.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::installer::ModernProfile;
use crate::launch::process::JAVA_OPTION_VARIABLES;
use crate::minecraft::library::library_path;
use crate::minecraft::version::version_dir;
use crate::safe_path::safe_relative;
use crate::storage::json::write_json_file;

/// `versions/<id>/.launcher-loader.json`: written once every processor is done.
pub const LOADER_MARKER: &str = ".launcher-loader.json";
/// Lines of processor output kept for an error message.
const OUTPUT_TAIL: usize = 20;

/// Where a processor plan is resolved.
pub struct ProcessorContext<'a> {
    pub mc_dir: &'a Path,
    /// The Minecraft version the loader is for.
    pub minecraft: &'a str,
    pub installer: &'a Path,
    /// Where `/data/…` files of the installer are unpacked.
    pub work_dir: &'a Path,
}

/// One processor, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessorCall {
    /// Its Maven coordinates, for messages.
    pub jar: String,
    /// The processor jar first, then its classpath.
    pub classpath: Vec<PathBuf>,
    pub main_class: String,
    pub args: Vec<String>,
    /// Files it makes and their expected SHA-1.
    pub outputs: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessorPlan {
    pub calls: Vec<ProcessorCall>,
    /// Installer entries to unpack first (`data/client.lzma` → the work folder).
    pub extracts: Vec<(String, PathBuf)>,
    /// Maven files named in `data`; the ones present afterwards go into the marker.
    pub artifacts: Vec<PathBuf>,
}

fn invalid(what: impl Into<String>) -> AppError {
    let what = what.into();
    AppError::new(ErrorCode::InvalidInput, format!("installer: {what}")).with_param("error", what)
}

fn failed(what: impl Into<String>) -> AppError {
    let what = what.into();
    AppError::new(ErrorCode::LoaderInstallFailed, what.clone()).with_param("error", what)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn wrapped(value: &str, open: char, close: char) -> Option<&str> {
    value.strip_prefix(open)?.strip_suffix(close)
}

/// `{KEY}` replaced by its value (an unknown key is an error); `\` keeps the next character.
fn replace_tokens(raw: &str, data: &BTreeMap<String, String>) -> AppResult<String> {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '{' => {
                let key: String = chars.by_ref().take_while(|c| *c != '}').collect();
                out.push_str(data.get(&key).ok_or_else(|| invalid(format!("unknown value {{{key}}}")))?);
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

/// `Main-Class` of a jar's manifest (continuation lines joined).
pub fn main_class(jar: &Path) -> AppResult<String> {
    let broken = |why: String| failed(format!("{}: {why}", jar.display()));
    let file = fs::File::open(jar).map_err(|e| broken(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| broken(e.to_string()))?;
    let mut manifest = String::new();
    archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|e| broken(e.to_string()))?
        .read_to_string(&mut manifest)
        .map_err(|e| broken(e.to_string()))?;
    let joined = manifest.replace("\r\n", "\n").replace("\n ", "");
    joined
        .lines()
        .find_map(|line| line.strip_prefix("Main-Class:"))
        .map(|class| class.trim().to_string())
        .filter(|class| !class.is_empty())
        .ok_or_else(|| broken("no Main-Class".into()))
}

/// Resolves the client processors of `profile`: `data` values are Maven paths
/// (`[…]`), literals (`'…'`) or installer files (`/data/…`, unpacked into the work folder); the
/// installer adds `SIDE`, `MINECRAFT_JAR`, `MINECRAFT_VERSION`, `ROOT`, `INSTALLER` and
/// `LIBRARY_DIR`. Every processor jar must already be in `libraries/` (its `Main-Class` is read).
pub fn plan_processors(profile: &ModernProfile, ctx: &ProcessorContext) -> AppResult<ProcessorPlan> {
    let libraries = ctx.mc_dir.join("libraries");
    let artifact = |coords: &str| -> AppResult<PathBuf> {
        library_path(coords)
            .map(|relative| libraries.join(relative))
            .ok_or_else(|| invalid(format!("unusable coordinates {coords}")))
    };
    let mut plan = ProcessorPlan::default();
    let mut data = BTreeMap::new();
    for (key, value) in &profile.data {
        let resolved = if let Some(coords) = wrapped(value, '[', ']') {
            let path = artifact(coords)?;
            plan.artifacts.push(path.clone());
            text(&path)
        } else if let Some(literal) = wrapped(value, '\'', '\'') {
            literal.to_string()
        } else {
            let entry = value.trim_start_matches('/');
            let relative =
                safe_relative(entry).ok_or_else(|| invalid(format!("unsafe data path {value}")))?;
            let dest = ctx.work_dir.join(relative);
            plan.extracts.push((entry.to_string(), dest.clone()));
            text(&dest)
        };
        data.insert(key.clone(), resolved);
    }
    let minecraft_jar = version_dir(ctx.mc_dir, ctx.minecraft).join(format!("{}.jar", ctx.minecraft));
    for (key, value) in [
        ("SIDE", "client".to_string()),
        ("MINECRAFT_JAR", text(&minecraft_jar)),
        ("MINECRAFT_VERSION", ctx.minecraft.to_string()),
        ("ROOT", text(ctx.mc_dir)),
        ("INSTALLER", text(ctx.installer)),
        ("LIBRARY_DIR", text(&libraries)),
    ] {
        data.insert(key.to_string(), value);
    }
    let value_of = |raw: &str| -> AppResult<String> {
        match (wrapped(raw, '[', ']'), wrapped(raw, '\'', '\'')) {
            (Some(coords), _) => artifact(coords).map(|path| text(&path)),
            (None, Some(literal)) => Ok(literal.to_string()),
            (None, None) => replace_tokens(raw, &data),
        }
    };
    for processor in profile.processors.iter().filter(|p| p.runs_on_client()) {
        let jar = artifact(&processor.jar)?;
        let mut classpath = vec![jar.clone()];
        for coords in &processor.classpath {
            classpath.push(artifact(coords)?);
        }
        let args = processor.args.iter().map(|arg| value_of(arg)).collect::<AppResult<Vec<_>>>()?;
        let outputs = processor
            .outputs
            .iter()
            .map(|(path, sha1)| Ok((PathBuf::from(value_of(path)?), value_of(sha1)?)))
            .collect::<AppResult<Vec<_>>>()?;
        plan.calls.push(ProcessorCall {
            jar: processor.jar.clone(),
            main_class: main_class(&jar)?,
            classpath,
            args,
            outputs,
        });
    }
    Ok(plan)
}

fn sha1_of(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| hex::encode(Sha1::digest(&bytes)))
}

/// Every output exists with its SHA-1 (an empty expectation only needs the file).
pub fn outputs_ok(outputs: &[(PathBuf, String)]) -> bool {
    outputs.iter().all(|(path, sha1)| match sha1.trim() {
        "" => path.is_file(),
        expected => sha1_of(path).is_some_and(|actual| actual.eq_ignore_ascii_case(expected)),
    })
}

/// Runs one processor; `Err` says what went wrong, with the end of its output.
pub trait ProcessorRunner: Send + Sync {
    fn run(&self, java: &Path, call: &ProcessorCall, cwd: &Path) -> Result<(), String>;
}

/// `java -cp <classpath> <Main-Class> <args>` without a console window, the JVM's global options
/// (`JAVA_OPTION_VARIABLES`) or the AppImage's environment.
pub struct JavaProcessorRunner;

impl ProcessorRunner for JavaProcessorRunner {
    fn run(&self, java: &Path, call: &ProcessorCall, cwd: &Path) -> Result<(), String> {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let classpath = call.classpath.iter().map(|path| text(path)).collect::<Vec<_>>().join(separator);
        let mut command = Command::new(java);
        command
            .arg("-cp")
            .arg(classpath)
            .arg(&call.main_class)
            .args(&call.args)
            .current_dir(cwd)
            .stdin(Stdio::null());
        crate::platform::child_env::clean(&mut command);
        for key in JAVA_OPTION_VARIABLES {
            command.env_remove(key);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window for java.exe.
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().map_err(|e| format!("{} could not start: {e}", java.display()))?;
        let printed =
            format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        tracing::debug!("Processor {} said:\n{printed}", call.jar);
        if output.status.success() {
            return Ok(());
        }
        let lines: Vec<&str> = printed.lines().collect();
        let tail = lines[lines.len().saturating_sub(OUTPUT_TAIL)..].join("\n");
        Err(format!("{} ({}) ended with {}: {tail}", call.jar, call.main_class, output.status))
    }
}

/// Remembers what the installer made — the jars it shipped and what its processors wrote: every
/// `artifacts` file present now, relative to `mc_dir`, with its SHA-1 (schema 2).
pub fn write_marker(mc_dir: &Path, id: &str, artifacts: &[PathBuf]) -> AppResult<()> {
    let outputs: Vec<Value> = artifacts
        .iter()
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let relative = path
                .strip_prefix(mc_dir)
                .ok()?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            Some(json!({"path": relative, "sha1": sha1_of(path)?}))
        })
        .collect();
    let path = version_dir(mc_dir, id).join(LOADER_MARKER);
    write_json_file(&path, &json!({"schema": 2, "outputs": outputs}), 2)
        .map_err(|e| AppError::new(ErrorCode::Io, format!("{}: {e}", path.display())))
}

/// The installer steps of `id` finished and what they made is still there — with `deep`, also
/// unchanged (by SHA-1). A schema-1 marker (paths only) passes on presence.
pub fn marker_ready(mc_dir: &Path, id: &str, deep: bool) -> bool {
    let Ok(raw) = fs::read(version_dir(mc_dir, id).join(LOADER_MARKER)) else { return false };
    let Ok(marker) = serde_json::from_slice::<Value>(&raw) else { return false };
    let Some(outputs) = marker["outputs"].as_array() else { return false };
    let present = |relative: &str| safe_relative(relative).map(|p| mc_dir.join(p)).filter(|p| p.is_file());
    match marker["schema"].as_u64() {
        Some(1) => outputs.iter().all(|output| output.as_str().and_then(present).is_some()),
        Some(2) => outputs.iter().all(|output| {
            let Some(path) = output["path"].as_str().and_then(present) else { return false };
            !deep
                || output["sha1"].as_str().is_some_and(|expected| {
                    sha1_of(&path).is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
                })
        }),
        _ => false,
    }
}

/// Forgets that the processors of `id` finished (before they run again).
pub fn clear_marker(mc_dir: &Path, id: &str) {
    let _ = fs::remove_file(version_dir(mc_dir, id).join(LOADER_MARKER));
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;
    use crate::loaders::installer::Processor;

    /// `root` joined with the `/`-separated `parts`.
    fn path(root: &Path, parts: &str) -> PathBuf {
        parts.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
    }

    fn jar_with_main(jar: &Path, main: &str) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("META-INF/MANIFEST.MF", zip::write::SimpleFileOptions::default()).unwrap();
        // A long name wraps onto a continuation line, as in real manifests.
        write!(zip, "Manifest-Version: 1.0\r\nMain-Class: {}\r\n {}\r\n\r\n", &main[..10], &main[10..])
            .unwrap();
        fs::create_dir_all(jar.parent().unwrap()).unwrap();
        fs::write(jar, zip.finish().unwrap().into_inner()).unwrap();
    }

    fn profile() -> ModernProfile {
        let data = [
            ("PATCHED", "[net.minecraftforge:forge:1.20.1-47.4.10:client]"),
            ("PATCHED_SHA", "'4d8a'"),
            ("BINPATCH", "/data/client.lzma"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let processor = |sides: Option<&str>, args: &[&str], outputs: &[(&str, &str)]| Processor {
            jar: "net.minecraftforge:binarypatcher:1.1.1".into(),
            classpath: vec!["net.sf.jopt-simple:jopt-simple:5.0.4".into()],
            args: args.iter().map(|a| a.to_string()).collect(),
            outputs: outputs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            sides: sides.map(|side| vec![side.to_string()]),
        };
        ModernProfile {
            minecraft: "1.20.1".into(),
            json: "version.json".into(),
            data,
            processors: vec![
                processor(Some("server"), &["--task", "SERVER"], &[]),
                processor(
                    None,
                    &[
                        "--clean",
                        "{MINECRAFT_JAR}",
                        "--output",
                        "{PATCHED}",
                        "--apply",
                        "{BINPATCH}",
                        "--side",
                        "{SIDE}",
                        "--root",
                        "{ROOT}",
                        "--mcp",
                        "[de.oceanlabs.mcp:mcp_config:1.20.1@zip]",
                        "--brace",
                        "\\{SIDE\\}",
                    ],
                    &[("{PATCHED}", "{PATCHED_SHA}")],
                ),
            ],
            libraries: Vec::new(),
        }
    }

    const PATCHER: &str = "net/minecraftforge/binarypatcher/1.1.1/binarypatcher-1.1.1.jar";

    #[test]
    fn client_processors_get_their_values() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path().join("mc");
        let libraries = mc.join("libraries");
        jar_with_main(&path(&libraries, PATCHER), "net.minecraftforge.binarypatcher.ConsoleTool");
        let (installer, work) = (mc.join("installer.jar"), mc.join("work"));
        let ctx =
            ProcessorContext { mc_dir: &mc, minecraft: "1.20.1", installer: &installer, work_dir: &work };
        let plan = plan_processors(&profile(), &ctx).unwrap();
        assert_eq!(plan.calls.len(), 1, "the server step is skipped");
        let call = &plan.calls[0];
        assert_eq!(call.main_class, "net.minecraftforge.binarypatcher.ConsoleTool");
        assert_eq!(
            call.classpath,
            [
                path(&libraries, PATCHER),
                path(&libraries, "net/sf/jopt-simple/jopt-simple/5.0.4/jopt-simple-5.0.4.jar")
            ]
        );
        let patched =
            path(&libraries, "net/minecraftforge/forge/1.20.1-47.4.10/forge-1.20.1-47.4.10-client.jar");
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            call.args,
            [
                "--clean".to_string(),
                s(&path(&mc, "versions/1.20.1/1.20.1.jar")),
                "--output".into(),
                s(&patched),
                "--apply".into(),
                s(&path(&work, "data/client.lzma")),
                "--side".into(),
                "client".into(),
                "--root".into(),
                s(&mc),
                "--mcp".into(),
                s(&path(&libraries, "de/oceanlabs/mcp/mcp_config/1.20.1/mcp_config-1.20.1.zip")),
                "--brace".into(),
                "{SIDE}".into(),
            ]
        );
        assert_eq!(call.outputs, [(patched.clone(), "4d8a".to_string())]);
        assert_eq!(plan.extracts, [("data/client.lzma".to_string(), path(&work, "data/client.lzma"))]);
        assert_eq!(plan.artifacts, [patched]);
    }

    #[test]
    fn unknown_values_and_escaping_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path().join("mc");
        let (installer, work) = (mc.join("installer.jar"), mc.join("work"));
        let ctx =
            ProcessorContext { mc_dir: &mc, minecraft: "1.20.1", installer: &installer, work_dir: &work };
        assert_eq!(
            plan_processors(&profile(), &ctx).unwrap_err().code,
            ErrorCode::LoaderInstallFailed,
            "a processor jar that is not there cannot run"
        );
        jar_with_main(&path(&mc.join("libraries"), PATCHER), "net.minecraftforge.binarypatcher.ConsoleTool");
        let mut unknown = profile();
        unknown.processors[1].args.push("{NOPE}".into());
        assert_eq!(plan_processors(&unknown, &ctx).unwrap_err().code, ErrorCode::InvalidInput);
        let mut escaping = profile();
        escaping.data.insert("BINPATCH".into(), "/../../evil.lzma".into());
        assert_eq!(plan_processors(&escaping, &ctx).unwrap_err().code, ErrorCode::InvalidInput);
        let mut coords = profile();
        coords.processors[1].jar = "../..:x:1".into();
        assert_eq!(plan_processors(&coords, &ctx).unwrap_err().code, ErrorCode::InvalidInput);
        assert!(!dir.path().join("evil.lzma").exists());
    }

    #[test]
    fn outputs_and_the_marker_tell_a_finished_install() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path();
        let out = mc.join("libraries").join("x.jar");
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        let expected = hex::encode(Sha1::digest(b"patched"));
        assert!(!outputs_ok(&[(out.clone(), expected.clone())]));
        fs::write(&out, b"patched").unwrap();
        assert!(outputs_ok(&[(out.clone(), expected.to_uppercase())]));
        assert!(!outputs_ok(&[(out.clone(), "0".repeat(40))]));
        let id = "neoforge-21.1.77";
        fs::create_dir_all(version_dir(mc, id)).unwrap();
        assert!(!marker_ready(mc, id, false));
        write_marker(mc, id, &[out.clone(), mc.join("libraries").join("never-made.jar")]).unwrap();
        assert!(marker_ready(mc, id, false) && marker_ready(mc, id, true));
        fs::write(&out, b"PATCHED").unwrap();
        assert!(marker_ready(mc, id, false), "the same size passes the quick check");
        assert!(!marker_ready(mc, id, true), "the deep check compares the SHA-1");
        fs::remove_file(&out).unwrap();
        assert!(!marker_ready(mc, id, false), "a deleted output needs the processors again");
        fs::write(&out, b"patched").unwrap();
        fs::write(
            version_dir(mc, id).join(LOADER_MARKER),
            r#"{"schema": 1, "outputs": ["libraries/x.jar"]}"#,
        )
        .unwrap();
        assert!(marker_ready(mc, id, true), "a schema-1 marker (paths only) passes on presence");
        clear_marker(mc, id);
        assert!(!version_dir(mc, id).join(LOADER_MARKER).exists());
    }
}
