//! The Components page's view of `versions/`: what
//! each installed version is, how big, when it changed, and which builds and components need it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use launcher_shared::{
    AppError, AppResult, ComponentDto, ComponentsSnapshot, ErrorCode, LoaderKind, Text, VerifyOutcome,
};
use serde_json::{Map, Value};

use crate::builds::ids::validate_component_id;
use crate::feedback::{FeedbackService, OperationHandle, OperationSpec};
use crate::loaders::versions::sort_key;
use crate::loaders::{ComponentInstaller, ComponentSpec, loader_version_of};
use crate::minecraft::InstallProgress;
use crate::minecraft::version::{read_version_json, version_dir, version_json_path};
use crate::storage::versions::{Build, VersionStore};

/// What an installed version is: a loader by its id, plain Minecraft when it
/// inherits from nothing, unknown otherwise. Returns (loader, Minecraft version, loader build).
pub fn classify(id: &str, json: &Map<String, Value>) -> (Option<LoaderKind>, Option<String>, Option<String>) {
    let parent = json.get("inheritsFrom").and_then(Value::as_str).map(str::to_string);
    let loader = match (LoaderKind::of_component(id), &parent) {
        (Some(kind), _) => Some(kind),
        (None, None) => Some(LoaderKind::Minecraft),
        (None, Some(_)) => None,
    };
    let minecraft = if loader == Some(LoaderKind::Minecraft) { Some(id.to_string()) } else { parent };
    let loader_version = match (loader, &minecraft) {
        (Some(kind), Some(mc)) => loader_version_of(kind, mc, id),
        _ => None,
    };
    (loader, minecraft, loader_version)
}

/// Bytes of the files under `path`; links are not followed.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Every readable version in `versions/` (a folder whose version JSON names it), with the builds
/// that use it (`loader` or `version` equal to its id) and the components that inherit from it;
/// newest Minecraft first. Blocking.
pub fn list_components(mc_dir: &Path, builds: &[Build]) -> Vec<ComponentDto> {
    let Ok(entries) = fs::read_dir(mc_dir.join("versions")) else { return Vec::new() };
    let mut found: Vec<(ComponentDto, Option<String>)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_string) else { continue };
        if validate_component_id(&id).is_err() {
            continue;
        }
        let Ok(json) = read_version_json(mc_dir, &id) else { continue };
        let parent = json.get("inheritsFrom").and_then(Value::as_str).map(str::to_string);
        let (loader, minecraft, loader_version) = classify(&id, &json);
        let mut used_by: Vec<String> = builds
            .iter()
            .filter(|b| b.loader.as_deref() == Some(id.as_str()) || b.version.as_deref() == Some(id.as_str()))
            .map(|b| b.name.clone())
            .collect();
        used_by.sort_by_key(|name| name.to_lowercase());
        let size = dir_size(&version_dir(mc_dir, &id));
        let modified_ms = modified_ms(&version_json_path(mc_dir, &id));
        let component = ComponentDto {
            id,
            loader,
            minecraft,
            loader_version,
            size,
            modified_ms,
            used_by,
            base_for: Vec::new(),
        };
        found.push((component, parent));
    }
    let parents: Vec<(String, Option<String>)> =
        found.iter().map(|(c, p)| (c.id.clone(), p.clone())).collect();
    let mut components: Vec<ComponentDto> = found
        .into_iter()
        .map(|(mut component, _)| {
            let mut base_for: Vec<String> = parents
                .iter()
                .filter(|(_, parent)| parent.as_deref() == Some(component.id.as_str()))
                .map(|(id, _)| id.clone())
                .collect();
            base_for.sort();
            component.base_for = base_for;
            component
        })
        .collect();
    components.sort_by(|a, b| {
        let key = |c: &ComponentDto| sort_key(c.minecraft.as_deref().unwrap_or_default());
        key(b).cmp(&key(a)).then_with(|| a.id.cmp(&b.id))
    });
    components
}

fn not_installed(id: &str) -> AppError {
    AppError::new(ErrorCode::VersionNotFound, format!("no component {id}")).with_param("version", id)
}

/// Installs, verifies, repairs and deletes components, one operation at a
/// time; successes are announced here, failures go to the activity log and back to the caller.
pub struct ComponentManager {
    versions: Arc<VersionStore>,
    components: Arc<ComponentInstaller>,
    feedback: Arc<FeedbackService>,
}

impl ComponentManager {
    pub fn new(
        versions: Arc<VersionStore>,
        components: Arc<ComponentInstaller>,
        feedback: Arc<FeedbackService>,
    ) -> ComponentManager {
        ComponentManager { versions, components, feedback }
    }

    fn mc_dir(&self) -> &Path {
        self.versions.minecraft_dir()
    }

    pub async fn list(&self) -> ComponentsSnapshot {
        let (mc_dir, builds) = (self.mc_dir().to_path_buf(), self.versions.list());
        let components =
            tokio::task::spawn_blocking(move || list_components(&mc_dir, &builds)).await.unwrap_or_default();
        ComponentsSnapshot { components }
    }

    /// What installed component `id` is; refuses an unsafe or unknown id. `None` for a loader the
    /// launcher cannot name (it is repaired as a plain version).
    fn find(&self, id: &str) -> AppResult<Option<ComponentSpec>> {
        validate_component_id(id)?;
        let json = read_version_json(self.mc_dir(), id).map_err(|_| not_installed(id))?;
        Ok(match classify(id, &json) {
            (Some(LoaderKind::Minecraft), Some(mc), _) => Some(ComponentSpec::vanilla(&mc)),
            (Some(kind), Some(mc), Some(lv)) => Some(ComponentSpec::loader(kind, &mc, &lv)),
            _ => None,
        })
    }

    fn start(&self, title: Text) -> AppResult<OperationHandle> {
        if self.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        Ok(self.feedback.begin(OperationSpec::new(title.clone(), "components").status(title)))
    }

    /// Installs a component without a build (the Install mode).
    pub async fn install(&self, kind: LoaderKind, mc: &str, loader_version: Option<&str>) -> AppResult<()> {
        let spec = match (kind, loader_version) {
            (LoaderKind::Minecraft, _) => ComponentSpec::vanilla(mc),
            (kind, Some(lv)) => ComponentSpec::loader(kind, mc, lv),
            (kind, None) => {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("{} needs a loader build", kind.display_name()),
                )
                .with_param("version", mc));
            }
        };
        let label = spec.component_id();
        let op = self.start(Text::key("minecraft_components_installing").param("version", &label))?;
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        match self.components.install(&spec, false, &progress).await {
            Ok(_) => {
                op.finish();
                self.feedback
                    .success(Text::key("minecraft_components_install_complete").param("version", &label));
                Ok(())
            }
            Err(e) => {
                op.fail(
                    Text::key("minecraft_components_install_failed")
                        .param("version", &label)
                        .param("error", e.detail.clone()),
                );
                Err(e)
            }
        }
    }

    /// Checks every file of `id` against its hash; anything wrong is repaired in the same
    /// operation.
    pub async fn verify(&self, id: &str) -> AppResult<VerifyOutcome> {
        let spec = self.find(id)?;
        let op = self.start(Text::key("minecraft_components_verifying").param("version", id))?;
        let check = self.components.verify(id, spec.as_ref()).await;
        if check.valid {
            op.finish();
            self.feedback.success(Text::key("minecraft_components_verify_ok").param("version", id));
            return Ok(VerifyOutcome::Intact);
        }
        tracing::warn!("Component {id} needs repair: {}", check.issues.join("; "));
        op.status(Text::key("minecraft_components_repairing").param("version", id));
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        match self.components.repair(id, spec.as_ref(), &progress).await {
            Ok(_) => {
                op.finish();
                self.feedback.success(Text::key("minecraft_components_repair_complete").param("version", id));
                Ok(VerifyOutcome::Repaired)
            }
            Err(e) => {
                op.fail(
                    Text::key("minecraft_components_repair_failed")
                        .param("version", id)
                        .param("error", e.detail.clone()),
                );
                Err(e)
            }
        }
    }

    /// Installs `id` again with every file checked.
    pub async fn reinstall(&self, id: &str) -> AppResult<()> {
        let spec = self.find(id)?;
        let op = self.start(Text::key("minecraft_components_reinstalling").param("version", id))?;
        let progress = |p: InstallProgress| {
            let bytes = p.download.map(|d| (d.bytes_done as f64, d.bytes_total as f64));
            op.update(Some(p.status), bytes.map(|b| b.0), bytes.map(|b| b.1));
        };
        match self.components.repair(id, spec.as_ref(), &progress).await {
            Ok(_) => {
                op.finish();
                self.feedback
                    .success(Text::key("minecraft_components_reinstall_complete").param("version", id));
                Ok(())
            }
            Err(e) => {
                op.fail(
                    Text::key("minecraft_components_reinstall_failed")
                        .param("version", id)
                        .param("error", e.detail.clone()),
                );
                Err(e)
            }
        }
    }

    /// Deletes `versions/<id>` (its version JSON must name it; a link is refused). Refused while a
    /// build that uses it runs (`running` answers by build key) or another operation is busy.
    /// Shared libraries, assets and runtimes stay.
    pub fn delete(&self, id: &str, running: impl Fn(&str) -> bool) -> AppResult<()> {
        self.find(id)?;
        let users: Vec<Build> = self
            .versions
            .list()
            .into_iter()
            .filter(|b| b.loader.as_deref() == Some(id) || b.version.as_deref() == Some(id))
            .collect();
        if let Some(build) = users.iter().find(|b| running(&b.key)) {
            return Err(AppError::new(ErrorCode::BuildRunning, format!("{} is running", build.name))
                .with_param("version", &build.name));
        }
        if self.feedback.is_busy() {
            return Err(AppError::new(ErrorCode::Busy, "another operation is running"));
        }
        let _lease = self.components.minecraft().lock("component_delete")?;
        let dir = version_dir(self.mc_dir(), id);
        if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("{} is a link", dir.display()))
                .with_param("version", id));
        }
        fs::remove_dir_all(&dir).map_err(|e| {
            AppError::new(ErrorCode::VersionFilesRemain, format!("{}: {e}", dir.display()))
                .with_param("path", dir.to_string_lossy())
        })?;
        tracing::info!("Deleted component {id}");
        self.feedback.success(Text::key("minecraft_components_delete_complete").param("version", id));
        Ok(())
    }

    /// Where component `id` lives ("Open folder").
    pub fn dir(&self, id: &str) -> AppResult<PathBuf> {
        self.find(id)?;
        Ok(version_dir(self.mc_dir(), id))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn put(mc: &Path, id: &str, json: Value) {
        let dir = mc.join("versions").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{id}.json")), json.to_string()).unwrap();
        fs::write(dir.join(format!("{id}.jar")), b"0123456789").unwrap();
    }

    fn build(name: &str, version: &str, loader: &str) -> Build {
        let mut b = Build::new(name);
        b.version = Some(version.into());
        b.loader = Some(loader.into());
        b
    }

    #[test]
    fn components_know_their_loader_users_and_dependents() {
        let dir = tempfile::tempdir().unwrap();
        let mc = dir.path();
        put(mc, "1.21.1", json!({"id": "1.21.1", "mainClass": "M"}));
        put(
            mc,
            "fabric-loader-0.16.9-1.21.1",
            json!({"id": "fabric-loader-0.16.9-1.21.1", "inheritsFrom": "1.21.1"}),
        );
        put(mc, "custom", json!({"id": "custom", "inheritsFrom": "1.21.1"}));
        put(mc, "1.20.1-forge-47.4.10", json!({"id": "1.20.1-forge-47.4.10", "inheritsFrom": "1.20.1"}));
        put(mc, "other", json!({"id": "not-other"}));
        let broken = mc.join("versions").join("broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("broken.json"), "{ nope").unwrap();
        fs::write(mc.join("versions").join("readme.txt"), "x").unwrap();
        let builds =
            [build("Vanilla", "1.21.1", "1.21.1"), build("aero", "1.21.1", "fabric-loader-0.16.9-1.21.1")];
        let list = list_components(mc, &builds);
        let ids: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            ["1.21.1", "custom", "fabric-loader-0.16.9-1.21.1", "1.20.1-forge-47.4.10"],
            "newest Minecraft first; unreadable and foreign folders are left out"
        );
        let base = &list[0];
        assert_eq!((base.loader, base.minecraft.as_deref()), (Some(LoaderKind::Minecraft), Some("1.21.1")));
        assert_eq!(base.used_by, ["aero", "Vanilla"], "a loader build needs its base too");
        assert_eq!(base.base_for, ["custom", "fabric-loader-0.16.9-1.21.1"]);
        assert!(base.size >= 10 && base.modified_ms.is_some());
        assert_eq!(
            (list[1].loader, list[1].minecraft.as_deref()),
            (None, Some("1.21.1")),
            "an unknown loader"
        );
        let fabric = &list[2];
        assert_eq!(
            (fabric.loader, fabric.loader_version.as_deref(), fabric.used_by.len()),
            (Some(LoaderKind::Fabric), Some("0.16.9"), 1)
        );
        let forge = &list[3];
        assert_eq!(
            (forge.loader, forge.minecraft.as_deref(), forge.loader_version.as_deref()),
            (Some(LoaderKind::Forge), Some("1.20.1"), Some("47.4.10"))
        );
        assert!(forge.used_by.is_empty() && forge.base_for.is_empty());
        assert!(list_components(&mc.join("nowhere"), &builds).is_empty());
    }
}
