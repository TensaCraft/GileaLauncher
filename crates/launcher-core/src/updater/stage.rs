//! Staging an update for installation and validating it before use.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};

use super::download::sha256_file;
use super::select::sanitize_file_name;
use super::version::Version;
use crate::paths::{Os, PathEnv, is_protected_dir, probe_writable};
use crate::storage::atomic::atomic_write;

pub const PENDING_DIR: &str = "pending-update";
pub const STAGING_DIR: &str = "staging";
pub const DOWNLOADS_DIR: &str = "downloads";
pub const MARKER_FILE: &str = "pending_update.json";
pub const APPLY_LOG: &str = "apply.log";
pub const MARKER_SCHEMA: u32 = 3;
/// Held (file lock) by a running helper.
pub const HELPER_LOCK: &str = "helper.lock";
/// Written into the staging folder when a helper is started.
pub const HELPER_STARTED: &str = "helper-started";

/// Facts about the running launcher that the updater needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecContext {
    pub os: Os,
    pub current_exe: PathBuf,
    /// `$APPIMAGE` when running as an AppImage.
    pub appimage: Option<PathBuf>,
    pub pid: u32,
}

impl ExecContext {
    pub fn current() -> io::Result<ExecContext> {
        let current_exe = std::env::current_exe()?;
        Ok(ExecContext {
            os: Os::current(),
            appimage: current_appimage(&current_exe),
            current_exe,
            pid: std::process::id(),
        })
    }

    /// The file the helper is copied from; a copy of an AppImage's inner binary would not start.
    pub fn trusted_binary(&self) -> &Path {
        self.appimage.as_deref().unwrap_or(&self.current_exe)
    }
}

/// `$APPIMAGE` is exported to every process started from an AppImage (a terminal, an IDE); it
/// describes this process only when the executable lives inside that AppImage's mount (`$APPDIR`).
pub fn appimage_from(
    appimage: Option<OsString>,
    appdir: Option<OsString>,
    current_exe: &Path,
) -> Option<PathBuf> {
    let appimage = PathBuf::from(appimage?);
    let appdir = PathBuf::from(appdir?);
    (appimage.is_absolute() && appdir.is_absolute() && current_exe.starts_with(&appdir)).then_some(appimage)
}

/// The AppImage this process runs from, if any.
pub fn current_appimage(current_exe: &Path) -> Option<PathBuf> {
    appimage_from(std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR"), current_exe)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub schema: u32,
    pub platform: String,
    pub version: String,
    pub helper: PathBuf,
    pub source: PathBuf,
    pub target: PathBuf,
    pub source_sha256: String,
    pub helper_sha256: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    Nothing,
    /// The helper was started; the launcher must exit now.
    Launched,
    /// A pending update was invalid and has been removed.
    Discarded(String),
    /// Another helper is installing the update right now; the launcher must exit.
    InProgress,
}

pub fn pending_dir(cache: &Path) -> PathBuf {
    cache.join(PENDING_DIR)
}

pub fn staging_dir(cache: &Path) -> PathBuf {
    pending_dir(cache).join(STAGING_DIR)
}

pub fn marker_path(cache: &Path) -> PathBuf {
    pending_dir(cache).join(MARKER_FILE)
}

pub fn helper_lock_path(cache: &Path) -> PathBuf {
    pending_dir(cache).join(HELPER_LOCK)
}

pub fn download_path(cache: &Path, version: &str, asset_name: &str) -> PathBuf {
    pending_dir(cache)
        .join(DOWNLOADS_DIR)
        .join(sanitize_file_name(version))
        .join(sanitize_file_name(asset_name))
}

fn io_err(e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

fn rejected(detail: &str) -> AppError {
    AppError::new(ErrorCode::InvalidInput, format!("pending update rejected: {detail}"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Owned by this user and closed to everyone else (staging is `0700`).
#[cfg(unix)]
fn private_to_current_user(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    meta.uid() == euid && meta.mode() & 0o077 == 0
}

/// Per-user profile folders are already closed to other users by their ACLs.
#[cfg(not(unix))]
fn private_to_current_user(_meta: &fs::Metadata) -> bool {
    true
}

/// What gets replaced: the exe (Windows, Linux binary), the AppImage, or the enclosing `.app`.
pub fn update_target(ctx: &ExecContext) -> AppResult<PathBuf> {
    match ctx.os {
        Os::MacOs => ctx
            .current_exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                AppError::new(ErrorCode::Unsupported, "the launcher is not running from an .app bundle")
            }),
        Os::Linux => Ok(ctx.appimage.clone().unwrap_or_else(|| ctx.current_exe.clone())),
        Os::Windows | Os::Other => Ok(ctx.current_exe.clone()),
    }
}

fn payload_name(ctx: &ExecContext) -> &'static str {
    match ctx.os {
        Os::Windows => "update_payload.exe",
        Os::MacOs => "update_payload.dmg",
        _ if ctx.appimage.is_some() => "update_payload.AppImage",
        _ => "update_payload.bin",
    }
}

fn helper_name(ctx: &ExecContext) -> &'static str {
    match ctx.os {
        Os::Windows => "update_helper.exe",
        _ if ctx.appimage.is_some() => "update_helper.AppImage",
        _ => "update_helper",
    }
}

fn reset_dir(dir: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(dir)?;
        } else {
            fs::remove_file(dir)?;
        }
    }
    fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)
}

/// Copies the verified payload and a helper (a copy of this very launcher) into the staging
/// folder and writes the marker.
pub fn prepare(
    cache: &Path,
    ctx: &ExecContext,
    env: &PathEnv,
    payload: &Path,
    version: &str,
    expected_sha256: &str,
) -> AppResult<Marker> {
    let target = update_target(ctx)?;
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let refuse = |detail: String| {
        AppError::new(ErrorCode::InvalidDirectoryPath, detail).with_param("path", folder.to_string_lossy())
    };
    if is_protected_dir(env, &target) {
        return Err(refuse("the launcher is installed in a protected folder".to_string()));
    }
    // The helper renames the program inside this folder; find out now rather than after exiting.
    if let Err(e) = probe_writable(&folder) {
        return Err(refuse(format!("the install folder is not writable: {e}")));
    }
    let staging = staging_dir(cache);
    reset_dir(&staging).map_err(io_err)?;
    let source = staging.join(payload_name(ctx));
    let helper = staging.join(helper_name(ctx));
    fs::copy(payload, &source).map_err(io_err)?;
    fs::copy(ctx.trusted_binary(), &helper).map_err(io_err)?;
    set_mode(&source, 0o700).map_err(io_err)?;
    set_mode(&helper, 0o700).map_err(io_err)?;
    let source_sha256 = sha256_file(&source).map_err(io_err)?;
    if source_sha256 != expected_sha256 {
        let _ = fs::remove_dir_all(&staging);
        return Err(AppError::new(
            ErrorCode::IntegrityMismatch,
            "staged payload does not match the release digest",
        ));
    }
    let marker = Marker {
        schema: MARKER_SCHEMA,
        platform: ctx.os.as_str().to_string(),
        version: version.to_string(),
        helper_sha256: sha256_file(&helper).map_err(io_err)?,
        helper,
        source,
        target,
        source_sha256,
        created_at: now_secs(),
    };
    let json = serde_json::to_vec_pretty(&marker).map_err(|e| AppError::internal(e.to_string()))?;
    let path = marker_path(cache);
    atomic_write(&path, &json).map_err(io_err)?;
    set_mode(&path, 0o600).map_err(io_err)?;
    Ok(marker)
}

/// Strict check. `trusted_helper_sha256` is the SHA-256 of the running launcher
/// (or its AppImage): the helper must be an exact copy of a program we already trust.
pub fn validate_marker(cache: &Path, os: Os, trusted_helper_sha256: Option<&str>) -> AppResult<Marker> {
    let bytes = fs::read(marker_path(cache)).map_err(io_err)?;
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|e| rejected(&e.to_string()))?;
    if marker.schema != MARKER_SCHEMA {
        return Err(rejected("unknown schema"));
    }
    if marker.platform != os.as_str() {
        return Err(rejected("prepared for another platform"));
    }
    let staging = staging_dir(cache);
    let staging_meta = fs::symlink_metadata(&staging).map_err(|_| rejected("staging folder is missing"))?;
    if !staging_meta.is_dir() || staging_meta.file_type().is_symlink() {
        return Err(rejected("staging folder is not a real folder"));
    }
    if !private_to_current_user(&staging_meta) {
        return Err(rejected("staging folder is open to other users"));
    }
    let staging = fs::canonicalize(&staging).map_err(io_err)?;
    if marker.helper == marker.source {
        return Err(rejected("helper and payload are the same file"));
    }
    for file in [&marker.helper, &marker.source] {
        let meta = fs::symlink_metadata(file).map_err(|_| rejected("a staged file is missing"))?;
        if !meta.file_type().is_file() {
            return Err(rejected("a staged file is not a regular file"));
        }
        let parent = file.parent().and_then(|p| fs::canonicalize(p).ok());
        if parent.as_deref() != Some(staging.as_path()) {
            return Err(rejected("a staged file is outside the staging folder"));
        }
    }
    if sha256_file(&marker.source).map_err(io_err)? != marker.source_sha256 {
        return Err(rejected("payload changed"));
    }
    if sha256_file(&marker.helper).map_err(io_err)? != marker.helper_sha256 {
        return Err(rejected("helper changed"));
    }
    if let Some(trusted) = trusted_helper_sha256
        && trusted != marker.helper_sha256
    {
        return Err(rejected("helper is not a copy of this launcher"));
    }
    if fs::symlink_metadata(&marker.target).is_err() {
        return Err(rejected("update target is missing"));
    }
    Ok(marker)
}

/// The launcher-side check: the strict marker check, and the marker must be for this very
/// program (a planted marker must not make the helper replace some other path).
pub fn validate_for(cache: &Path, ctx: &ExecContext) -> AppResult<Marker> {
    let trusted = sha256_file(ctx.trusted_binary()).map_err(io_err)?;
    let marker = validate_marker(cache, ctx.os, Some(&trusted))?;
    let expected = update_target(ctx)?;
    let same = match (fs::canonicalize(&marker.target), fs::canonicalize(&expected)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same {
        return Err(rejected("prepared for another program"));
    }
    Ok(marker)
}

/// Starts `<helper> --apply-update <marker> --wait-pid <pid>` as a detached process
/// (arguments as an array, never through a shell).
pub fn spawn_helper(cache: &Path, marker: &Marker, wait_pid: u32) -> AppResult<()> {
    // Removed by the helper only when it gives up without touching anything; if it is killed
    // mid-way (antivirus, crash), the next start sees this and stops retrying.
    let started = staging_dir(cache).join(HELPER_STARTED);
    fs::write(&started, now_secs().to_string()).map_err(io_err)?;
    let mut cmd = Command::new(&marker.helper);
    cmd.arg("--apply-update").arg(marker_path(cache)).arg("--wait-pid").arg(wait_pid.to_string());
    super::apply::detach(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| {
        let _ = fs::remove_file(&started);
        io_err(e)
    })
}

/// True while a helper holds its lock (the lock is released when that process ends).
fn helper_running(cache: &Path) -> bool {
    let Ok(file) = fs::OpenOptions::new().write(true).open(helper_lock_path(cache)) else { return false };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
}

pub fn discard_pending(cache: &Path) {
    let _ = fs::remove_file(marker_path(cache));
    let _ = fs::remove_dir_all(staging_dir(cache));
}

/// Called at startup by the primary instance only (after single-instance).
pub fn resume_pending(cache: &Path, ctx: &ExecContext) -> ResumeOutcome {
    if !marker_path(cache).exists() {
        return ResumeOutcome::Nothing;
    }
    if helper_running(cache) {
        return ResumeOutcome::InProgress;
    }
    if staging_dir(cache).join(HELPER_STARTED).exists() {
        discard_pending(cache);
        return ResumeOutcome::Discarded("the previous installation attempt did not finish".to_string());
    }
    let outcome = validate_for(cache, ctx).and_then(|marker| spawn_helper(cache, &marker, ctx.pid));
    match outcome {
        Ok(()) => ResumeOutcome::Launched,
        Err(e) => {
            discard_pending(cache);
            ResumeOutcome::Discarded(e.detail)
        }
    }
}

/// Removes leftovers when nothing is pending: the staging folder and downloads of versions that
/// are not newer than the running one (a half-downloaded newer version is kept for resuming).
pub fn cleanup_after_update(cache: &Path, current: &Version) {
    if marker_path(cache).exists() {
        return;
    }
    let _ = fs::remove_dir_all(staging_dir(cache));
    let Ok(entries) = fs::read_dir(pending_dir(cache).join(DOWNLOADS_DIR)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Version::parse(&name).is_none_or(|v| v <= *current) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    struct Setup {
        _tmp: tempfile::TempDir,
        cache: PathBuf,
        ctx: ExecContext,
        payload: PathBuf,
        payload_sha: String,
    }

    /// An "installed" launcher (a plain file is enough here) and a downloaded payload.
    fn setup() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("Іван & 100% Co").join("app");
        let exe = if cfg!(target_os = "macos") {
            app.join("Launcher.app").join("Contents").join("MacOS").join("Launcher")
        } else {
            app.join(if cfg!(windows) { "Launcher.exe" } else { "Launcher" })
        };
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"launcher 0.1.0").unwrap();
        let payload = tmp.path().join("download").join("Launcher-new");
        fs::create_dir_all(payload.parent().unwrap()).unwrap();
        fs::write(&payload, b"launcher 0.2.0").unwrap();
        let payload_sha = sha256_file(&payload).unwrap();
        let ctx = ExecContext { os: Os::current(), current_exe: exe, appimage: None, pid: 4242 };
        Setup { cache: tmp.path().join("cache"), _tmp: tmp, ctx, payload, payload_sha }
    }

    fn trusted(s: &Setup) -> String {
        sha256_file(s.ctx.trusted_binary()).unwrap()
    }

    #[test]
    fn prepare_writes_a_strict_marker_that_validates() {
        let s = setup();
        let marker =
            prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha).unwrap();
        let raw: Value = serde_json::from_slice(&fs::read(marker_path(&s.cache)).unwrap()).unwrap();
        let mut keys: Vec<&str> = raw.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "created_at",
                "helper",
                "helper_sha256",
                "platform",
                "schema",
                "source",
                "source_sha256",
                "target",
                "version"
            ]
        );
        assert_eq!(marker.schema, MARKER_SCHEMA);
        assert_eq!(marker.target, update_target(&s.ctx).unwrap());
        assert_eq!(validate_marker(&s.cache, Os::current(), Some(&trusted(&s))).unwrap(), marker);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&staging_dir(&s.cache)), 0o700);
            assert_eq!(mode(&marker_path(&s.cache)), 0o600);
        }
    }

    #[test]
    fn prepare_rejects_a_payload_with_the_wrong_hash() {
        let s = setup();
        let err =
            prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &"0".repeat(64)).unwrap_err();
        assert_eq!(err.code, ErrorCode::IntegrityMismatch);
        assert!(!marker_path(&s.cache).exists());
    }

    #[test]
    fn prepare_refuses_protected_install_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("Program Files").join("Launcher");
        fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("Launcher.exe");
        fs::write(&exe, b"x").unwrap();
        let payload = tmp.path().join("p");
        fs::write(&payload, b"y").unwrap();
        let ctx = ExecContext { os: Os::Windows, current_exe: exe, appimage: None, pid: 1 };
        let env = PathEnv { exe_dir: Some(exe_dir), ..PathEnv::default() };
        let sha = sha256_file(&payload).unwrap();
        let err = prepare(&tmp.path().join("cache"), &ctx, &env, &payload, "0.2.0", &sha).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidDirectoryPath);
    }

    type Mutation = Box<dyn Fn(&mut serde_json::Map<String, Value>)>;

    #[test]
    fn tampered_markers_are_rejected() {
        let s = setup();
        prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha).unwrap();
        let original = fs::read(marker_path(&s.cache)).unwrap();
        let edit = |f: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            let mut v: Value = serde_json::from_slice(&original).unwrap();
            f(v.as_object_mut().unwrap());
            fs::write(marker_path(&s.cache), serde_json::to_vec(&v).unwrap()).unwrap();
        };
        let outside = s.cache.join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        let copied_helper = outside.join("helper");
        let marker: Marker = serde_json::from_slice(&original).unwrap();
        fs::copy(&marker.helper, &copied_helper).unwrap();
        let outside_helper = copied_helper.to_string_lossy().into_owned();
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "extra key",
                Box::new(|m| {
                    m.insert("extra".into(), Value::Bool(true));
                }),
            ),
            (
                "missing key",
                Box::new(|m| {
                    m.remove("created_at");
                }),
            ),
            (
                "old schema",
                Box::new(|m| {
                    m.insert("schema".into(), 2.into());
                }),
            ),
            (
                "other platform",
                Box::new(|m| {
                    m.insert("platform".into(), "plan9".into());
                }),
            ),
            (
                "helper outside staging",
                Box::new(move |m| {
                    m.insert("helper".into(), outside_helper.clone().into());
                }),
            ),
            (
                "wrong payload hash",
                Box::new(|m| {
                    m.insert("source_sha256".into(), "0".repeat(64).into());
                }),
            ),
        ];
        for (name, mutate) in cases {
            edit(&*mutate);
            assert!(validate_marker(&s.cache, Os::current(), Some(&trusted(&s))).is_err(), "{name}");
        }
        fs::write(marker_path(&s.cache), &original).unwrap();
        assert!(validate_marker(&s.cache, Os::current(), Some(&"f".repeat(64))).is_err(), "foreign helper");
        fs::write(&marker.source, b"swapped payload").unwrap();
        assert!(validate_marker(&s.cache, Os::current(), Some(&trusted(&s))).is_err(), "changed payload");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_staged_files_are_rejected() {
        let s = setup();
        let marker =
            prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha).unwrap();
        let real = s.cache.join("real-payload");
        fs::rename(&marker.source, &real).unwrap();
        std::os::unix::fs::symlink(&real, &marker.source).unwrap();
        assert!(validate_marker(&s.cache, Os::current(), Some(&trusted(&s))).is_err());
    }

    #[test]
    fn update_target_per_platform() {
        let exe = PathBuf::from("/opt/app/Launcher");
        let linux = ExecContext { os: Os::Linux, current_exe: exe.clone(), appimage: None, pid: 1 };
        assert_eq!(update_target(&linux).unwrap(), exe);
        let appimage =
            ExecContext { appimage: Some(PathBuf::from("/home/p/Launcher.AppImage")), ..linux.clone() };
        assert_eq!(update_target(&appimage).unwrap(), PathBuf::from("/home/p/Launcher.AppImage"));
        assert_eq!(appimage.trusted_binary(), Path::new("/home/p/Launcher.AppImage"));
        let mac = ExecContext {
            os: Os::MacOs,
            current_exe: PathBuf::from("/Applications/Launcher.app/Contents/MacOS/Launcher"),
            appimage: None,
            pid: 1,
        };
        assert_eq!(update_target(&mac).unwrap(), PathBuf::from("/Applications/Launcher.app"));
        let loose_mac = ExecContext { current_exe: PathBuf::from("/tmp/Launcher"), ..mac };
        assert_eq!(update_target(&loose_mac).unwrap_err().code, ErrorCode::Unsupported);
    }

    #[test]
    fn resume_discards_an_invalid_marker() {
        let s = setup();
        let marker =
            prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha).unwrap();
        fs::write(&marker.source, b"tampered").unwrap();
        assert!(matches!(resume_pending(&s.cache, &s.ctx), ResumeOutcome::Discarded(_)));
        assert!(!marker_path(&s.cache).exists());
        assert!(!staging_dir(&s.cache).exists());
        assert_eq!(resume_pending(&s.cache, &s.ctx), ResumeOutcome::Nothing);
    }

    #[test]
    fn cleanup_keeps_newer_downloads() {
        let s = setup();
        for v in ["0.1.0", "0.2.0", "0.3.0"] {
            let p = download_path(&s.cache, v, "Launcher.exe");
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, v).unwrap();
        }
        fs::create_dir_all(staging_dir(&s.cache)).unwrap();
        let current = Version::parse("0.2.0").unwrap();
        fs::write(marker_path(&s.cache), b"{}").unwrap();
        cleanup_after_update(&s.cache, &current);
        assert!(staging_dir(&s.cache).exists(), "nothing is touched while an update is pending");
        fs::remove_file(marker_path(&s.cache)).unwrap();
        cleanup_after_update(&s.cache, &current);
        assert!(!staging_dir(&s.cache).exists());
        assert!(!download_path(&s.cache, "0.1.0", "Launcher.exe").exists());
        assert!(!download_path(&s.cache, "0.2.0", "Launcher.exe").exists());
        assert!(download_path(&s.cache, "0.3.0", "Launcher.exe").exists());
    }

    #[test]
    fn download_path_is_per_version_and_sanitised() {
        let cache = Path::new("cache");
        let p = download_path(cache, "0.2.0", "../evil.exe");
        assert_eq!(p, cache.join(PENDING_DIR).join(DOWNLOADS_DIR).join("0.2.0").join("_evil.exe"));
    }

    #[test]
    fn appimage_is_trusted_only_inside_its_own_mount() {
        let root = std::env::temp_dir();
        let image = root.join("Other.AppImage").into_os_string();
        let mount = root.join(".mount_Other").into_os_string();
        let plain = root.join("bin").join("standard");
        let inner = root.join(".mount_Other").join("usr").join("bin").join("standard");
        assert_eq!(appimage_from(Some(image.clone()), None, &plain), None);
        assert_eq!(appimage_from(Some(image.clone()), Some(mount.clone()), &plain), None);
        assert_eq!(appimage_from(Some(image.clone()), Some(mount), &inner), Some(PathBuf::from(&image)));
        assert_eq!(appimage_from(None, None, &inner), None);
    }

    #[cfg(unix)]
    #[test]
    fn staging_open_to_other_users_is_rejected() {
        let s = setup();
        prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha).unwrap();
        set_mode(&staging_dir(&s.cache), 0o755).unwrap();
        assert!(validate_marker(&s.cache, s.ctx.os, Some(&trusted(&s))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_install_folder_is_refused_before_staging() {
        let s = setup();
        let dir = update_target(&s.ctx).unwrap().parent().unwrap().to_path_buf();
        set_mode(&dir, 0o555).unwrap();
        let result = prepare(&s.cache, &s.ctx, &PathEnv::default(), &s.payload, "0.2.0", &s.payload_sha);
        set_mode(&dir, 0o755).unwrap();
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidDirectoryPath);
        assert!(!marker_path(&s.cache).exists());
    }
}
