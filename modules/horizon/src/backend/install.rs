//! Horizon's nightly build on this computer, under `<minecraft>/horizon/`: looked for at most once a
//! day, downloaded when Horizon has signed a new one, unpacked whole before it is used. When the
//! network fails, the one here plays.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::core_app::CoreApp;
use launcher_core::net::downloader::{DownloadProgress, DownloadTask, ExpectedHash};
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};

use super::release::{self, NIGHTLY, NIGHTLY_RELEASE};

/// How long a look at the nightly holds.
const DAY: u64 = 24 * 60 * 60;
/// The build in use and when the nightly was last looked at.
const STATE: &str = "current.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Current {
    sha256: String,
    archive: String,
    checked: u64,
}

/// An unpacked Horizon build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Its archive's SHA-256, shortened: which build it is.
    pub id: String,
    pub client: PathBuf,
    pub prepare: PathBuf,
}

/// The module's folder in the Minecraft folder `minecraft`.
pub fn root(minecraft: &Path) -> PathBuf {
    minecraft.join("horizon")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn build_dir(root: &Path, sha256: &str) -> PathBuf {
    root.join("builds").join(&sha256[..12.min(sha256.len())])
}

/// `name` in `dir` or a folder up to two levels under it (an archive keeps its files in one).
fn find(dir: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    let here = dir.join(name);
    if here.is_file() {
        return Some(here);
    }
    if depth == 0 {
        return None;
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find(&e.path(), name, depth - 1))
}

fn installed(root: &Path, current: &Current) -> Option<Installed> {
    let client = find(&build_dir(root, &current.sha256), &exe("horizon-client"), 2)?;
    let prepare = client.with_file_name(exe("horizon-prepare"));
    prepare.is_file().then(|| Installed {
        id: current.sha256[..12.min(current.sha256.len())].into(),
        client,
        prepare,
    })
}

fn read_state(root: &Path) -> Option<Current> {
    serde_json::from_slice(&fs::read(root.join(STATE)).ok()?).ok()
}

fn write_state(root: &Path, current: &Current) {
    let written = serde_json::to_vec_pretty(current).map_err(io::Error::other).and_then(|bytes| {
        fs::create_dir_all(root)?;
        fs::write(root.join(STATE), bytes)
    });
    if let Err(e) = written {
        tracing::warn!("Unable to note Horizon's build: {e}");
    }
}

fn io_error(path: &Path, e: io::Error) -> AppError {
    AppError::new(ErrorCode::of_io(&e), format!("{}: {e}", path.display()))
        .with_param("path", path.to_string_lossy())
}

/// Horizon's build for this computer, the newest signed nightly when it was looked at over a day
/// ago; `report` follows a download.
pub async fn ensure(core: &CoreApp, report: &(dyn Fn(f64, f64) + Send + Sync)) -> AppResult<Installed> {
    let root = root(&core.paths.minecraft_dir);
    let current = read_state(&root);
    if let Some(found) =
        current.as_ref().filter(|c| now().saturating_sub(c.checked) < DAY).and_then(|c| installed(&root, c))
    {
        return Ok(found);
    }
    let newest = match signed_nightly(core, &root).await {
        Ok(newest) => newest,
        Err(e) => {
            return match current.as_ref().and_then(|c| installed(&root, c)) {
                Some(found) => {
                    tracing::warn!(
                        "Horizon's nightly cannot be looked at now; the build here plays: {}",
                        e.detail
                    );
                    Ok(found)
                }
                None => Err(e),
            };
        }
    };
    if installed(&root, &newest).is_none() {
        download(core, &root, &newest, report).await?;
    }
    write_state(&root, &newest);
    forget_others(&root, &newest);
    installed(&root, &newest)
        .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, "Horizon's archive has no game in it"))
}

/// This computer's archive of the newest nightly and its SHA-256, from checksums Horizon signed.
async fn signed_nightly(core: &CoreApp, root: &Path) -> AppResult<Current> {
    let archive = release::archive_for(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "Horizon has no build for this computer"))?;
    // Where 0.1.2 and before kept the checksums.
    let _ = fs::remove_dir_all(root.join("index"));
    let title =
        core.meta.get_json(NIGHTLY_RELEASE).await.ok().and_then(|r| r["name"].as_str().map(str::to_string));
    let sums = core.meta.get_bytes(&format!("{NIGHTLY}/SHA256SUMS")).await?;
    let mut signatures = Vec::new();
    for name in ["SHA256SUMS.v2.sig", "SHA256SUMS.sig"] {
        let sig = core.meta.get_bytes(&format!("{NIGHTLY}/{name}")).await.ok();
        signatures.push(sig.map(|s| String::from_utf8_lossy(&s).into_owned()));
    }
    if !release::nightly_signed(title.as_deref(), &sums, signatures[0].as_deref(), signatures[1].as_deref()) {
        return Err(AppError::new(
            ErrorCode::IntegrityMismatch,
            "Horizon's checksums carry no signature of Horizon's",
        ));
    }
    let sha256 = release::checksum(&String::from_utf8_lossy(&sums), archive).ok_or_else(|| {
        AppError::new(ErrorCode::NoFileFound, format!("Horizon's nightly has no {archive}"))
    })?;
    Ok(Current { sha256, archive: archive.to_string(), checked: now() })
}

/// Downloads `current`'s archive (checked by its SHA-256) and unpacks it beside its folder first.
async fn download(
    core: &CoreApp,
    root: &Path,
    current: &Current,
    report: &(dyn Fn(f64, f64) + Send + Sync),
) -> AppResult<()> {
    let file = root.join("downloads").join(&current.archive);
    let task = DownloadTask::new(format!("{NIGHTLY}/{}", current.archive), &file)
        .hash(ExpectedHash::sha256(&current.sha256));
    let progress = |p: DownloadProgress| report(p.bytes_done as f64, p.bytes_total.max(1) as f64);
    core.downloader.download_one(task, &progress).await?;
    let (dir, part) = (build_dir(root, &current.sha256), root.join("builds").join(".unpacking"));
    tokio::task::spawn_blocking(move || {
        let _ = fs::remove_dir_all(&part);
        fs::create_dir_all(&part).map_err(|e| io_error(&part, e))?;
        unpack(&file, &part).map_err(|e| io_error(&file, e))?;
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&part, &dir).map_err(|e| io_error(&dir, e))?;
        let _ = fs::remove_file(&file);
        Ok(())
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?
}

/// Unpacks `archive` into `into`: a zip entry by entry (none outside `into`), a `.tar.gz` by the
/// system's `tar`, which refuses such entries itself.
fn unpack(archive: &Path, into: &Path) -> io::Result<()> {
    if archive.to_string_lossy().ends_with(".tar.gz") {
        let status =
            std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(into).status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("tar ended with {status}")))
        };
    }
    let mut zip =
        zip::ZipArchive::new(io::BufReader::new(fs::File::open(archive)?)).map_err(io::Error::other)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(io::Error::other)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(io::Error::other(format!("{:?} lies outside the archive", entry.name_raw())));
        };
        let path = into.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut fs::File::create(&path)?)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// The builds other than `current`'s go; one a running game still holds stays until next time.
fn forget_others(root: &Path, current: &Current) {
    let keep = build_dir(root, &current.sha256);
    for entry in fs::read_dir(root.join("builds")).into_iter().flatten().flatten() {
        if entry.path() != keep && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn an_unpacked_build_is_found_by_its_programs() {
        let root = tempfile::tempdir().unwrap();
        let current = Current { sha256: "b942cd4c5ea42065a3fe".into(), archive: "a.zip".into(), checked: 0 };
        assert_eq!(installed(root.path(), &current), None);
        let archive = root.path().join("a.zip");
        let (client, prepare) = (exe("horizon-client"), exe("horizon-prepare"));
        let entries = [
            (format!("horizon-windows-x64/{client}"), b"c" as &[u8]),
            (format!("horizon-windows-x64/{prepare}"), b"p"),
        ];
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), *b)).collect();
        fs::write(&archive, zip_of(&refs)).unwrap();
        let dir = build_dir(root.path(), &current.sha256);
        fs::create_dir_all(&dir).unwrap();
        unpack(&archive, &dir).unwrap();
        let found = installed(root.path(), &current).unwrap();
        assert_eq!(found.id, "b942cd4c5ea4");
        assert_eq!(found.client, dir.join("horizon-windows-x64").join(&client));
        assert_eq!(fs::read(&found.prepare).unwrap(), b"p");
    }

    #[test]
    fn an_entry_outside_the_archive_stops_the_unpacking() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("evil.zip");
        fs::write(&archive, zip_of(&[("../outside.txt", b"x")])).unwrap();
        let into = root.path().join("into");
        fs::create_dir_all(&into).unwrap();
        assert!(unpack(&archive, &into).is_err());
        assert!(!root.path().join("outside.txt").exists());
    }

    #[test]
    fn only_the_build_in_use_stays() {
        let root = tempfile::tempdir().unwrap();
        let current = Current { sha256: "aaaaaaaaaaaa1111".into(), archive: "a.zip".into(), checked: 0 };
        for sha in ["aaaaaaaaaaaa1111", "bbbbbbbbbbbb2222"] {
            fs::create_dir_all(build_dir(root.path(), sha)).unwrap();
        }
        forget_others(root.path(), &current);
        assert!(build_dir(root.path(), "aaaaaaaaaaaa").exists());
        assert!(!build_dir(root.path(), "bbbbbbbbbbbb").exists());
        write_state(root.path(), &current);
        assert_eq!(read_state(root.path()), Some(current));
    }
}
