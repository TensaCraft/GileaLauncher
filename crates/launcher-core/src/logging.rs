//! File logging with size-based rotation (512 KiB x 3) and secret redaction.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

pub const LOG_FILE: &str = "app.log";
pub const LOG_MAX_BYTES: u64 = 512 * 1024;
pub const LOG_BACKUPS: usize = 3;

pub struct SizeRotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    backups: usize,
    file: File,
    size: u64,
}

impl SizeRotatingWriter {
    pub fn open(path: PathBuf, max_bytes: u64, backups: usize, truncate: bool) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file =
            OpenOptions::new().create(true).append(!truncate).write(true).truncate(truncate).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self { path, max_bytes, backups, file, size })
    }

    fn backup_path(&self, n: usize) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{n}"));
        self.path.with_file_name(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        if self.backups == 0 {
            self.file = OpenOptions::new().write(true).truncate(true).open(&self.path)?;
        } else {
            let _ = fs::remove_file(self.backup_path(self.backups));
            for n in (1..self.backups).rev() {
                let from = self.backup_path(n);
                if from.exists() {
                    fs::rename(&from, self.backup_path(n + 1))?;
                }
            }
            fs::rename(&self.path, self.backup_path(1))?;
            self.file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        }
        self.size = 0;
        Ok(())
    }
}

impl Write for SizeRotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// `2026-09-26 12:00:00 INFO     message`
pub struct LineFormat;

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        write!(writer, "{now} {:<8} ", event.metadata().level().as_str())?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// The file logging writes to, once initialised (it may be the fallback directory's).
pub fn log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}

/// Initialises global logging once. Tries `dir`, then `fallback_dir`.
pub fn init_logging(dir: &Path, fallback_dir: &Path) -> io::Result<PathBuf> {
    if let Some(p) = LOG_PATH.get() {
        return Ok(p.clone());
    }
    let truncate = std::env::var("LAUNCHER_CLEAR_LOG_ON_START").as_deref() == Ok("1");
    let (path, writer) = [dir, fallback_dir]
        .iter()
        .map(|d| d.join(LOG_FILE))
        .find_map(|p| {
            SizeRotatingWriter::open(p.clone(), LOG_MAX_BYTES, LOG_BACKUPS, truncate).ok().map(|w| (p, w))
        })
        .ok_or_else(|| io::Error::other("no writable log directory"))?;

    let filter = tracing_subscriber::EnvFilter::try_from_env("LAUNCHER_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
    let file_writer = Mutex::new(RedactingWriter::new(writer, home.as_deref()));
    let init = if cfg!(debug_assertions) {
        use tracing_subscriber::fmt::writer::MakeWriterExt;
        tracing_subscriber::fmt()
            .event_format(LineFormat)
            .with_env_filter(filter)
            .with_writer(file_writer.and(io::stdout))
            .try_init()
    } else {
        tracing_subscriber::fmt()
            .event_format(LineFormat)
            .with_env_filter(filter)
            .with_writer(file_writer)
            .try_init()
    };
    if init.is_err() {
        // Another subscriber is already installed (tests, embedding): keep it.
    }
    let _ = LOG_PATH.set(path.clone());
    tracing::info!("Logging initialised");
    Ok(path)
}

/// Writes what it is given with `redact` applied: the log file never shows the home
/// folder or a token.
pub struct RedactingWriter<W> {
    inner: W,
    redactor: Redactor,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W, home: Option<&str>) -> RedactingWriter<W> {
        RedactingWriter { inner, redactor: Redactor::new(home) }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    /// Each call is one whole event (the formatter writes a line at once), so what it redacts is
    /// never cut in two.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = self.redactor.apply(&String::from_utf8_lossy(buf));
        self.inner.write_all(text.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Removes access/refresh tokens, client secrets, bearer headers and the home directory.
pub fn redact(text: &str, home: Option<&str>) -> String {
    Redactor::new(home).apply(text)
}

/// `redact` with the home folder's patterns made once.
struct Redactor {
    home: Vec<regex::Regex>,
}

impl Redactor {
    fn new(home: Option<&str>) -> Redactor {
        let home = home.map(str::trim).filter(|h| h.len() > 2).map_or_else(Vec::new, |home| {
            [home.to_string(), home.replace('\\', "/"), home.replace('/', "\\")]
                .iter()
                .map(|variant| {
                    regex::Regex::new(&format!("(?i){}", regex::escape(variant))).expect("escaped regex")
                })
                .collect()
        });
        Redactor { home }
    }

    fn apply(&self, text: &str) -> String {
        use std::sync::LazyLock;
        static PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
            [
                r"(?i)(authorization\s*:\s*bearer\s+)([^\s]+)",
                r"(?i)(--accessToken\s+)([^\s]+)",
                r#"(?ix)(["']?(?:access[_-]?token|refresh[_-]?token|client[_-]?secret)["']?\s*[:=]\s*["']?)([^"',\s}]+)"#,
            ]
            .iter()
            .map(|p| regex::Regex::new(p).expect("valid redaction regex"))
            .collect()
        });
        let mut out = text.to_string();
        for re in &self.home {
            out = re.replace_all(&out, "<USER_HOME>").into_owned();
        }
        for re in PATTERNS.iter() {
            out = re.replace_all(&out, "${1}<redacted>").into_owned();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[test]
    fn rotates_by_size_and_keeps_three_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG_FILE);
        let mut w = SizeRotatingWriter::open(path.clone(), 100, 3, false).unwrap();
        for i in 0..20 {
            writeln!(w, "line {i:02} {}", "x".repeat(40)).unwrap();
        }
        w.flush().unwrap();
        for n in 1..=3 {
            assert!(dir.path().join(format!("app.log.{n}")).is_file(), "backup {n}");
        }
        assert!(!dir.path().join("app.log.4").exists());
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.contains("line 19"));
        assert!(std::fs::metadata(&path).unwrap().len() <= 100);
    }

    #[test]
    fn truncate_clears_existing_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG_FILE);
        std::fs::write(&path, "old content\n").unwrap();
        let mut w = SizeRotatingWriter::open(path.clone(), 1024, 3, true).unwrap();
        writeln!(w, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
    }

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn line_format_matches_original_layout() {
        let buf = Buf::default();
        let out = buf.clone();
        let subscriber =
            tracing_subscriber::fmt().event_format(LineFormat).with_writer(move || buf.clone()).finish();
        tracing::subscriber::with_default(subscriber, || tracing::info!("Logging initialised"));
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} INFO     Logging initialised\n$")
            .unwrap();
        assert!(re.is_match(&text), "{text:?}");
    }

    #[test]
    fn the_log_file_hides_the_home_folder() {
        let buf = Buf::default();
        let mut writer = RedactingWriter::new(buf.clone(), Some("C:\\Users\\Ivan"));
        writer.write_all(b"2026-09-29 INFO Reading C:\\Users\\Ivan\\AppData --accessToken eyJ1\n").unwrap();
        let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "2026-09-29 INFO Reading <USER_HOME>\\AppData --accessToken <redacted>\n");
    }

    #[test]
    fn redacts_tokens_and_home_dir() {
        let input = "Authorization: Bearer abc.def --accessToken eyJ123 \"refresh_token\": \"r1\" \
                     client_secret=zzz C:\\Users\\Ivan\\AppData c:/users/ivan/x";
        let out = redact(input, Some("C:\\Users\\Ivan"));
        assert!(out.contains("Authorization: Bearer <redacted>"));
        assert!(out.contains("--accessToken <redacted>"));
        assert!(out.contains("\"refresh_token\": \"<redacted>"));
        assert!(out.contains("client_secret=<redacted>"));
        assert!(out.contains("<USER_HOME>\\AppData"));
        assert!(out.contains("<USER_HOME>/x"));
        assert!(!out.contains("abc.def") && !out.contains("eyJ123") && !out.contains("zzz"));
    }
}
