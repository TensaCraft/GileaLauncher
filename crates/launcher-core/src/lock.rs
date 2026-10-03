//! OS file locks for instances (game folders) and the shared Minecraft folder.
//! A crashed process loses its lock automatically; the
//! lock files themselves are never deleted.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LOCK_DIR: &str = ".launcher-locks";

/// Who holds a lock; written into the lock file (keys in sorted order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockOwner {
    pub acquired_at: String,
    pub hostname: String,
    pub kind: String,
    pub pid: u32,
    pub resource: String,
    pub schema: u32,
    pub token: String,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Windows paths compare case-insensitively.
fn normcase(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text.into_owned() }
}

/// One spelling per file: absolute, and case-folded where the file system ignores case.
pub(crate) fn path_key(path: &Path) -> String {
    normcase(&absolute(path))
}

/// Where a namespace keeps its lock files: beside the resource (game folders, all in `games/`)
/// or inside it (the Minecraft folder, whose parent need not be the player's to write).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPlace {
    Beside,
    Inside,
}

/// `<parent or resource>/.launcher-locks/<namespace>-<sha256(normcase(path))[..32]>.lock`.
pub fn lock_path(resource: &Path, namespace: &str, place: LockPlace) -> PathBuf {
    let resolved = absolute(resource);
    let digest = hex::encode(Sha256::digest(normcase(&resolved).as_bytes()));
    let dir = match place {
        LockPlace::Beside => resolved.parent().map(Path::to_path_buf).unwrap_or_else(|| resolved.clone()),
        LockPlace::Inside => resolved,
    };
    dir.join(LOCK_DIR).join(format!("{namespace}-{}.lock", &digest[..32]))
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "unknown".into())
}

#[derive(Debug)]
pub enum LockError {
    /// Held by another process (its owner, when the lock file could be read).
    Busy(Option<Box<LockOwner>>),
    Io(io::Error),
}

/// An exclusive OS lock on `lock_path(resource, namespace, place)`, released when dropped.
#[derive(Debug)]
pub struct OsFileLock {
    _file: File,
    pub owner: LockOwner,
}

impl OsFileLock {
    pub fn try_acquire(
        resource: &Path,
        namespace: &str,
        place: LockPlace,
        kind: &str,
        token: &str,
    ) -> Result<OsFileLock, LockError> {
        let path = lock_path(resource, namespace, place);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(LockError::Io)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                // Best effort: Windows locks forbid reading the file while it is held.
                let owner = fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str(t.trim()).ok())
                    .map(Box::new);
                return Err(LockError::Busy(owner));
            }
            Err(fs::TryLockError::Error(e)) => return Err(LockError::Io(e)),
        }
        let owner = LockOwner {
            acquired_at: chrono::Utc::now().to_rfc3339(),
            hostname: hostname(),
            kind: kind.to_string(),
            pid: std::process::id(),
            resource: absolute(resource).to_string_lossy().into_owned(),
            schema: 1,
            token: token.to_string(),
        };
        let json = serde_json::to_string(&owner).map_err(|e| LockError::Io(io::Error::other(e)))?;
        file.set_len(0).map_err(LockError::Io)?;
        file.write_all(format!("\n{json}\n").as_bytes()).map_err(LockError::Io)?;
        file.sync_all().map_err(LockError::Io)?;
        Ok(OsFileLock { _file: file, owner })
    }
}

type Active = Arc<Mutex<HashMap<String, String>>>;

/// One running operation on a resource; dropping it releases the in-process slot and the OS lock.
#[derive(Debug)]
pub struct Lease {
    pub kind: String,
    pub path: PathBuf,
    key: String,
    active: Active,
    /// Taken (released) first when the lease ends: until then the slot stays taken, so another
    /// try in this process sees it busy here rather than another program holding the file.
    lock: Option<OsFileLock>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.lock.take());
        self.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
}

/// Non-blocking, non-reentrant locks for one namespace. Nested operations receive the `Lease`
/// they run under instead of acquiring again. Lock order: instance → shared → process registry.
pub struct Coordinator {
    namespace: &'static str,
    place: LockPlace,
    busy: ErrorCode,
    active: Active,
}

impl Coordinator {
    /// Game folders (`games/<id>`): launch, installs, backups.
    pub fn instances() -> Coordinator {
        Coordinator {
            namespace: "instance",
            place: LockPlace::Beside,
            busy: ErrorCode::InstanceBusy,
            active: Arc::default(),
        }
    }

    /// The shared Minecraft folder: versions, libraries, runtimes.
    pub fn shared() -> Coordinator {
        Coordinator {
            namespace: "shared",
            place: LockPlace::Inside,
            busy: ErrorCode::SharedBusy,
            active: Arc::default(),
        }
    }

    fn busy_error(&self, path: &Path, kind: &str) -> AppError {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        AppError::new(self.busy, format!("{} is busy with {kind}", path.display()))
            .with_param("version", name)
            .with_param("kind", kind)
    }

    pub fn try_acquire(&self, path: &Path, kind: &str) -> AppResult<Lease> {
        let resolved = absolute(path);
        let key = normcase(&resolved);
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = active.get(&key) {
            return Err(self.busy_error(&resolved, current));
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let lock = match OsFileLock::try_acquire(&resolved, self.namespace, self.place, kind, &token) {
            Ok(lock) => lock,
            Err(LockError::Busy(owner)) => {
                let other = owner.map(|o| o.kind).unwrap_or_else(|| "external_operation".to_string());
                return Err(self.busy_error(&resolved, &other));
            }
            Err(LockError::Io(e)) => return Err(AppError::new(ErrorCode::Io, e.to_string())),
        };
        active.insert(key.clone(), kind.to_string());
        Ok(Lease {
            kind: kind.to_string(),
            path: resolved,
            key,
            active: self.active.clone(),
            lock: Some(lock),
        })
    }

    pub fn active_kind(&self, path: &Path) -> Option<String> {
        let key = normcase(&absolute(path));
        self.active.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_files_live_next_to_the_resource() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("games").join("aeronautics");
        let path = lock_path(&game, "instance", LockPlace::Beside);
        assert_eq!(path.parent().unwrap(), dir.path().join("games").join(LOCK_DIR));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("instance-") && name.ends_with(".lock"), "{name}");
        assert_eq!(name.len(), "instance-".len() + 32 + ".lock".len());
        assert_ne!(lock_path(&game, "shared", LockPlace::Beside), path);
    }

    #[test]
    fn the_shared_lock_lives_inside_the_minecraft_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path().join("minecraft");
        let lease = Coordinator::shared().try_acquire(&mc, "install").unwrap();
        assert_eq!(fs::read_dir(mc.join(LOCK_DIR)).unwrap().count(), 1);
        assert!(!dir.path().join(LOCK_DIR).exists(), "nothing is written beside the folder");
        assert!(Coordinator::shared().try_acquire(&mc, "install").is_err(), "and it still locks");
        drop(lease);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case() {
        let dir = tempfile::tempdir().unwrap();
        let upper = dir.path().join("Games").join("Aero");
        let lower = PathBuf::from(upper.to_string_lossy().to_lowercase());
        assert_eq!(
            lock_path(&upper, "instance", LockPlace::Beside).file_name(),
            lock_path(&lower, "instance", LockPlace::Beside).file_name()
        );
    }

    #[test]
    fn a_second_operation_on_the_same_instance_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("games").join("aero");
        let instances = Coordinator::instances();
        let lease = instances.try_acquire(&game, "launch").unwrap();
        assert_eq!(instances.active_kind(&game).as_deref(), Some("launch"));
        let err = instances.try_acquire(&game, "install").unwrap_err();
        assert_eq!(err.code, ErrorCode::InstanceBusy);
        assert_eq!(err.params.get("version").map(String::as_str), Some("aero"));
        assert_eq!(err.params.get("kind").map(String::as_str), Some("launch"));
        let other = dir.path().join("games").join("other");
        drop(instances.try_acquire(&other, "install").unwrap());
        drop(lease);
        assert_eq!(instances.active_kind(&game), None);
        instances.try_acquire(&game, "install").unwrap();
    }

    #[test]
    fn a_lock_held_elsewhere_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path().join("minecraft");
        // Another launcher process holds the OS lock (simulated by a second handle).
        let foreign =
            OsFileLock::try_acquire(&mc, "shared", LockPlace::Inside, "minecraft_install", "other-token")
                .unwrap();
        let shared = Coordinator::shared();
        assert_eq!(shared.try_acquire(&mc, "java_runtime").unwrap_err().code, ErrorCode::SharedBusy);
        drop(foreign);
        shared.try_acquire(&mc, "java_runtime").unwrap();
    }

    #[test]
    fn a_leftover_lock_file_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("games").join("aero");
        let path = lock_path(&game, "instance", LockPlace::Beside);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "\n{\"kind\":\"launch\",\"pid\":1}\n").unwrap();
        Coordinator::instances().try_acquire(&game, "launch").unwrap();
    }

    #[test]
    fn the_lock_file_names_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("games").join("aero");
        drop(Coordinator::instances().try_acquire(&game, "world_backup").unwrap());
        let text = std::fs::read_to_string(lock_path(&game, "instance", LockPlace::Beside)).unwrap();
        assert!(text.starts_with('\n') && text.ends_with('\n'), "{text:?}");
        let owner: LockOwner = serde_json::from_str(text.trim()).unwrap();
        assert_eq!((owner.kind.as_str(), owner.schema, owner.pid), ("world_backup", 1, std::process::id()));
        assert_eq!(owner.resource, std::path::absolute(&game).unwrap().to_string_lossy());
        assert!(text.find("\"acquired_at\"").unwrap() < text.find("\"token\"").unwrap(), "sorted keys");
    }
}
