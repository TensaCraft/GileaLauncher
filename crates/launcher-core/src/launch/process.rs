//! The game process: its folder as the working directory, without
//! `JAVA_TOOL_OPTIONS`/`_JAVA_OPTIONS`, output appended to `logs/launch.log`, no
//! console window on Windows. It outlives the launcher.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const LAUNCH_LOG: &str = "launch.log";
/// The JVM's global options from the environment (`JDK_JAVA_OPTIONS`: the Java 9+ launcher's): the
/// game and the installers run with their own options only.
pub const JAVA_OPTION_VARIABLES: [&str; 3] = ["JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS", "JDK_JAVA_OPTIONS"];
pub const LOG_TAIL_LINES: usize = 40;
pub const LOG_TAIL_BYTES: u64 = 256 * 1024;
/// Files older than the launch by more than this belong to an earlier run.
pub const FRESHNESS: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Extra variables (GPU offload); Java option variables are removed even if listed here.
    pub env: Vec<(String, String)>,
    /// Output is appended here; `None` discards it.
    pub log: Option<PathBuf>,
}

impl GameCommand {
    /// `argv` is `[java, arguments…]`.
    pub fn new(
        argv: Vec<String>,
        cwd: &Path,
        env: Vec<(String, String)>,
        log: Option<PathBuf>,
    ) -> GameCommand {
        let mut argv = argv.into_iter();
        let program = PathBuf::from(argv.next().unwrap_or_default());
        GameCommand { program, args: argv.collect(), cwd: cwd.to_path_buf(), env, log }
    }
}

pub trait GameProcess: Send {
    fn pid(&self) -> u32;
    /// `Some(code)` once it exited; the code is `None` when a signal ended it.
    fn try_wait(&mut self) -> io::Result<Option<Option<i32>>>;
    fn kill(&mut self) -> io::Result<()>;
}

pub type SharedProcess = Arc<Mutex<Box<dyn GameProcess>>>;

pub trait Spawner: Send + Sync {
    fn spawn(&self, command: &GameCommand) -> io::Result<Box<dyn GameProcess>>;
}

/// Real processes.
pub struct SystemSpawner;

struct SystemProcess(Child);

impl GameProcess for SystemProcess {
    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
        Ok(self.0.try_wait()?.map(|status| status.code()))
    }

    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }
}

impl Spawner for SystemSpawner {
    fn spawn(&self, c: &GameCommand) -> io::Result<Box<dyn GameProcess>> {
        let mut command = Command::new(&c.program);
        command.args(&c.args).current_dir(&c.cwd).stdin(Stdio::null());
        // The game opens folders and links itself (xdg-open): not with the AppImage's environment.
        crate::platform::child_env::clean(&mut command);
        for (key, value) in &c.env {
            command.env(key, value);
        }
        for key in JAVA_OPTION_VARIABLES {
            command.env_remove(key);
        }
        match &c.log {
            Some(path) => {
                let out = OpenOptions::new().create(true).append(true).open(path)?;
                command.stdout(out.try_clone()?).stderr(out);
            }
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window for java.exe.
            command.creation_flags(0x0800_0000);
        }
        Ok(Box::new(SystemProcess(command.spawn()?)))
    }
}

/// Starts `logs/launch.log` with the original's header. When another copy of the build still plays
/// (`game_open`), its log is kept and the new header goes after it.
pub fn prepare_launch_log(game_dir: &Path, component: &str, minecraft: &str) -> io::Result<PathBuf> {
    let logs = game_dir.join("logs");
    fs::create_dir_all(&logs)?;
    let path = logs.join(LAUNCH_LOG);
    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let header = format!(
        "Minecraft process diagnostics\nloader={component}\nminecraft={minecraft}\ngame_dir={}\nstarted_at={started}\n\n",
        game_dir.display()
    );
    if super::alive::game_open(game_dir) {
        let mut log = OpenOptions::new().create(true).append(true).open(&path)?;
        log.write_all(format!("\n\n{header}").as_bytes())?;
    } else {
        fs::write(&path, header)?;
    }
    Ok(path)
}

/// The last `LOG_TAIL_LINES` lines within the last `LOG_TAIL_BYTES` of `path`.
pub fn log_tail(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n"))
}

/// The file most likely to explain a crash, written since the launch: the newest crash report,
/// else the newest `hs_err_*` (Java itself failed, out of memory say: the game's log just stops),
/// else `latest.log`, else the launch log.
pub fn crash_artifact(game_dir: &Path, launched_at: SystemTime) -> Option<PathBuf> {
    let since = launched_at.checked_sub(FRESHNESS).unwrap_or(launched_at);
    let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
    let fresh = |path: &Path| path.is_file() && modified(path).is_some_and(|t| t >= since);
    let newest = |dir: &Path, prefix: &str| {
        fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)) && fresh(p))
            .max_by_key(|p| modified(p))
    };
    newest(&game_dir.join("crash-reports"), "")
        .or_else(|| newest(game_dir, "hs_err_"))
        .or_else(|| Some(game_dir.join("logs").join("latest.log")).filter(|p| fresh(p)))
        .or_else(|| Some(game_dir.join("logs").join(LAUNCH_LOG)).filter(|p| fresh(p)))
}

/// What Java writes when the computer has no memory left for it.
const OUT_OF_MEMORY: [&str; 3] = [
    "There is insufficient memory for the Java Runtime Environment",
    "Native memory allocation (malloc) failed",
    "java.lang.OutOfMemoryError",
];

/// The game stopped for want of memory, by the end of the crash's own file or of the launch log
/// (where Java prints it).
pub fn ran_out_of_memory(game_dir: &Path, artifact: Option<&Path>) -> bool {
    let launch_log = game_dir.join("logs").join(LAUNCH_LOG);
    artifact
        .into_iter()
        .chain([launch_log.as_path()])
        .filter_map(|path| log_tail(path).ok())
        .any(|tail| OUT_OF_MEMORY.iter().any(|sign| tail.contains(sign)))
}

#[cfg(test)]
pub(crate) mod fake {
    use std::time::Instant;

    use super::*;

    /// Exits with `code` once `exits_at` passes (never, when `None`), or at once when killed.
    pub struct FakeProcess {
        pub pid: u32,
        pub exits_at: Option<Instant>,
        pub code: Option<i32>,
        pub killed: bool,
    }

    impl FakeProcess {
        pub fn exiting_after(pid: u32, after: Duration, code: Option<i32>) -> FakeProcess {
            FakeProcess { pid, exits_at: Some(Instant::now() + after), code, killed: false }
        }

        pub fn running(pid: u32) -> FakeProcess {
            FakeProcess { pid, exits_at: None, code: None, killed: false }
        }

        pub fn shared(self) -> SharedProcess {
            Arc::new(Mutex::new(Box::new(self)))
        }
    }

    impl GameProcess for FakeProcess {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
            if self.killed {
                return Ok(Some(None));
            }
            Ok(self.exits_at.filter(|at| Instant::now() >= *at).map(|_| self.code))
        }

        fn kill(&mut self) -> io::Result<()> {
            self.killed = true;
            Ok(())
        }
    }
}
