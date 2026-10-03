//! Mojang's Java runtimes under `<minecraft>/runtime`, in the layout MLL and the original use:
//! `runtime/<component>/<platform>/{.version, <component>.sha1, <component>/…}`. When the runtime
//! in use cannot be replaced (a running game holds its files), the new one goes to
//! `runtime/.generations/<component>/<uuid>/runtime/…` and `runtime/.active/<component>.txt`
//! names it.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use launcher_shared::{AppError, AppResult, ErrorCode, Text};
use serde::{Deserialize, Serialize};

use crate::builds::ids::validate_component_id;
use crate::lock::Lease;
use crate::minecraft::manifest::MojangEndpoints;
use crate::minecraft::platform::GamePlatform;
use crate::minecraft::{InstallProgress, InstallProgressFn};
use crate::net::downloader::{DownloadTask, Downloader, ExpectedHash, HashKind, hash_file};
use crate::net::meta::MetaClient;
use crate::safe_path::safe_relative;
use crate::storage::atomic::{atomic_write, atomic_write_text};

/// The runtime for versions whose JSON names none.
pub const LEGACY_COMPONENT: &str = "jre-legacy";
const VERIFIED_FILE: &str = ".launcher-verified.json";
const UNAVAILABLE_MARKER: &str = ".launcher-unavailable";
const ISOLATED_ATTEMPTS: u32 = 2;

#[derive(Deserialize)]
struct RemoteFile {
    url: String,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct RuntimeVersion {
    name: String,
}

#[derive(Deserialize)]
struct RuntimeEntry {
    manifest: RemoteFile,
    version: RuntimeVersion,
}

#[derive(Deserialize)]
struct FileDownloads {
    raw: RemoteFile,
    #[serde(default)]
    lzma: Option<RemoteFile>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RuntimeFile {
    Directory,
    File {
        #[serde(default)]
        executable: bool,
        downloads: FileDownloads,
    },
    Link {
        target: String,
    },
}

#[derive(Deserialize)]
struct RuntimeManifest {
    files: BTreeMap<String, RuntimeFile>,
}

/// Size and modification time of each file when it last hashed right: launches compare these
/// instead of hashing the runtime again.
#[derive(Default, Serialize, Deserialize)]
struct Verified {
    files: BTreeMap<String, (u64, u64)>,
}

/// Work left after the downloads.
#[derive(Default)]
struct AfterDownload {
    /// (packed file, destination, raw SHA-1)
    unpack: Vec<(PathBuf, PathBuf, String)>,
    executables: Vec<PathBuf>,
    links: Vec<(PathBuf, String)>,
    /// (manifest path, raw SHA-1) for `<component>.sha1`.
    records: Vec<(String, String)>,
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::Io, format!("{}: {e}", path.display()))
}

fn failed(component: &str, why: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::JavaRuntimeFailed, format!("Java {component}: {why}"))
        .with_param("component", component)
        .with_param("error", why.to_string())
}

/// A file some program holds, or one we may not write: a separate folder helps.
fn is_lock_error(error: &AppError) -> bool {
    let text = error.detail.to_lowercase();
    [
        "os error 5)",
        "os error 13)",
        "os error 16)",
        "os error 32)",
        "permission denied",
        "access is denied",
        "being used by another process",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn stamp(meta: &fs::Metadata) -> (u64, u64) {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    (meta.len(), modified)
}

/// Creation time in nanoseconds, as MLL writes it into `<component>.sha1` (`st_ctime_ns`).
fn ctime_ns(meta: &fs::Metadata) -> u128 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.ctime().max(0) as u128 * 1_000_000_000 + meta.ctime_nsec().max(0) as u128
    }
    #[cfg(not(unix))]
    {
        meta.created().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
    }
}

/// `<path> /#// <sha1> <ctime>`; a missing or malformed hash reads as `None`.
fn parse_record(line: &str) -> Option<(String, Option<String>)> {
    let (relative, rest) = line.trim().split_once(" /#// ")?;
    let sha1 = rest
        .split_whitespace()
        .next()
        .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase);
    Some((relative.trim().to_string(), sha1))
}

/// Every file `<component>.sha1` lists is present and, unless its size and modification time
/// match the last good check (always, when `deep`), hashes right.
fn complete_at(dir: &Path, component: &str, deep: bool) -> bool {
    let Ok(text) = fs::read_to_string(dir.join(format!("{component}.sha1"))) else { return false };
    let records: Vec<(String, Option<String>)> = text.lines().filter_map(parse_record).collect();
    if records.is_empty() {
        return false;
    }
    let home = dir.join(component);
    let cache_path = dir.join(VERIFIED_FILE);
    let mut verified: Verified =
        fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut changed = false;
    for (relative, sha1) in records {
        let Some(path) = safe_relative(&relative).map(|r| home.join(r)) else { return false };
        let Ok(meta) = fs::metadata(&path) else { return false };
        if !meta.is_file() {
            return false;
        }
        let Some(expected) = sha1 else { continue };
        let now = stamp(&meta);
        if !deep && verified.files.get(&relative) == Some(&now) {
            continue;
        }
        if hash_file(&path, HashKind::Sha1).ok().as_deref() != Some(expected.as_str()) {
            tracing::warn!("Java runtime file is damaged: {}", path.display());
            return false;
        }
        verified.files.insert(relative, now);
        changed = true;
    }
    if changed && let Ok(bytes) = serde_json::to_vec(&verified) {
        let _ = atomic_write(&cache_path, &bytes);
    }
    true
}

fn remote_task(file: &RemoteFile, dest: PathBuf) -> DownloadTask {
    let mut task = DownloadTask::new(file.url.as_str(), dest);
    if let Some(size) = file.size {
        task = task.size(size);
    }
    if let Some(sha1) = &file.sha1 {
        task = task.hash(ExpectedHash::sha1(sha1));
    }
    task
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}{suffix}"))
}

/// Already unpacked with the raw size (and, when verifying, the raw hash).
fn unpacked(path: &Path, raw: &RemoteFile, verify: bool) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if !meta.is_file() || raw.size.is_some_and(|size| size != meta.len()) {
        return false;
    }
    !verify
        || raw.sha1.as_deref().is_none_or(|expected| {
            hash_file(path, HashKind::Sha1).is_ok_and(|actual| actual.eq_ignore_ascii_case(expected))
        })
}

/// A link at `link` (relative to the runtime folder) whose `target`, resolved lexically, stays
/// inside the runtime folder.
fn link_stays_inside(link: &str, target: &str) -> bool {
    if target.starts_with(['/', '\\']) || target.contains(':') {
        return false;
    }
    let mut parts: Vec<&str> = link.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts.pop();
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return false;
                }
            }
            other => parts.push(other),
        }
    }
    true
}

/// The downloads to run and the work after them; creates the directories the manifest lists.
fn plan_files(
    manifest: &RuntimeManifest,
    home: &Path,
    verify: bool,
) -> Result<(Vec<DownloadTask>, AfterDownload), String> {
    let mut tasks = Vec::new();
    let mut after = AfterDownload::default();
    for (relative, file) in &manifest.files {
        let path = safe_relative(relative)
            .map(|r| home.join(r))
            .ok_or_else(|| format!("unsafe path {relative:?}"))?;
        match file {
            RuntimeFile::Directory => {
                fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?
            }
            RuntimeFile::Link { target } => {
                if !link_stays_inside(relative, target) {
                    return Err(format!("unsafe link {relative:?} -> {target:?}"));
                }
                after.links.push((path, target.clone()));
            }
            RuntimeFile::File { executable, downloads } => {
                let sha1 = downloads.raw.sha1.clone().unwrap_or_default().to_ascii_lowercase();
                after.records.push((relative.clone(), sha1.clone()));
                if *executable {
                    after.executables.push(path.clone());
                }
                match &downloads.lzma {
                    Some(_) if unpacked(&path, &downloads.raw, verify) => {}
                    Some(lzma) => {
                        let packed = sibling(&path, ".launcher.lzma");
                        tasks.push(remote_task(lzma, packed.clone()));
                        after.unpack.push((packed, path, sha1));
                    }
                    None => tasks.push(remote_task(&downloads.raw, path)),
                }
            }
        }
    }
    Ok((tasks, after))
}

/// LZMA → a temp file → raw SHA-1 check → the destination.
fn unpack(packed: &Path, dest: &Path, sha1: &str) -> io::Result<()> {
    let temp = sibling(dest, ".launcher.tmp");
    {
        let mut input = BufReader::new(File::open(packed)?);
        let mut output = BufWriter::new(File::create(&temp)?);
        lzma_rs::lzma_decompress(&mut input, &mut output)
            .map_err(|e| io::Error::other(format!("{}: {e:?}", packed.display())))?;
        output.flush()?;
    }
    let actual = hash_file(&temp, HashKind::Sha1)?;
    if !sha1.is_empty() && actual != sha1 {
        let _ = fs::remove_file(&temp);
        return Err(io::Error::other(format!("{}: sha1 {actual}, expected {sha1}", dest.display())));
    }
    if let Err(e) = fs::rename(&temp, dest) {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    let _ = fs::remove_file(packed);
    Ok(())
}

#[cfg(unix)]
fn set_executable(paths: &[PathBuf]) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for path in paths {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_paths: &[PathBuf]) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_links(links: &[(PathBuf, String)]) -> io::Result<()> {
    for (path, target) in links {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::os::unix::fs::symlink(target, path)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_links(_links: &[(PathBuf, String)]) -> io::Result<()> {
    // Mojang's Windows runtimes have no links, and creating one needs extra rights (MLL skips too).
    Ok(())
}

/// `.version`, `<component>.sha1` (MLL format) and the verification cache.
fn write_records(dir: &Path, component: &str, version: &str, records: &[(String, String)]) -> io::Result<()> {
    let home = dir.join(component);
    let mut lines = String::new();
    let mut verified = Verified::default();
    for (relative, sha1) in records {
        let Some(path) = safe_relative(relative).map(|r| home.join(r)) else { continue };
        let meta = fs::metadata(&path)?;
        lines.push_str(&format!("{relative} /#// {sha1} {}\n", ctime_ns(&meta)));
        verified.files.insert(relative.clone(), stamp(&meta));
    }
    atomic_write_text(&dir.join(".version"), version)?;
    atomic_write_text(&dir.join(format!("{component}.sha1")), &lines)?;
    atomic_write(&dir.join(VERIFIED_FILE), &serde_json::to_vec(&verified).map_err(io::Error::other)?)
}

/// Every packed file unpacked, a few side by side (each decodes on one core): a first runtime has
/// hundreds of them. The first failure is the answer; the others stop taking files.
fn unpack_all(files: &[(PathBuf, PathBuf, String)]) -> io::Result<()> {
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8).min(files.len());
    let (next, failed) = (AtomicUsize::new(0), AtomicBool::new(false));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> io::Result<()> {
                    while !failed.load(Ordering::SeqCst) {
                        let Some((packed, dest, sha1)) = files.get(next.fetch_add(1, Ordering::SeqCst))
                        else {
                            break;
                        };
                        if let Err(e) = unpack(packed, dest, sha1) {
                            failed.store(true, Ordering::SeqCst);
                            return Err(e);
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("unpacking a Java file panicked"))))
            .collect::<io::Result<Vec<()>>>()
            .map(|_| ())
    })
}

fn finish(after: &AfterDownload, dir: &Path, component: &str, version: &str) -> io::Result<()> {
    unpack_all(&after.unpack)?;
    set_executable(&after.executables)?;
    make_links(&after.links)?;
    write_records(dir, component, version, &after.records)
}

pub struct JavaRuntimes {
    mc_dir: PathBuf,
    platform: GamePlatform,
    endpoints: MojangEndpoints,
    meta: Arc<MetaClient>,
    downloader: Arc<Downloader>,
}

impl JavaRuntimes {
    pub fn new(
        mc_dir: &Path,
        platform: GamePlatform,
        endpoints: MojangEndpoints,
        meta: Arc<MetaClient>,
        downloader: Arc<Downloader>,
    ) -> JavaRuntimes {
        JavaRuntimes { mc_dir: mc_dir.to_path_buf(), platform, endpoints, meta, downloader }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.mc_dir.join("runtime")
    }

    /// `<root>/runtime/<component>/<platform key>`; `None` where Mojang ships no runtime.
    fn platform_dir(&self, root: &Path, component: &str) -> Option<PathBuf> {
        Some(root.join("runtime").join(component).join(self.platform.java_runtime_key()?))
    }

    fn generations_dir(&self, component: &str) -> PathBuf {
        self.runtime_dir().join(".generations").join(component)
    }

    fn active_marker(&self, component: &str) -> PathBuf {
        self.runtime_dir().join(".active").join(format!("{component}.txt"))
    }

    /// The isolated generation `.active/<component>.txt` names, if it is a folder inside
    /// `.generations/<component>`.
    fn active_generation(&self, component: &str) -> Option<PathBuf> {
        let text = fs::read_to_string(self.active_marker(component)).ok()?;
        let root = self.mc_dir.join(safe_relative(text.trim())?);
        (root.starts_with(self.generations_dir(component)) && root.is_dir()).then_some(root)
    }

    /// Where the runtime in use lives: the active generation, else the Minecraft folder.
    fn install_root(&self, component: &str) -> PathBuf {
        self.active_generation(component).unwrap_or_else(|| self.mc_dir.clone())
    }

    fn executable_in(&self, root: &Path, component: &str) -> Option<PathBuf> {
        let home = self.platform_dir(root, component)?.join(component);
        let bin = home.join("bin").join(self.platform.java_binary());
        let bundle = home.join("jre.bundle").join("Contents").join("Home").join("bin").join("java");
        [bin, bundle].into_iter().find(|path| path.is_file())
    }

    /// The runtime's `java`, if it is installed.
    pub fn executable(&self, component: &str) -> Option<PathBuf> {
        validate_component_id(component).ok()?;
        self.executable_in(&self.install_root(component), component)
    }

    /// Mojang has no build of `component` here: no runtimes for this platform at all, or an empty
    /// list, remembered by a marker so that launches do not ask again.
    pub fn unavailable(&self, component: &str) -> bool {
        match self.platform_dir(&self.mc_dir, component) {
            None => true,
            Some(dir) => dir.join(UNAVAILABLE_MARKER).is_file(),
        }
    }

    /// Every file of the runtime in use is present; `deep` hashes all of them, otherwise only the
    /// files whose size or modification time changed. Blocking.
    pub fn is_complete(&self, component: &str, deep: bool) -> bool {
        validate_component_id(component).is_ok()
            && self.executable(component).is_some()
            && self
                .platform_dir(&self.install_root(component), component)
                .is_some_and(|dir| complete_at(&dir, component, deep))
    }

    async fn complete(&self, component: &str, deep: bool) -> bool {
        let Some(dir) = self.platform_dir(&self.install_root(component), component) else { return false };
        let component = component.to_string();
        tokio::task::spawn_blocking(move || complete_at(&dir, &component, deep)).await.unwrap_or(false)
    }

    /// The runtime's `java`, installed or repaired when it is missing or incomplete; `None` where
    /// Mojang has no build of it. The caller holds the shared Minecraft lease.
    pub async fn ensure(
        &self,
        component: &str,
        _lease: &Lease,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Option<PathBuf>> {
        validate_component_id(component)?;
        if self.unavailable(component) {
            return Ok(None);
        }
        if let Some(java) = self.executable(component)
            && self.complete(component, false).await
        {
            return Ok(Some(java));
        }
        self.reinstall(component, progress).await
    }

    /// `ensure` after hashing every file; a runtime remembered as unavailable is asked about
    /// again (repairing installs, Components → repair).
    pub async fn repair(
        &self,
        component: &str,
        _lease: &Lease,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Option<PathBuf>> {
        validate_component_id(component)?;
        let Some(dir) = self.platform_dir(&self.mc_dir, component) else { return Ok(None) };
        let _ = fs::remove_file(dir.join(UNAVAILABLE_MARKER));
        if let Some(java) = self.executable(component)
            && self.complete(component, true).await
        {
            return Ok(Some(java));
        }
        self.reinstall(component, progress).await
    }

    /// Repairs over the files present; only damage found on disk warrants starting from scratch
    /// (a network failure keeps what is there). A runtime locked by a running game goes to a new
    /// isolated generation instead.
    async fn reinstall(
        &self,
        component: &str,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Option<PathBuf>> {
        let result = match self.install_at(&self.mc_dir, component, true, progress).await {
            Err(e)
                if !is_lock_error(&e)
                    && !matches!(e.code, ErrorCode::Network | ErrorCode::DownloadFailed) =>
            {
                tracing::warn!("Java {component} repair failed, installing it from scratch: {}", e.detail);
                let _ = fs::remove_dir_all(self.runtime_dir().join(component));
                self.install_at(&self.mc_dir, component, false, progress).await
            }
            other => other,
        };
        match result {
            Ok(java) => {
                self.forget_generations(component, None);
                Ok(java)
            }
            Err(e) if is_lock_error(&e) => {
                tracing::warn!("Java {component} is in use, installing a new generation: {}", e.detail);
                self.install_isolated(component, progress).await
            }
            Err(e) => Err(e),
        }
    }

    async fn install_isolated(
        &self,
        component: &str,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Option<PathBuf>> {
        let mut last = None;
        for _ in 0..ISOLATED_ATTEMPTS {
            let generation = self.generations_dir(component).join(uuid::Uuid::new_v4().simple().to_string());
            match self.install_at(&generation, component, false, progress).await {
                Ok(Some(java)) => {
                    self.activate(component, &generation)?;
                    self.forget_generations(component, Some(&generation));
                    return Ok(Some(java));
                }
                Ok(None) => return Ok(None),
                Err(e) => {
                    let _ = fs::remove_dir_all(&generation);
                    last = Some(e);
                }
            }
        }
        Err(last.expect("an attempt ran"))
    }

    fn activate(&self, component: &str, generation: &Path) -> AppResult<()> {
        let marker = self.active_marker(component);
        let relative = generation
            .strip_prefix(&self.mc_dir)
            .map_err(|_| AppError::internal("the generation is outside the Minecraft folder"))?;
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        atomic_write_text(&marker, &relative.to_string_lossy().replace('\\', "/"))
            .map_err(|e| io_error(&marker, e))
    }

    /// Drops the active marker (unless keeping `keep`) and every other generation; folders a
    /// running game still holds stay until next time.
    fn forget_generations(&self, component: &str, keep: Option<&Path>) {
        if keep.is_none() {
            let _ = fs::remove_file(self.active_marker(component));
        }
        let Ok(entries) = fs::read_dir(self.generations_dir(component)) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if Some(path.as_path()) != keep {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }

    /// Downloads `component` into `root` as MLL's `install_jvm_runtime` does; `None` (and the
    /// unavailable marker) when Mojang lists no build of it for this platform.
    async fn install_at(
        &self,
        root: &Path,
        component: &str,
        verify: bool,
        progress: InstallProgressFn<'_>,
    ) -> AppResult<Option<PathBuf>> {
        let (Some(key), Some(dir)) = (self.platform.java_runtime_key(), self.platform_dir(root, component))
        else {
            return Ok(None);
        };
        let status = Text::key("installing_java_runtime").param("component", component);
        progress(InstallProgress::status(status.clone()));
        let list = self.meta.get_json(&self.endpoints.java_runtimes).await?;
        let Some(entries) = list.get(key).and_then(|platform| platform.get(component)) else {
            return Err(failed(component, format!("Mojang lists no runtime {component}")));
        };
        let entries: Vec<RuntimeEntry> =
            serde_json::from_value(entries.clone()).map_err(|e| failed(component, e))?;
        let Some(entry) = entries.into_iter().next() else {
            tracing::warn!("Mojang has no build of Java {component} for {key}");
            if let Some(main) = self.platform_dir(&self.mc_dir, component) {
                fs::create_dir_all(&main).map_err(|e| io_error(&main, e))?;
                fs::write(main.join(UNAVAILABLE_MARKER), b"").map_err(|e| io_error(&main, e))?;
            }
            return Ok(None);
        };
        let bytes = self.meta.get_verified(&entry.manifest.url, entry.manifest.sha1.as_deref()).await?;
        let manifest: RuntimeManifest = serde_json::from_slice(&bytes).map_err(|e| failed(component, e))?;
        let home = dir.join(component);
        let (tasks, after) = tokio::task::spawn_blocking(move || plan_files(&manifest, &home, verify))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
            .map_err(|e| failed(component, e))?;
        self.downloader
            .download_all(tasks, verify, &|p| progress(InstallProgress::download(status.clone(), p)))
            .await?
            .into_result()?;
        let (target, name, version) = (dir.clone(), component.to_string(), entry.version.name);
        tokio::task::spawn_blocking(move || finish(&after, &target, &name, &version))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?
            .map_err(|e| failed(component, e))?;
        match self.executable_in(root, component) {
            Some(java) => Ok(Some(java)),
            None => Err(failed(component, "the runtime has no java executable")),
        }
    }
}

#[cfg(test)]
mod unpack_tests {
    use super::*;

    fn packed(dir: &Path, name: &str, bytes: &[u8]) -> (PathBuf, PathBuf, String) {
        let packed = dir.join(format!("{name}.launcher.lzma"));
        let mut out = Vec::new();
        lzma_rs::lzma_compress(&mut &bytes[..], &mut out).unwrap();
        fs::write(&packed, out).unwrap();
        let sha1 = {
            let raw = dir.join(format!("{name}.raw"));
            fs::write(&raw, bytes).unwrap();
            let sum = hash_file(&raw, HashKind::Sha1).unwrap();
            fs::remove_file(raw).unwrap();
            sum
        };
        (packed, dir.join(name), sha1)
    }

    #[test]
    fn a_runtime_s_files_are_unpacked_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<_> = (0..24)
            .map(|i| packed(dir.path(), &format!("f{i}"), format!("file {i}").repeat(500).as_bytes()))
            .collect();
        unpack_all(&files).unwrap();
        for (i, (packed, dest, _)) in files.iter().enumerate() {
            assert_eq!(fs::read(dest).unwrap(), format!("file {i}").repeat(500).into_bytes());
            assert!(!packed.exists(), "the packed file goes once unpacked");
        }
        let mut bad = vec![packed(dir.path(), "ok", b"fine"), packed(dir.path(), "bad", b"damaged")];
        bad[1].2 = "0".repeat(40);
        assert!(unpack_all(&bad).is_err(), "a wrong SHA-1 fails the whole unpacking");
        assert!(!dir.path().join("bad").exists());
        assert!(unpack_all(&[]).is_ok());
    }
}
