//! One downloader for game files: bounded concurrency, retries, partial files bound to the exact
//! request (resumed only with a trustworthy `Range` answer), size and hash checks, storage
//! preflight before the first byte.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use launcher_shared::branding::{APP_NAME, VERSION};
use launcher_shared::{AppError, AppResult, ErrorCode};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use super::preflight::{SpaceRequest, preflight};
use super::{HTTPS_ONLY, redirect_policy, redirect_policy_within, url_allowed};
use crate::lock::path_key;
use crate::storage::atomic::rename_retrying;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Sha1,
    Sha256,
    Sha512,
}

impl HashKind {
    fn name(self) -> &'static str {
        match self {
            HashKind::Sha1 => "sha1",
            HashKind::Sha256 => "sha256",
            HashKind::Sha512 => "sha512",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedHash {
    pub kind: HashKind,
    /// Lower-case hex.
    pub hex: String,
}

impl ExpectedHash {
    pub fn sha1(hex: &str) -> Self {
        ExpectedHash { kind: HashKind::Sha1, hex: hex.to_ascii_lowercase() }
    }

    pub fn sha256(hex: &str) -> Self {
        ExpectedHash { kind: HashKind::Sha256, hex: hex.to_ascii_lowercase() }
    }

    pub fn sha512(hex: &str) -> Self {
        ExpectedHash { kind: HashKind::Sha512, hex: hex.to_ascii_lowercase() }
    }
}

/// Where a download's bytes may come from: checked on the address it ends at, after any
/// redirects, before a byte of its body is read.
#[derive(Clone)]
pub struct Origin(Arc<dyn Fn(&Url) -> bool + Send + Sync>);

impl Origin {
    pub fn new(allows: impl Fn(&Url) -> bool + Send + Sync + 'static) -> Origin {
        Origin(Arc::new(allows))
    }

    fn allows(&self, url: &Url) -> bool {
        (self.0)(url)
    }
}

impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Origin(..)")
    }
}

/// The same rule, not merely an equal one.
impl PartialEq for Origin {
    fn eq(&self, other: &Origin) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Origin {}

/// A secret header some hosts need (an API key): sent only to an address `hosts` accepts, with a
/// client of its own whose redirects never leave those hosts. Mark `value` sensitive.
#[derive(Clone)]
pub struct Credential(Arc<CredentialInner>);

struct CredentialInner {
    name: HeaderName,
    value: HeaderValue,
    hosts: Arc<dyn Fn(&Url) -> bool + Send + Sync>,
    /// Built at the first use, with the downloader's settings; `None` when it could not be.
    client: OnceLock<Option<Client>>,
}

impl Credential {
    pub fn new(
        name: HeaderName,
        value: HeaderValue,
        hosts: impl Fn(&Url) -> bool + Send + Sync + 'static,
    ) -> Self {
        Credential(Arc::new(CredentialInner { name, value, hosts: Arc::new(hosts), client: OnceLock::new() }))
    }

    fn covers(&self, url: &Url) -> bool {
        (self.0.hosts)(url)
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({})", self.0.name)
    }
}

/// The same credential, not merely an equal one.
impl PartialEq for Credential {
    fn eq(&self, other: &Credential) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Credential {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub hash: Option<ExpectedHash>,
    /// Where the bytes may come from; any host the downloader allows when `None`.
    pub origin: Option<Origin>,
    /// A secret header for the hosts it names.
    pub credential: Option<Credential>,
}

impl DownloadTask {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        DownloadTask {
            url: url.into(),
            dest: dest.into(),
            size: None,
            hash: None,
            origin: None,
            credential: None,
        }
    }

    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = Some(credential);
        self
    }

    pub fn origin(mut self, origin: Origin) -> Self {
        self.origin = Some(origin);
        self
    }

    /// The download ended at an address its origin rule refuses.
    fn strayed(&self, url: &Url) -> Option<Failure> {
        let origin = self.origin.as_ref()?;
        (!origin.allows(url)).then(|| {
            Failure::Fatal(format!(
                "the download came from {}, which is not allowed",
                url.host_str().unwrap_or("?")
            ))
        })
    }

    pub fn size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn hash(mut self, hash: ExpectedHash) -> Self {
        self.hash = Some(hash);
        self
    }

    fn name(&self) -> String {
        self.dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.url.clone())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    /// Sum of the known sizes of the files being downloaded.
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadReport {
    pub downloaded: usize,
    pub skipped: usize,
    /// `"<file name>: <reason>"`.
    pub failed: Vec<String>,
    /// How many of `failed` ended on a wrong digest.
    pub mismatched: usize,
}

/// Why the files left were not even tried: too many connections failed in a row.
pub const NETWORK_DOWN: &str = "the network is unreachable";
/// How many failures in a row (no connection, no answer) mean the network is down.
const OUTAGE_AFTER: usize = 6;
/// How many failures an error names; the rest are counted.
const NAMED_FAILURES: usize = 5;
/// The longest a server's `Retry-After` is waited for.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

impl DownloadReport {
    /// `DownloadFailed` naming the first failures when anything failed; `Network` (with the param
    /// `network` = `down`) when the network went down, which another attempt soon will not mend.
    pub fn into_result(self) -> AppResult<DownloadReport> {
        let Some(first) = self.failed.first() else { return Ok(self) };
        let down = self.failed.iter().any(|f| f.ends_with(NETWORK_DOWN));
        let mut named = self.failed.iter().take(NAMED_FAILURES).cloned().collect::<Vec<_>>().join("; ");
        if self.failed.len() > NAMED_FAILURES {
            named.push_str(&format!("; and {} more", self.failed.len() - NAMED_FAILURES));
        }
        let detail = format!("{} file(s) failed: {named}", self.failed.len());
        let error = if down {
            AppError::new(ErrorCode::Network, detail).with_param("network", "down")
        } else {
            AppError::new(ErrorCode::DownloadFailed, detail)
        };
        Err(error.with_param("error", first.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct DownloaderConfig {
    pub workers: usize,
    pub retries: u32,
    /// Pause before retry `n` is `retry_delay × 4ⁿ⁻¹` (or what the server asks for).
    pub retry_delay: Duration,
    pub timeout: Duration,
    /// Limit for files of unknown size.
    pub max_size: u64,
    /// Files of a known size up to this are fetched whole into memory and written in one go:
    /// no partial file, no sidecar, no resume (a dropped connection starts them over).
    pub in_memory_up_to: u64,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        DownloaderConfig {
            workers: 16,
            retries: 3,
            retry_delay: Duration::from_millis(250),
            timeout: Duration::from_secs(30),
            max_size: 8 * 1024 * 1024 * 1024,
            in_memory_up_to: 4 * 1024 * 1024,
        }
    }
}

type ProgressFn<'a> = &'a (dyn Fn(DownloadProgress) + Send + Sync);

/// Why one attempt failed: `Retry`, `Unreachable` (no connection or no answer: counted towards
/// the network being down), `Wait` (the server says when) and `Mismatch` (a wrong digest) are worth
/// another attempt, `Fatal` is not.
enum Failure {
    Retry(String),
    Unreachable(String),
    Wait(String, Duration),
    Mismatch(String),
    Fatal(String),
}

/// How a file failed after all its attempts: the last reason, and whether it was a wrong digest.
struct Failed {
    reason: String,
    mismatch: bool,
}

fn fatal(e: impl std::fmt::Display) -> Failure {
    Failure::Fatal(e.to_string())
}

fn send_failure(e: reqwest::Error) -> Failure {
    if e.is_redirect() {
        Failure::Fatal(error_chain(&e))
    } else if e.is_connect() || e.is_timeout() {
        Failure::Unreachable(e.to_string())
    } else {
        Failure::Retry(e.to_string())
    }
}

/// A client error (bar 408 and 429) is final; anything else is worth another attempt, after the
/// server's `Retry-After` (seconds) when it gives one.
fn status_failure(response: &reqwest::Response) -> Failure {
    let status = response.status();
    if status.is_client_error()
        && status != StatusCode::TOO_MANY_REQUESTS
        && status != StatusCode::REQUEST_TIMEOUT
    {
        return Failure::Fatal(format!("HTTP {status}"));
    }
    let after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs).min(MAX_RETRY_AFTER));
    match after {
        Some(after) => Failure::Wait(format!("HTTP {status}"), after),
        None => Failure::Retry(format!("HTTP {status}")),
    }
}

fn mismatch(expected: &ExpectedHash, actual: &str) -> String {
    format!("{} mismatch: expected {}, got {actual}", expected.kind.name(), expected.hex)
}

struct Counters {
    files_done: AtomicUsize,
    files_total: usize,
    bytes_done: AtomicU64,
    bytes_total: u64,
    /// Failures in a row that reached no one, across the batch.
    unreached: AtomicUsize,
}

impl Counters {
    fn snapshot(&self) -> DownloadProgress {
        DownloadProgress {
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total,
        }
    }

    /// The network is down: the last `OUTAGE_AFTER` attempts of the batch reached no one.
    fn network_down(&self) -> bool {
        self.unreached.load(Ordering::Relaxed) >= OUTAGE_AFTER
    }

    fn reached(&self) {
        self.unreached.store(0, Ordering::Relaxed);
    }

    fn unreached(&self) {
        self.unreached.fetch_add(1, Ordering::Relaxed);
    }

    /// Moves one file's contribution from `reported` to `now` bytes.
    fn set_file_bytes(&self, reported: &mut u64, now: u64) {
        if now >= *reported {
            self.bytes_done.fetch_add(now - *reported, Ordering::Relaxed);
        } else {
            self.bytes_done.fetch_sub(*reported - now, Ordering::Relaxed);
        }
        *reported = now;
    }
}

/// A running digest of one of the supported kinds.
enum Hasher {
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
}

impl Hasher {
    fn new(kind: HashKind) -> Hasher {
        match kind {
            HashKind::Sha1 => Hasher::Sha1(Sha1::new()),
            HashKind::Sha256 => Hasher::Sha256(Sha256::new()),
            HashKind::Sha512 => Hasher::Sha512(Sha512::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(bytes),
            Hasher::Sha256(h) => h.update(bytes),
            Hasher::Sha512(h) => h.update(bytes),
        }
    }

    fn finish(self) -> String {
        match self {
            Hasher::Sha1(h) => hex::encode(h.finalize()),
            Hasher::Sha256(h) => hex::encode(h.finalize()),
            Hasher::Sha512(h) => hex::encode(h.finalize()),
        }
    }
}

/// Lower-case hex digest of the file at `path`.
pub fn hash_file(path: &Path, kind: HashKind) -> io::Result<String> {
    hash_reader(&mut File::open(path)?, kind)
}

/// Lower-case hex digest of everything `reader` gives.
pub fn hash_reader(reader: &mut dyn Read, kind: HashKind) -> io::Result<String> {
    let mut hasher = Hasher::new(kind);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buf[..n]);
    }
}

/// Writes `body` beside `dest` and swaps it in, so `dest` never holds part of a file; the
/// folder is made only when it is missing.
fn write_whole(part: &Path, dest: &Path, body: &[u8]) -> io::Result<()> {
    let mut file = match File::create(part) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(part.parent().unwrap_or(Path::new(".")))?;
            File::create(part)?
        }
        other => other?,
    };
    let written = file.write_all(body).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| rename_retrying(part, dest)) {
        let _ = fs::remove_file(part);
        return Err(e);
    }
    Ok(())
}

/// Present with the right size (and, when asked, the right hash).
fn should_skip(task: &DownloadTask, verify: bool) -> bool {
    let Ok(meta) = fs::metadata(&task.dest) else { return false };
    if !meta.is_file() || task.size.is_some_and(|size| meta.len() != size) {
        return false;
    }
    match (&task.hash, verify) {
        (Some(hash), true) => hash_file(&task.dest, hash.kind).is_ok_and(|actual| actual == hash.hex),
        // Known by name alone (old Maven libraries): a jar or zip counts while it still opens.
        (None, _) if task.size.is_none() && is_archive(&task.dest) => opens_as_zip(&task.dest),
        _ => true,
    }
}

fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jar") || e.eq_ignore_ascii_case("zip"))
}

/// The file reads as a zip archive (its central directory is whole).
fn opens_as_zip(path: &Path) -> bool {
    File::open(path).is_ok_and(|file| zip::ZipArchive::new(io::BufReader::new(file)).is_ok())
}

/// How many of `tasks` are already in place, and the rest; the checks (and with `verify` the
/// hashing) run in batches on blocking threads, several batches at once.
async fn split_present(tasks: Vec<DownloadTask>, verify: bool) -> AppResult<(usize, Vec<DownloadTask>)> {
    const BATCH: usize = 64;
    let lanes = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut batches: Vec<Vec<DownloadTask>> = Vec::new();
    let mut rest = tasks.into_iter().peekable();
    while rest.peek().is_some() {
        batches.push(rest.by_ref().take(BATCH).collect());
    }
    let checked: Vec<Vec<(DownloadTask, bool)>> = futures_util::stream::iter(batches)
        .map(|batch| {
            tokio::task::spawn_blocking(move || {
                batch
                    .into_iter()
                    .map(|task| {
                        let present = should_skip(&task, verify);
                        (task, present)
                    })
                    .collect()
            })
        })
        .buffered(lanes)
        .map(|joined| joined.map_err(|e| AppError::internal(e.to_string())))
        .try_collect()
        .await?;
    let mut present = 0;
    let mut pending = Vec::new();
    for (task, skip) in checked.into_iter().flatten() {
        if skip {
            present += 1;
        } else {
            pending.push(task);
        }
    }
    Ok((present, pending))
}

/// Everything that defines "the same download"; a partial file belongs to exactly one identity.
fn identity(task: &DownloadTask) -> String {
    json!({
        "expected_hash": task.hash.as_ref().map(|h| h.hex.as_str()),
        "expected_hash_algorithm": task.hash.as_ref().map(|h| h.kind.name()),
        "expected_size": task.size,
        "method": "GET",
        "post_data": null,
        "url": task.url,
    })
    .to_string()
}

/// `error: cause: cause…` — reqwest keeps the interesting part (e.g. a refused redirect) in the
/// source chain.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// A resumed answer must come from the same version of the file as the partial one: its
/// validator header (`ETag` or `Last-Modified`) must equal the one the partial file was started
/// with, whatever the server made of `If-Range`.
fn same_version(headers: &HeaderMap, validator: &Option<(HeaderName, String)>) -> bool {
    match validator {
        None => true,
        Some((name, value)) => headers.get(name).and_then(|v| v.to_str().ok()) == Some(value.as_str()),
    }
}

/// `Content-Range: bytes S-E/T` must continue exactly at `offset` and end at the known total.
fn range_matches(headers: &HeaderMap, offset: u64, size: Option<u64>, limit: u64) -> bool {
    let text = |name: HeaderName| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let Some(range) = text(header::CONTENT_RANGE) else { return false };
    let Some((span, total)) = range.strip_prefix("bytes ").and_then(|r| r.split_once('/')) else {
        return false;
    };
    let Some((start, end)) = span.split_once('-') else { return false };
    let (Ok(start), Ok(end), Ok(total)) = (start.parse::<u64>(), end.parse::<u64>(), total.parse::<u64>())
    else {
        return false;
    };
    if start > end {
        return false;
    }
    let length_ok = text(header::CONTENT_LENGTH)
        .and_then(|v| v.parse::<u64>().ok())
        .is_none_or(|length| length == end - start + 1);
    start == offset && end + 1 == total && total <= limit && size.is_none_or(|s| s == total) && length_ok
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    schema: u32,
    identity: String,
    partial_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

/// `<name>.part.<sha256(identity)[..24]>.tmp` and its sidecar `<name>.part.json`; a small file is
/// written whole to `<name>.part.whole.tmp`.
#[derive(Clone)]
struct Partial {
    name: String,
    part: PathBuf,
    sidecar: PathBuf,
}

impl Partial {
    fn new(dest: &Path, identity: &str) -> Partial {
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
        let digest = hex::encode(Sha256::digest(identity.as_bytes()));
        Partial {
            part: dir.join(format!("{name}.part.{}.tmp", &digest[..24])),
            sidecar: dir.join(format!("{name}.part.json")),
            name,
        }
    }

    fn read_sidecar(&self) -> Option<Sidecar> {
        serde_json::from_slice(&fs::read(&self.sidecar).ok()?).ok()
    }

    /// Where an earlier attempt of this very request stopped, and the validator (header, value)
    /// it was started with.
    fn resumable(
        &self,
        identity: &str,
        size: Option<u64>,
        has_hash: bool,
    ) -> Option<(u64, Option<(HeaderName, String)>)> {
        let sidecar = self.read_sidecar()?;
        if sidecar.schema != 1 || sidecar.identity != identity {
            return None;
        }
        let len = fs::metadata(&self.part).ok()?.len();
        if len == 0 || size.is_some_and(|s| len >= s) {
            return None;
        }
        let validator = match (sidecar.etag, sidecar.last_modified) {
            (Some(etag), _) => Some((header::ETAG, etag)),
            (None, Some(date)) => Some((header::LAST_MODIFIED, date)),
            (None, None) => None,
        };
        // Without a validator only a known hash could catch a changed file.
        (validator.is_some() || has_hash).then_some((len, validator))
    }

    /// Where a small file is written whole: one name per destination, so a crash's leftover is
    /// written over by the next attempt whatever it asks for.
    fn whole(&self) -> PathBuf {
        self.sidecar.with_file_name(format!("{}.part.whole.tmp", self.name))
    }

    /// The partial file an earlier attempt's sidecar names, when it is safely one of ours.
    fn older(&self, old: &Sidecar) -> Option<PathBuf> {
        let safe = !old.partial_file.contains(['/', '\\'])
            && old.partial_file.starts_with(&format!("{}.part.", self.name))
            && old.partial_file.ends_with(".tmp");
        safe.then(|| self.sidecar.with_file_name(&old.partial_file))
    }

    /// An earlier streamed attempt is over for good (the file came whole): its partial file and
    /// sidecar go. Without a sidecar this is one failed open.
    fn forget_older(&self) {
        let Ok(bytes) = fs::read(&self.sidecar) else { return };
        if let Some(old_part) = serde_json::from_slice(&bytes).ok().and_then(|old| self.older(&old)) {
            let _ = fs::remove_file(old_part);
        }
        let _ = fs::remove_file(&self.sidecar);
    }

    /// A fresh download: drop an older partial file of this destination and record the new one.
    fn start(&self, identity: &str, headers: &HeaderMap) -> io::Result<()> {
        if let Some(old_part) = self.read_sidecar().and_then(|old| self.older(&old))
            && old_part != self.part
        {
            let _ = fs::remove_file(old_part);
        }
        let text = |name: HeaderName| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let sidecar = Sidecar {
            schema: 1,
            identity: identity.to_string(),
            partial_file: self.part.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            etag: text(header::ETAG).filter(|e| !e.starts_with("W/")),
            last_modified: text(header::LAST_MODIFIED),
        };
        fs::write(&self.sidecar, serde_json::to_vec(&sidecar).map_err(io::Error::other)?)
    }

    fn discard(&self) {
        let _ = fs::remove_file(&self.part);
        let _ = fs::remove_file(&self.sidecar);
    }
}

/// One task per destination: exact repeats (an asset index lists one object under many names)
/// count as skipped; different tasks for one file all fail, since either could be the wrong one.
fn unique_destinations(tasks: Vec<DownloadTask>, report: &mut DownloadReport) -> Vec<DownloadTask> {
    let mut groups: Vec<Vec<DownloadTask>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for task in tasks {
        let key = path_key(&task.dest);
        match index.get(&key) {
            Some(&i) => groups[i].push(task),
            None => {
                index.insert(key, groups.len());
                groups.push(vec![task]);
            }
        }
    }
    let mut unique = Vec::with_capacity(groups.len());
    for mut group in groups {
        if group.iter().all(|t| *t == group[0]) {
            report.skipped += group.len() - 1;
            group.truncate(1);
            unique.extend(group);
        } else {
            for task in group {
                report.failed.push(format!("{}: conflicting downloads for the same file", task.name()));
            }
        }
    }
    unique
}

pub struct Downloader {
    client: Client,
    cfg: DownloaderConfig,
}

/// A client with the downloader's settings and `redirects`.
fn client_with(cfg: &DownloaderConfig, redirects: reqwest::redirect::Policy) -> reqwest::Result<Client> {
    Client::builder()
        .user_agent(format!("{APP_NAME}/{VERSION}"))
        .connect_timeout(cfg.timeout)
        .read_timeout(cfg.timeout)
        .redirect(redirects)
        .build()
}

impl Downloader {
    pub fn new(cfg: DownloaderConfig) -> AppResult<Downloader> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = client_with(&cfg, redirect_policy()).map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Downloader { client, cfg })
    }

    /// A GET of `url` for `task`: with its credential's header and client when the credential
    /// covers `url`, else plain.
    fn get(&self, task: &DownloadTask, url: Url) -> reqwest::RequestBuilder {
        if let Some(credential) = task.credential.as_ref().filter(|c| c.covers(&url)) {
            let client = credential.0.client.get_or_init(|| {
                client_with(&self.cfg, redirect_policy_within(credential.0.hosts.clone()))
                    .inspect_err(|e| {
                        tracing::warn!("Unable to prepare a client for {}: {e}", credential.0.name)
                    })
                    .ok()
            });
            if let Some(client) = client {
                return client.get(url).header(&credential.0.name, credential.0.value.clone());
            }
        }
        self.client.get(url)
    }

    /// Downloads every task that is not already present (`verify_existing` also checks the hash
    /// of present files). Only a failed storage preflight is an `Err`; individual failures are
    /// listed in the report.
    pub async fn download_all(
        &self,
        tasks: Vec<DownloadTask>,
        verify_existing: bool,
        progress: ProgressFn<'_>,
    ) -> AppResult<DownloadReport> {
        let mut report = DownloadReport::default();
        let unique = unique_destinations(tasks, &mut report);
        let (present, pending) = split_present(unique, verify_existing).await?;
        report.skipped += present;
        let requests: Vec<SpaceRequest> = pending
            .iter()
            .filter_map(|t| {
                let dir = t.dest.parent()?.to_path_buf();
                Some(SpaceRequest { dir, bytes: t.size.unwrap_or(0), label: t.name() })
            })
            .collect();
        tokio::task::spawn_blocking(move || preflight(&requests))
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
        let counters = Counters {
            files_done: AtomicUsize::new(report.skipped),
            files_total: report.skipped + pending.len(),
            bytes_done: AtomicU64::new(0),
            bytes_total: pending.iter().filter_map(|t| t.size).sum(),
            unreached: AtomicUsize::new(0),
        };
        progress(counters.snapshot());
        let counters = &counters;
        let results: Vec<(String, Result<(), Failed>)> = futures_util::stream::iter(pending)
            .map(|task| async move {
                let result = self.download_with_retries(&task, counters, progress).await;
                counters.files_done.fetch_add(1, Ordering::Relaxed);
                progress(counters.snapshot());
                (task.name(), result)
            })
            .buffer_unordered(self.cfg.workers.max(1))
            .collect()
            .await;
        for (name, result) in results {
            match result {
                Ok(()) => report.downloaded += 1,
                Err(Failed { reason, mismatch }) => {
                    tracing::warn!("Download of {name} failed: {reason}");
                    report.failed.push(format!("{name}: {reason}"));
                    report.mismatched += usize::from(mismatch);
                }
            }
        }
        Ok(report)
    }

    /// One file, as `download_all` fetches it (a present one with the right digest stays), with
    /// its failure as the error: a digest that never matched is `IntegrityMismatch`.
    pub async fn download_one(&self, task: DownloadTask, progress: ProgressFn<'_>) -> AppResult<()> {
        let report = self.download_all(vec![task], true, progress).await?;
        match report.failed.first() {
            Some(reason) if report.mismatched > 0 => {
                Err(AppError::new(ErrorCode::IntegrityMismatch, reason.clone())
                    .with_param("error", reason.clone()))
            }
            _ => report.into_result().map(|_| ()),
        }
    }

    async fn download_with_retries(
        &self,
        task: &DownloadTask,
        counters: &Counters,
        progress: ProgressFn<'_>,
    ) -> Result<(), Failed> {
        let mut reported = 0;
        let mut last = Failed { reason: String::from("no attempt was made"), mismatch: false };
        let attempts = self.cfg.retries.max(1);
        for attempt in 1..=attempts {
            if counters.network_down() {
                counters.set_file_bytes(&mut reported, 0);
                return Err(Failed { reason: NETWORK_DOWN.into(), mismatch: false });
            }
            let mut wait = None;
            match self.download_once(task, counters, progress, &mut reported).await {
                Ok(()) => {
                    counters.reached();
                    return Ok(());
                }
                Err(Failure::Fatal(e)) => {
                    counters.set_file_bytes(&mut reported, 0);
                    return Err(Failed { reason: e, mismatch: false });
                }
                Err(Failure::Retry(e)) => last = Failed { reason: e, mismatch: false },
                Err(Failure::Unreachable(e)) => {
                    counters.unreached();
                    last = Failed { reason: e, mismatch: false };
                }
                Err(Failure::Wait(e, after)) => {
                    wait = Some(after);
                    last = Failed { reason: e, mismatch: false };
                }
                Err(Failure::Mismatch(e)) => last = Failed { reason: e, mismatch: true },
            }
            if attempt < attempts {
                // Longer each time (×4): a busy server gets room, unless it says how long itself.
                let backoff = self.cfg.retry_delay * 4u32.saturating_pow(attempt - 1);
                tokio::time::sleep(wait.unwrap_or(backoff)).await;
            }
        }
        counters.set_file_bytes(&mut reported, 0);
        Err(last)
    }

    async fn download_once(
        &self,
        task: &DownloadTask,
        counters: &Counters,
        progress: ProgressFn<'_>,
        reported: &mut u64,
    ) -> Result<(), Failure> {
        let url = Url::parse(&task.url).map_err(|e| Failure::Fatal(format!("invalid address: {e}")))?;
        if !url_allowed(&url) {
            return Err(Failure::Fatal(HTTPS_ONLY.into()));
        }
        let identity = identity(task);
        let partial = Partial::new(&task.dest, &identity);
        if task.size.is_some_and(|size| size <= self.cfg.in_memory_up_to) {
            self.fetch_whole(url, task, &partial, counters, progress, reported).await
        } else {
            self.stream(url, task, &identity, &partial, counters, progress, reported).await
        }
    }

    /// A small file: the whole body in memory, its digest on the fly, then one write.
    async fn fetch_whole(
        &self,
        url: Url,
        task: &DownloadTask,
        partial: &Partial,
        counters: &Counters,
        progress: ProgressFn<'_>,
        reported: &mut u64,
    ) -> Result<(), Failure> {
        let mut response = self.get(task, url).send().await.map_err(send_failure)?;
        if let Some(strayed) = task.strayed(response.url()) {
            return Err(strayed);
        }
        let status = response.status();
        if status == StatusCode::PARTIAL_CONTENT {
            return Err(Failure::Retry("the server sent an unusable range".into()));
        }
        if !status.is_success() {
            return Err(status_failure(&response));
        }
        let limit = task.size.unwrap_or(self.cfg.in_memory_up_to);
        let mut body = Vec::with_capacity(usize::try_from(limit).unwrap_or(0));
        let mut hasher = task.hash.as_ref().map(|h| Hasher::new(h.kind));
        counters.set_file_bytes(reported, 0);
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => return Err(Failure::Retry(e.to_string())),
            };
            if body.len() as u64 + chunk.len() as u64 > limit {
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Fatal(format!("the server sent more than {limit} bytes")));
            }
            body.extend_from_slice(&chunk);
            if let Some(hasher) = &mut hasher {
                hasher.update(&chunk);
            }
            counters.set_file_bytes(reported, body.len() as u64);
            progress(counters.snapshot());
        }
        let written = body.len() as u64;
        if let Some(size) = task.size
            && written < size
        {
            return Err(Failure::Retry(format!("the connection closed after {written} of {size} bytes")));
        }
        if let (Some(expected), Some(hasher)) = (&task.hash, hasher) {
            let actual = hasher.finish();
            if actual != expected.hex {
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Mismatch(mismatch(expected, &actual)));
            }
        }
        let (partial, dest) = (partial.clone(), task.dest.clone());
        tokio::task::spawn_blocking(move || {
            write_whole(&partial.whole(), &dest, &body)?;
            partial.forget_older();
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(fatal)?
        .map_err(fatal)
    }

    /// A large file (or one of unknown size): streamed to its partial file, resumed when the
    /// server allows it.
    #[allow(clippy::too_many_arguments)]
    async fn stream(
        &self,
        url: Url,
        task: &DownloadTask,
        identity: &str,
        partial: &Partial,
        counters: &Counters,
        progress: ProgressFn<'_>,
        reported: &mut u64,
    ) -> Result<(), Failure> {
        let dir = task.dest.parent().ok_or_else(|| fatal("the destination has no folder"))?;
        fs::create_dir_all(dir).map_err(fatal)?;
        let resume = partial.resumable(identity, task.size, task.hash.is_some());
        let mut request = self.get(task, url);
        if let Some((offset, validator)) = &resume {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
            if let Some((_, validator)) = validator {
                request = request.header(header::IF_RANGE, validator.as_str());
            }
        }
        let mut response = request.send().await.map_err(send_failure)?;
        if let Some(strayed) = task.strayed(response.url()) {
            return Err(strayed);
        }
        let status = response.status();
        let limit = task.size.unwrap_or(self.cfg.max_size);
        let (mut file, mut written) = match (status, resume) {
            (StatusCode::PARTIAL_CONTENT, Some((offset, validator)))
                if range_matches(response.headers(), offset, task.size, limit)
                    && same_version(response.headers(), &validator) =>
            {
                (OpenOptions::new().append(true).open(&partial.part).map_err(fatal)?, offset)
            }
            (StatusCode::PARTIAL_CONTENT, _) => {
                // A range we cannot trust is never spliced in: the next attempt starts over.
                partial.discard();
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Retry("the server sent an unusable range".into()));
            }
            (StatusCode::RANGE_NOT_SATISFIABLE, Some(_)) => {
                partial.discard();
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Retry("the server refused to resume".into()));
            }
            (status, _) if status.is_success() => {
                partial.start(identity, response.headers()).map_err(fatal)?;
                (File::create(&partial.part).map_err(fatal)?, 0)
            }
            (_, _) => return Err(status_failure(&response)),
        };
        counters.set_file_bytes(reported, written);
        progress(counters.snapshot());
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => return Err(Failure::Retry(e.to_string())),
            };
            written += chunk.len() as u64;
            if written > limit {
                drop(file);
                partial.discard();
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Fatal(format!("the server sent more than {limit} bytes")));
            }
            file.write_all(&chunk).map_err(fatal)?;
            counters.set_file_bytes(reported, written);
            progress(counters.snapshot());
        }
        let file = tokio::task::spawn_blocking(move || file.sync_all().map(|()| file))
            .await
            .map_err(fatal)?
            .map_err(fatal)?;
        drop(file);
        if let Some(size) = task.size
            && written < size
        {
            return Err(Failure::Retry(format!("the connection closed after {written} of {size} bytes")));
        }
        // Hashed on a blocking thread once whole: measured faster than hashing each chunk on the
        // async threads, which the network needs.
        if let Some(expected) = &task.hash {
            let (path, kind) = (partial.part.clone(), expected.kind);
            let actual = tokio::task::spawn_blocking(move || hash_file(&path, kind))
                .await
                .map_err(fatal)?
                .map_err(fatal)?;
            if actual != expected.hex {
                partial.discard();
                counters.set_file_bytes(reported, 0);
                return Err(Failure::Mismatch(mismatch(expected, &actual)));
            }
        }
        let (part, dest, sidecar) = (partial.part.clone(), task.dest.clone(), partial.sidecar.clone());
        tokio::task::spawn_blocking(move || {
            rename_retrying(&part, &dest)?;
            let _ = fs::remove_file(&sidecar);
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(fatal)?
        .map_err(fatal)
    }
}
